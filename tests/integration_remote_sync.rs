//! Remote plugin protocol coverage: token scope, export, preview, and apply.

use std::{path::PathBuf, sync::Arc};

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
        .route(
            "/api/sync/v1/previews/{preview_id}/refresh",
            post(remote_sync::refresh_preview),
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

/// Two token-scoped catalogs wired to one router, the shape every remote sync
/// exercise needs.
struct Harness {
    /// Held so the databases and cache outlive the test.
    _directory: tempfile::TempDir,
    state: Arc<AppState>,
    router: Router,
    destination_path: PathBuf,
}

impl Harness {
    fn new(source_rows: &str, destination_rows: &str) -> Self {
        let directory = tempdir().unwrap();
        let source_path = directory.path().join("source.sqlite");
        let destination_path = directory.path().join("destination.sqlite");
        let image_path = directory.path().join("images");
        std::fs::create_dir(&image_path).unwrap();
        for (path, rows) in [
            (&source_path, source_rows),
            (&destination_path, destination_rows),
        ] {
            let connection = Connection::open(path).unwrap();
            schema(&connection);
            if !rows.trim().is_empty() {
                connection.execute_batch(rows).unwrap();
            }
        }

        let image_dirs = vec![image_path.to_string_lossy().into_owned()];
        let state = Arc::new(
            AppState::from_databases(
                vec![
                    DbEntry {
                        id: "source".into(),
                        name: "Source".into(),
                        db_path: source_path.to_string_lossy().into_owned(),
                        image_dirs: image_dirs.clone(),
                        reject_archive: None,
                        remote_image_upload: Some(api_config(SOURCE_TOKEN)),
                    },
                    DbEntry {
                        id: "destination".into(),
                        name: "Destination".into(),
                        db_path: destination_path.to_string_lossy().into_owned(),
                        image_dirs,
                        reject_archive: None,
                        remote_image_upload: Some(api_config(DESTINATION_TOKEN)),
                    },
                ],
                directory
                    .path()
                    .join("cache")
                    .to_string_lossy()
                    .into_owned(),
                PregenerationConfig::default(),
            )
            .unwrap(),
        );
        Self {
            _directory: directory,
            router: app(Arc::clone(&state)),
            state,
            destination_path,
        }
    }

    /// Build a bundle on the source catalog, exactly as a remote client would.
    async fn export(&self, operation: &str) -> Value {
        let (status, exported) = call(
            self.router.clone(),
            "POST",
            "/api/sync/v1/exports",
            Some(SOURCE_TOKEN),
            Some(json!({
                "protocol_version": 1,
                "catalog_id": "source",
                "operation": operation,
                "reviewed_only": false
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{exported:#}");
        exported["data"]["bundle"].clone()
    }

    /// Upload a bundle against the destination catalog and return its ID.
    async fn preview(&self, operation: &str, bundle: Value) -> String {
        let (status, preview) = call(
            self.router.clone(),
            "POST",
            "/api/sync/v1/previews",
            Some(DESTINATION_TOKEN),
            Some(json!({
                "protocol_version": 1,
                "catalog_id": "destination",
                "operation": operation,
                "bundle": bundle
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{preview:#}");
        preview["data"]["preview_id"].as_str().unwrap().to_string()
    }

    async fn apply(&self, preview_id: &str) -> (StatusCode, Value) {
        call(
            self.router.clone(),
            "POST",
            &format!("/api/sync/v1/previews/{preview_id}/apply"),
            Some(DESTINATION_TOKEN),
            None,
        )
        .await
    }

    fn destination(&self) -> Connection {
        Connection::open(&self.destination_path).unwrap()
    }

    /// Every audit line written so far, oldest first.
    fn audit_entries(&self) -> Vec<Value> {
        let contents = std::fs::read_to_string(self.state.remote_audit.path()).unwrap_or_default();
        contents
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

#[tokio::test]
async fn token_scoped_export_previews_and_applies_to_the_selected_database() {
    let harness = Harness::new(
        "INSERT INTO project VALUES (1,'p','M42','remote settings',2,8,'project-guid');
         INSERT INTO target VALUES (1,'M42',1,5.5,-5.4,0,1,'target-guid');
         INSERT INTO exposuretemplate VALUES (1,'p','Ha 300','Ha',120,'template-guid');
         INSERT INTO exposureplan VALUES (1,'p',300,40,18,16,1,1,1,'plan-guid');
         INSERT INTO ruleweight VALUES (1,'Priority',2.5,1);",
        "INSERT INTO project VALUES (20,'p','M42','old settings',1,2,'project-guid');
         INSERT INTO target VALUES (30,'Old target',1,5.4,-5.3,0,20,'target-guid');
         INSERT INTO exposuretemplate VALUES (40,'p','Old Ha','Ha',100,'template-guid');
         INSERT INTO exposureplan VALUES (50,'p',180,20,7,6,0,30,40,'plan-guid');
         INSERT INTO ruleweight VALUES (60,'Priority',1.0,20);",
    );
    let destination_path = harness.destination_path.clone();
    let router = harness.router.clone();

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

/// Rows for a source catalog that has actually observed something.
const OBSERVED: &str = "INSERT INTO project VALUES (1,'p','M42','remote settings',2,8,'project-guid');
     INSERT INTO target VALUES (1,'M42',1,5.5,-5.4,0,1,'target-guid');
     INSERT INTO exposuretemplate VALUES (1,'p','Ha 300','Ha',120,'template-guid');
     INSERT INTO exposureplan VALUES (1,'p',300,40,18,16,1,1,1,'plan-guid');
     INSERT INTO ruleweight VALUES (1,'Priority',2.5,1);
     INSERT INTO acquiredimage VALUES (1,1,1,1750000000,'Ha',1,'{\"FileName\":\"one.fits\"}',NULL,'p',1,'image-one');
     INSERT INTO acquiredimage VALUES (2,1,1,1750000600,'Ha',2,'{\"FileName\":\"two.fits\"}','clouds','p',1,'image-two');";

#[tokio::test]
async fn merge_brings_a_remote_catalogs_projects_targets_and_captures_across() {
    let harness = Harness::new(OBSERVED, "");

    let bundle = harness.export("merge").await;
    // A merge has to carry the capture tables, not only the planning ones.
    for table in ["project", "target", "acquiredimage", "imagedata"] {
        assert!(
            bundle["tables"].get(table).is_some(),
            "merge bundle is missing {table}: {bundle:#}"
        );
    }
    let preview_id = harness.preview("merge", bundle).await;

    // Nothing lands until apply.
    assert_eq!(count(&harness.destination(), "acquiredimage"), 0);

    let (status, applied) = harness.apply(&preview_id).await;
    assert_eq!(status, StatusCode::OK, "{applied:#}");
    let destination = harness.destination();
    assert_eq!(count(&destination, "project"), 1);
    assert_eq!(count(&destination, "target"), 1);
    assert_eq!(count(&destination, "acquiredimage"), 2);
    let (grade, reason): (i64, Option<String>) = destination
        .query_row(
            "SELECT gradingStatus, rejectreason FROM acquiredimage WHERE guid = 'image-two'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(grade, 2, "a new image takes the remote catalog's grade");
    assert_eq!(reason.as_deref(), Some("clouds"));
}

#[tokio::test]
async fn a_second_merge_keeps_the_grade_the_local_reviewer_gave() {
    // The merge contract: the telescope owns structure, this end owns grading.
    // A re-merge must not undo a review done here.
    let harness = Harness::new(OBSERVED, "");
    let bundle = harness.export("merge").await;
    let preview_id = harness.preview("merge", bundle).await;
    let (status, applied) = harness.apply(&preview_id).await;
    assert_eq!(status, StatusCode::OK, "{applied:#}");

    harness
        .destination()
        .execute(
            "UPDATE acquiredimage SET gradingStatus = 2, rejectreason = 'reviewed here' \
             WHERE guid = 'image-one'",
            [],
        )
        .unwrap();

    let bundle = harness.export("merge").await;
    let preview_id = harness.preview("merge", bundle).await;
    let (status, applied) = harness.apply(&preview_id).await;
    assert_eq!(status, StatusCode::OK, "{applied:#}");

    let (grade, reason): (i64, Option<String>) = harness
        .destination()
        .query_row(
            "SELECT gradingStatus, rejectreason FROM acquiredimage WHERE guid = 'image-one'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(grade, 2, "the remote catalog still says Accepted");
    assert_eq!(reason.as_deref(), Some("reviewed here"));
}

#[tokio::test]
async fn push_grades_moves_grading_state_and_leaves_planning_alone() {
    let harness = Harness::new(
        OBSERVED,
        "INSERT INTO project VALUES (20,'p','M42','local settings',1,2,'project-guid');
         INSERT INTO target VALUES (30,'M42',1,5.5,-5.4,0,20,'target-guid');
         INSERT INTO exposuretemplate VALUES (40,'p','Ha 300','Ha',120,'template-guid');
         INSERT INTO exposureplan VALUES (50,'p',300,40,18,16,1,30,40,'plan-guid');
         INSERT INTO acquiredimage VALUES (60,20,30,1750000000,'Ha',0,'{\"FileName\":\"one.fits\"}',NULL,'p',50,'image-one');
         INSERT INTO acquiredimage VALUES (61,20,30,1750000600,'Ha',0,'{\"FileName\":\"two.fits\"}',NULL,'p',50,'image-two');",
    );

    let bundle = harness.export("push_grades").await;
    let preview_id = harness.preview("push_grades", bundle).await;
    let (status, applied) = harness.apply(&preview_id).await;
    assert_eq!(status, StatusCode::OK, "{applied:#}");
    assert_eq!(applied["data"]["summary"]["grades_changed"], 2);

    let destination = harness.destination();
    let mut statement = destination
        .prepare("SELECT guid, gradingStatus, rejectreason FROM acquiredimage ORDER BY guid")
        .unwrap();
    let grades: Vec<(String, i64, Option<String>)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        grades,
        vec![
            ("image-one".to_string(), 1, None),
            ("image-two".to_string(), 2, Some("clouds".to_string())),
        ]
    );
    // A grade push must not touch planning, however different the two ends are.
    let description: String = destination
        .query_row("SELECT description FROM project", [], |row| row.get(0))
        .unwrap();
    assert_eq!(description, "local settings");
}

#[tokio::test]
async fn a_destination_that_moved_refuses_the_apply_but_keeps_the_preview() {
    let harness = Harness::new(
        "INSERT INTO project VALUES (1,'p','M42','remote settings',2,8,'project-guid');
         INSERT INTO target VALUES (1,'M42',1,5.5,-5.4,0,1,'target-guid');
         INSERT INTO exposuretemplate VALUES (1,'p','Ha 300','Ha',120,'template-guid');
         INSERT INTO exposureplan VALUES (1,'p',300,40,18,16,1,1,1,'plan-guid');",
        "INSERT INTO project VALUES (20,'p','M42','old settings',1,2,'project-guid');
         INSERT INTO target VALUES (30,'M42',1,5.4,-5.3,0,20,'target-guid');
         INSERT INTO exposuretemplate VALUES (40,'p','Old Ha','Ha',100,'template-guid');
         INSERT INTO exposureplan VALUES (50,'p',180,20,7,6,0,30,40,'plan-guid');",
    );

    let bundle = harness.export("push_planning").await;
    let preview_id = harness.preview("push_planning", bundle).await;

    // Somebody edits the destination between preview and apply.
    harness
        .destination()
        .execute("UPDATE project SET priority = 9", [])
        .unwrap();

    let (status, refused) = harness.apply(&preview_id).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused:#}");
    let description: String = harness
        .destination()
        .query_row("SELECT description FROM project", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        description, "old settings",
        "a refused apply writes nothing"
    );

    // The point of the fix: the uploaded source data survives the refusal, so
    // the client re-reviews rather than re-uploading a whole bundle.
    let (status, kept) = call(
        harness.router.clone(),
        "GET",
        &format!("/api/sync/v1/previews/{preview_id}"),
        Some(DESTINATION_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{kept:#}");

    let (status, refreshed) = call(
        harness.router.clone(),
        "POST",
        &format!("/api/sync/v1/previews/{preview_id}/refresh"),
        Some(DESTINATION_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{refreshed:#}");
    assert_eq!(refreshed["data"]["preview_id"], preview_id);
    assert_eq!(refreshed["data"]["summary"]["project_updated"], 1);

    let (status, applied) = harness.apply(&preview_id).await;
    assert_eq!(status, StatusCode::OK, "{applied:#}");
    let description: String = harness
        .destination()
        .query_row("SELECT description FROM project", [], |row| row.get(0))
        .unwrap();
    assert_eq!(description, "remote settings");

    // Applying twice is still impossible: the successful apply consumed it.
    let (status, _) = harness.apply(&preview_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Every step of that is on the record, refusals included.
    let entries = harness.audit_entries();
    let applies: Vec<_> = entries
        .iter()
        .filter(|entry| entry["action"] == "apply")
        .map(|entry| entry["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(applies, vec!["refused", "ok", "refused"], "{entries:#?}");
    assert!(
        entries
            .iter()
            .any(|entry| entry["action"] == "preview_refresh" && entry["outcome"] == "ok"),
        "{entries:#?}"
    );
    let applied_ok = entries
        .iter()
        .find(|entry| entry["action"] == "apply" && entry["outcome"] == "ok")
        .unwrap();
    assert_eq!(applied_ok["catalog_id"], "destination");
    assert_eq!(applied_ok["operation"], "push_planning");
    assert_eq!(applied_ok["summary"]["total_updated"], 4);
}

#[tokio::test]
async fn a_rejected_token_is_recorded_without_naming_a_catalog() {
    let harness = Harness::new("", "");

    let (status, _) = call(
        harness.router.clone(),
        "GET",
        "/api/sync/v1/capabilities",
        Some("not-a-configured-token-000000000000"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let entries = harness.audit_entries();
    assert_eq!(entries.len(), 1, "{entries:#?}");
    assert_eq!(entries[0]["action"], "capabilities");
    assert_eq!(entries[0]["outcome"], "refused");
    assert_eq!(entries[0]["catalog_id"], "-");
    assert_eq!(entries[0]["detail"], "Invalid API token");
}

fn count(connection: &Connection, table: &str) -> i64 {
    connection
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
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
