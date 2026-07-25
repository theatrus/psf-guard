//! Remote plugin protocol coverage: token scope, export, preview, and apply.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::DefaultBodyLimit,
    http::{Request, StatusCode},
    routing::{get, post},
    Router,
};
use http_body_util::BodyExt;
use psf_guard::{
    cli::PregenerationConfig,
    db_registry::{DbEntry, RemoteImageUploadConfig},
    server::{remote_sync, state::AppState},
};
use rusqlite::Connection;
use serde_json::{json, Value};
use tempfile::tempdir;
use tower::ServiceExt;

const SOURCE_TOKEN: &str = "source-remote-api-key-1234567890";
const DESTINATION_TOKEN: &str = "destination-remote-api-key-123456";

fn schema(connection: &Connection) {
    connection
        .execute_batch(
            "PRAGMA user_version=22;
             CREATE TABLE project (Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT, description TEXT, state INTEGER, priority INTEGER, guid TEXT);
             CREATE TABLE target (Id INTEGER PRIMARY KEY, name TEXT, active INTEGER, ra REAL, dec REAL, epochcode INTEGER, projectid INTEGER, guid TEXT);
             CREATE TABLE exposuretemplate (Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT, filtername TEXT, gain INTEGER, guid TEXT);
             CREATE TABLE exposureplan (Id INTEGER PRIMARY KEY, profileId TEXT, exposure REAL, desired INTEGER, acquired INTEGER, accepted INTEGER, enabled INTEGER, targetid INTEGER, exposureTemplateId INTEGER, guid TEXT);
             CREATE TABLE acquiredimage (Id INTEGER PRIMARY KEY, projectId INTEGER, targetId INTEGER, acquireddate INTEGER, filtername TEXT, gradingStatus INTEGER NOT NULL, metadata TEXT NOT NULL, rejectreason TEXT, profileId TEXT, exposureId INTEGER, guid TEXT);
             CREATE TABLE ruleweight (Id INTEGER PRIMARY KEY, name TEXT, weight REAL, projectid INTEGER);
             CREATE TABLE imagedata (Id INTEGER PRIMARY KEY, tag TEXT, imagedata BLOB, acquiredimageid INTEGER, width INTEGER, height INTEGER);",
        )
        .unwrap();
}

fn api_config(token: &str) -> RemoteImageUploadConfig {
    let mut config = RemoteImageUploadConfig {
        enabled: false,
        sync_enabled: true,
        ..Default::default()
    };
    config.set_token(token).unwrap();
    config
}

/// A key configured for image upload only, from before the sync protocol.
fn upload_only_config(token: &str) -> RemoteImageUploadConfig {
    let mut config = api_config(token);
    config.sync_enabled = false;
    config
}

fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/sync/v1/capabilities", get(remote_sync::capabilities))
        .route(
            "/api/sync/v1/previews",
            post(remote_sync::create_preview)
                .layer(DefaultBodyLimit::max(remote_sync::MAX_SYNC_BODY_BYTES)),
        )
        .route(
            "/api/sync/v1/previews/{preview_id}",
            get(remote_sync::get_preview),
        )
        .route(
            "/api/sync/v1/previews/{preview_id}/apply",
            post(remote_sync::apply_preview),
        )
        .route("/api/sync/v1/exports", post(remote_sync::create_export))
        .route(
            "/api/sync/v1/exports/{export_id}",
            get(remote_sync::get_export),
        )
        .with_state(state)
}

async fn call(
    app: Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let response = app
        .oneshot(
            builder
                .body(body.map_or_else(Body::empty, |value| {
                    Body::from(serde_json::to_vec(&value).unwrap())
                }))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn token_scoped_export_previews_and_applies_to_the_selected_database() {
    let directory = tempdir().unwrap();
    let source_path = directory.path().join("source.sqlite");
    let destination_path = directory.path().join("destination.sqlite");
    let image_path = directory.path().join("images");
    std::fs::create_dir(&image_path).unwrap();
    let source = Connection::open(&source_path).unwrap();
    let destination = Connection::open(&destination_path).unwrap();
    schema(&source);
    schema(&destination);
    source
        .execute_batch(
            "INSERT INTO project VALUES (1,'p','M42','remote settings',2,8,'project-guid');
             INSERT INTO target VALUES (1,'M42',1,5.5,-5.4,0,1,'target-guid');
             INSERT INTO exposuretemplate VALUES (1,'p','Ha 300','Ha',120,'template-guid');
             INSERT INTO exposureplan VALUES (1,'p',300,40,18,16,1,1,1,'plan-guid');
             INSERT INTO ruleweight VALUES (1,'Priority',2.5,1);",
        )
        .unwrap();
    destination
        .execute_batch(
            "INSERT INTO project VALUES (20,'p','M42','old settings',1,2,'project-guid');
             INSERT INTO target VALUES (30,'Old target',1,5.4,-5.3,0,20,'target-guid');
             INSERT INTO exposuretemplate VALUES (40,'p','Old Ha','Ha',100,'template-guid');
             INSERT INTO exposureplan VALUES (50,'p',180,20,7,6,0,30,40,'plan-guid');
             INSERT INTO ruleweight VALUES (60,'Priority',1.0,20);",
        )
        .unwrap();
    drop(source);
    drop(destination);

    let entries = vec![
        DbEntry {
            id: "source".into(),
            name: "Source".into(),
            db_path: source_path.to_string_lossy().into_owned(),
            image_dirs: vec![image_path.to_string_lossy().into_owned()],
            reject_archive: None,
            remote_image_upload: Some(api_config(SOURCE_TOKEN)),
        },
        DbEntry {
            id: "destination".into(),
            name: "Destination".into(),
            db_path: destination_path.to_string_lossy().into_owned(),
            image_dirs: vec![image_path.to_string_lossy().into_owned()],
            reject_archive: None,
            remote_image_upload: Some(api_config(DESTINATION_TOKEN)),
        },
    ];
    let state = Arc::new(
        AppState::from_databases(
            entries,
            directory
                .path()
                .join("cache")
                .to_string_lossy()
                .into_owned(),
            PregenerationConfig::default(),
        )
        .unwrap(),
    );
    let router = app(state);

    let (status, capabilities) = call(
        router.clone(),
        "GET",
        "/api/sync/v1/capabilities",
        Some(DESTINATION_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(capabilities["data"]["catalogs"][0]["id"], "destination");
    assert_eq!(
        capabilities["data"]["catalogs"].as_array().unwrap().len(),
        1
    );

    let (status, _) = call(
        router.clone(),
        "POST",
        "/api/sync/v1/exports",
        Some(SOURCE_TOKEN),
        Some(json!({
            "protocol_version": 1,
            "catalog_id": "destination",
            "operation": "push_planning",
            "reviewed_only": false
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, exported) = call(
        router.clone(),
        "POST",
        "/api/sync/v1/exports",
        Some(SOURCE_TOKEN),
        Some(json!({
            "protocol_version": 1,
            "catalog_id": "source",
            "operation": "push_planning",
            "reviewed_only": false
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let bundle = exported["data"]["bundle"].clone();
    assert_eq!(bundle["payload_sha256"].as_str().unwrap().len(), 64);

    let (status, preview) = call(
        router.clone(),
        "POST",
        "/api/sync/v1/previews",
        Some(DESTINATION_TOKEN),
        Some(json!({
            "protocol_version": 1,
            "catalog_id": "destination",
            "operation": "push_planning",
            "bundle": bundle
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview:#}");
    assert_eq!(preview["data"]["summary"]["project_updated"], 1);
    let preview_id = preview["data"]["preview_id"].as_str().unwrap();

    let destination = Connection::open(&destination_path).unwrap();
    let description: String = destination
        .query_row("SELECT description FROM project", [], |row| row.get(0))
        .unwrap();
    assert_eq!(description, "old settings");
    drop(destination);

    let (status, _) = call(
        router.clone(),
        "GET",
        &format!("/api/sync/v1/previews/{preview_id}"),
        Some(SOURCE_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, applied) = call(
        router,
        "POST",
        &format!("/api/sync/v1/previews/{preview_id}/apply"),
        Some(DESTINATION_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied:#}");
    assert_eq!(applied["data"]["state"], "applied");
    assert_eq!(applied["data"]["summary"]["total_updated"], 5);

    let destination = Connection::open(destination_path).unwrap();
    let description: String = destination
        .query_row("SELECT description FROM project", [], |row| row.get(0))
        .unwrap();
    assert_eq!(description, "remote settings");
}

/// A key that predates the sync protocol authenticates but reaches nothing.
/// Upgrading PSF Guard must not hand an existing upload token the power to
/// merge into the user's scheduler database.
#[tokio::test]
async fn an_upload_only_key_cannot_reach_the_sync_protocol() {
    let directory = tempdir().unwrap();
    let database_path = directory.path().join("catalog.sqlite");
    let image_path = directory.path().join("images");
    std::fs::create_dir(&image_path).unwrap();
    let connection = Connection::open(&database_path).unwrap();
    schema(&connection);
    drop(connection);

    let state = Arc::new(
        AppState::from_databases(
            vec![DbEntry {
                id: "catalog".into(),
                name: "Catalog".into(),
                db_path: database_path.to_string_lossy().into_owned(),
                image_dirs: vec![image_path.to_string_lossy().into_owned()],
                reject_archive: None,
                remote_image_upload: Some(upload_only_config(SOURCE_TOKEN)),
            }],
            directory
                .path()
                .join("cache")
                .to_string_lossy()
                .into_owned(),
            PregenerationConfig::default(),
        )
        .unwrap(),
    );
    let router = app(state);

    for (method, uri) in [
        ("GET", "/api/sync/v1/capabilities"),
        ("POST", "/api/sync/v1/exports"),
    ] {
        let body = (method == "POST").then(|| {
            json!({
                "protocol_version": 1,
                "catalog_id": "catalog",
                "operation": "push_planning",
                "reviewed_only": false
            })
        });
        let (status, response) = call(router.clone(), method, uri, Some(SOURCE_TOKEN), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {response:#}");
    }
}
