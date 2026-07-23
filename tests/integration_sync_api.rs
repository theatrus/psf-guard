//! Management API coverage for safe scheduler database sync directions.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::Router;
use http_body_util::BodyExt;
use psf_guard::cli::PregenerationConfig;
use psf_guard::db_registry::DbEntry;
use psf_guard::server::handlers;
use psf_guard::server::state::AppState;
use rusqlite::Connection;
use serde_json::{json, Value};
use tempfile::tempdir;
use tower::ServiceExt;

fn schema(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE project (Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT, description TEXT, state INTEGER, priority INTEGER, guid TEXT);
         CREATE TABLE target (Id INTEGER PRIMARY KEY, name TEXT, active INTEGER, ra REAL, dec REAL, epochcode INTEGER, projectid INTEGER, guid TEXT);
         CREATE TABLE exposuretemplate (Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT, filtername TEXT, gain INTEGER, guid TEXT);
         CREATE TABLE exposureplan (Id INTEGER PRIMARY KEY, profileId TEXT, exposure REAL, desired INTEGER, acquired INTEGER, accepted INTEGER, enabled INTEGER, targetid INTEGER, exposureTemplateId INTEGER, guid TEXT);
         CREATE TABLE acquiredimage (Id INTEGER PRIMARY KEY, projectId INTEGER, targetId INTEGER, acquireddate INTEGER, filtername TEXT, gradingStatus INTEGER NOT NULL, metadata TEXT NOT NULL, rejectreason TEXT, profileId TEXT, exposureId INTEGER, guid TEXT);
         CREATE TABLE ruleweight (Id INTEGER PRIMARY KEY, name TEXT, weight REAL, projectid INTEGER);
         CREATE TABLE imagedata (Id INTEGER PRIMARY KEY, tag TEXT, imagedata BLOB, acquiredimageid INTEGER, width INTEGER, height INTEGER);",
    )
    .unwrap();
}

async fn request(app: Router, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::post("/api/databases/local/sync")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn planning_push_previews_then_keeps_telescope_capture_state() {
    let dir = tempdir().unwrap();
    let local_path = dir.path().join("local.sqlite");
    let telescope_path = dir.path().join("telescope.sqlite");
    let local = Connection::open(&local_path).unwrap();
    let telescope = Connection::open(&telescope_path).unwrap();
    schema(&local);
    schema(&telescope);
    local
        .execute_batch(
            "INSERT INTO project VALUES (1,'p','M42','new settings',2,8,'project-guid');
         INSERT INTO target VALUES (1,'M42',1,5.5,-5.4,0,1,'target-guid');
         INSERT INTO exposuretemplate VALUES (1,'p','Ha 300','Ha',120,'template-guid');
         INSERT INTO exposureplan VALUES (1,'p',300,40,18,16,1,1,1,'plan-guid');
         INSERT INTO ruleweight VALUES (1,'Priority',2.5,1);",
        )
        .unwrap();
    telescope
        .execute_batch(
            "INSERT INTO project VALUES (20,'p','M42','old settings',1,2,'project-guid');
         INSERT INTO target VALUES (30,'Old target',1,5.4,-5.3,0,20,'target-guid');
         INSERT INTO exposuretemplate VALUES (40,'p','Old Ha','Ha',100,'template-guid');
         INSERT INTO exposureplan VALUES (50,'p',180,20,7,6,0,30,40,'plan-guid');
         INSERT INTO ruleweight VALUES (60,'Priority',1.0,20);
         INSERT INTO acquiredimage VALUES (70,20,30,1000,'Ha',2,'{}','cloud','p',50,'image-guid');",
        )
        .unwrap();
    drop(local);
    drop(telescope);

    let image_dir = dir.path().join("images");
    std::fs::create_dir(&image_dir).unwrap();
    let entries = vec![
        DbEntry {
            id: "local".into(),
            name: "Review copy".into(),
            db_path: local_path.to_string_lossy().into_owned(),
            image_dirs: vec![image_dir.to_string_lossy().into_owned()],
            reject_archive: None,
        },
        DbEntry {
            id: "scope".into(),
            name: "Telescope".into(),
            db_path: telescope_path.to_string_lossy().into_owned(),
            image_dirs: vec![image_dir.to_string_lossy().into_owned()],
            reject_archive: None,
        },
    ];
    let state = Arc::new(
        AppState::from_databases(
            entries,
            dir.path().join("cache").to_string_lossy().into_owned(),
            PregenerationConfig::default(),
        )
        .unwrap(),
    );
    state.set_allow_database_management(true);
    let app = Router::new()
        .route(
            "/api/databases/{db_id}/sync",
            post(handlers::sync_database_route),
        )
        .with_state(state);

    let payload = json!({
        "peer_db_id": "scope",
        "kind": "push_planning",
        "dry_run": true
    });
    let (status, preview) = request(app.clone(), payload).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["data"]["project"]["updated"], 1);
    let telescope = Connection::open(&telescope_path).unwrap();
    assert_eq!(
        telescope
            .query_row("SELECT description FROM project", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "old settings"
    );
    drop(telescope);

    let (status, applied) = request(
        app,
        json!({
            "peer_db_id": "scope",
            "kind": "push_planning",
            "dry_run": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(applied["data"]["total_updated"], 5);

    let telescope = Connection::open(&telescope_path).unwrap();
    assert_eq!(
        telescope
            .query_row("SELECT description FROM project", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "new settings"
    );
    assert_eq!(
        telescope
            .query_row("SELECT acquired FROM exposureplan", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        telescope
            .query_row("SELECT accepted FROM exposureplan", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        6
    );
    assert_eq!(
        telescope
            .query_row("SELECT gradingStatus FROM acquiredimage", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}
