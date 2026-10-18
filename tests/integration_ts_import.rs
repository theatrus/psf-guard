//! New imports own their database; legacy registrations keep direct access.
use axum::{
    body::Body,
    http::Request,
    routing::{get, post},
    Router,
};
use http_body_util::BodyExt;
use psf_guard::{
    cli::PregenerationConfig,
    db_registry::DbRegistry,
    server::{handlers, state::AppState, ts_sync},
};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

async fn call(app: Router, method: &str, path: &str, body: Value) -> (u16, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn import_owns_a_complete_snapshot_and_retains_a_writeback_connection() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("telescope.sqlite");
    let images = dir.path().join("images");
    std::fs::create_dir(&images).unwrap();
    psf_guard::ts_schema::create_fresh_db(&source).unwrap();
    let conn = Connection::open(&source).unwrap();
    conn.execute_batch("CREATE TABLE psf_guard_existing_note(value TEXT); INSERT INTO psf_guard_existing_note VALUES('keep my processing data')").unwrap();
    conn.execute_batch("INSERT INTO project(Id,profileId,name,guid) VALUES(1,'p','M42','project-guid');
        INSERT INTO target(Id,name,active,epochcode,projectid,guid) VALUES(1,'M42',1,0,1,'target-guid');
        INSERT INTO acquiredimage(Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,guid)
            VALUES(7,1,1,1000,'Ha',0,'{}','image-guid');").unwrap();
    drop(conn);
    let registry = dir.path().join("config.json");
    DbRegistry::default().save(&registry).unwrap();
    let state = Arc::new(
        AppState::from_databases(
            vec![],
            dir.path().join("cache").to_string_lossy().into_owned(),
            PregenerationConfig::default(),
        )
        .unwrap(),
    );
    state.set_registry_path(Some(registry.clone()));
    state.set_allow_database_management(true);
    let app = Router::new()
        .route("/import", post(ts_sync::import_route))
        .route("/jobs", get(ts_sync::status_route))
        .route(
            "/databases/{db_id}",
            axum::routing::put(handlers::update_database_route),
        )
        .route(
            "/databases/{db_id}/sync/preview",
            post(handlers::preview_sync_database_route),
        )
        .route(
            "/databases/{db_id}/sync/previews/{preview_id}/apply",
            post(handlers::apply_sync_database_preview_route),
        )
        .with_state(state.clone());
    let (status, response) = call(
        app.clone(),
        "POST",
        "/import",
        json!({"name":"Rig","db_path":source,"slug":"rig","image_dirs":[images]}),
    )
    .await;
    assert_eq!(status, 202, "{response}");
    let mut complete = None;
    for _ in 0..150 {
        let (_, jobs) = call(app.clone(), "GET", "/jobs", json!(null)).await;
        let job = &jobs["data"][0];
        if job["running"] == false {
            complete = Some(job.clone());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let job = complete.expect("the import finished");
    assert!(job["error"].is_null(), "{job}");
    let saved = DbRegistry::load_or_init(&registry).unwrap();
    let entry = saved.find("rig").unwrap();
    assert_ne!(std::path::Path::new(&entry.db_path), source);
    assert!(std::path::Path::new(&entry.db_path)
        .starts_with(std::fs::canonicalize(dir.path().join("databases")).unwrap()));
    let settings = entry.ts_sync.as_ref().unwrap();
    assert!(settings.writeback && settings.on_open && settings.on_change);
    assert_eq!(
        std::fs::canonicalize(&settings.source_path).unwrap(),
        std::fs::canonicalize(&source).unwrap()
    );
    let owned = Connection::open(&entry.db_path).unwrap();
    assert_eq!(
        owned
            .query_row("SELECT value FROM psf_guard_existing_note", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "keep my processing data"
    );
    let original = Connection::open(&source).unwrap();
    assert_eq!(
        original
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='psf_guard_ts_sync_grade'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    // Reopening the app imports arrivals and writes local grades through the
    // saved file connection, without registering the external TS as a catalog.
    owned
        .execute(
            "UPDATE acquiredimage SET gradingStatus=1 WHERE guid='image-guid'",
            [],
        )
        .unwrap();
    original.execute_batch("INSERT INTO acquiredimage(Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,guid)
        VALUES(8,1,1,2000,'Ha',0,'{}','arrival-guid')").unwrap();
    ts_sync::spawn(state.clone());
    let mut synced = false;
    for _ in 0..200 {
        let (_, jobs) = call(app.clone(), "GET", "/jobs", json!(null)).await;
        if let Some(job) = jobs["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|job| job["id"] == "rig" && job["running"] == false)
        {
            assert!(job["error"].is_null(), "{job}");
            assert_eq!(job["report"]["grades_written"], 1);
            synced = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(synced, "startup sync completed");
    assert_eq!(
        owned
            .query_row("SELECT count(*) FROM acquiredimage", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        original
            .query_row(
                "SELECT gradingStatus FROM acquiredimage WHERE guid='image-guid'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    let (status, preview) = call(
        app.clone(),
        "POST",
        "/databases/rig/sync/preview",
        json!({"peer_db_id":"ts:rig","kind":"pull","dry_run":true}),
    )
    .await;
    assert_eq!(status, 200, "{preview}");
    let mut disabled = settings.clone();
    disabled.writeback = false;
    let (status, response) = call(
        app.clone(),
        "PUT",
        "/databases/rig",
        json!({"ts_sync":disabled}),
    )
    .await;
    assert_eq!(status, 200, "{response}");
    owned
        .execute(
            "UPDATE acquiredimage SET gradingStatus=2 WHERE guid='image-guid'",
            [],
        )
        .unwrap();
    let (status, response) = call(
        app.clone(),
        "POST",
        "/databases/rig/sync/preview",
        json!({"peer_db_id":"ts:rig","kind":"push_grades","dry_run":true}),
    )
    .await;
    assert_eq!(status, 200, "{response}");
    assert_eq!(
        original
            .query_row(
                "SELECT gradingStatus FROM acquiredimage WHERE guid='image-guid'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1,
        "a preview cannot write grades"
    );
    let preview_id = response["data"]["preview_id"].as_str().unwrap();
    let (status, applied) = call(
        app.clone(),
        "POST",
        &format!("/databases/rig/sync/previews/{preview_id}/apply"),
        json!({}),
    )
    .await;
    assert_eq!(status, 200, "{applied}");
    assert_eq!(
        original
            .query_row(
                "SELECT gradingStatus FROM acquiredimage WHERE guid='image-guid'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2,
        "automatic writeback being off does not disable an explicit Apply"
    );
    disabled.source_path = "/tmp/different-source.sqlite".into();
    let (status, response) = call(app, "PUT", "/databases/rig", json!({"ts_sync":disabled})).await;
    assert_eq!(status, 400, "{response}");
    state.shutdown.begin();
}

#[tokio::test]
async fn legacy_registration_keeps_direct_target_scheduler_access() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("legacy.sqlite");
    psf_guard::ts_schema::create_fresh_db(&source).unwrap();
    let registry = dir.path().join("config.json");
    DbRegistry::default().save(&registry).unwrap();
    let state = Arc::new(
        AppState::from_databases(
            vec![],
            dir.path().join("cache").to_string_lossy().into_owned(),
            PregenerationConfig::default(),
        )
        .unwrap(),
    );
    state.set_registry_path(Some(registry.clone()));
    state.set_allow_database_management(true);
    let app = Router::new()
        .route("/databases", post(handlers::add_database_route))
        .with_state(state.clone());
    let (status, response) = call(
        app,
        "POST",
        "/databases",
        json!({"name":"Legacy","slug":"legacy","db_path":source,"image_dirs":[dir.path()]}),
    )
    .await;
    assert_eq!(status, 200, "{response}");
    let saved = DbRegistry::load_or_init(&registry).unwrap();
    let entry = saved.find("legacy").unwrap();
    assert_eq!(std::path::Path::new(&entry.db_path), source);
    assert!(entry.ts_sync.is_none());
    assert!(!dir.path().join("databases").exists());
    state.shutdown.begin();
}

#[tokio::test]
async fn importing_a_file_requires_database_management() {
    let dir = tempfile::tempdir().unwrap();
    let state = Arc::new(
        AppState::from_databases(
            vec![],
            dir.path().join("cache").to_string_lossy().into_owned(),
            PregenerationConfig::default(),
        )
        .unwrap(),
    );
    let app = Router::new()
        .route("/import", post(ts_sync::import_route))
        .with_state(state);
    let (status, _) = call(
        app,
        "POST",
        "/import",
        json!({"name":"Rig","db_path":"/tmp/not-a-live-catalog.sqlite"}),
    )
    .await;
    assert_ne!(status, 202);
}

async fn run_import_in_fixture(dir: &std::path::Path, body: Value) -> Value {
    let registry = dir.join("config.json");
    if !registry.exists() {
        DbRegistry::default().save(&registry).unwrap();
    }
    let state = Arc::new(
        AppState::from_databases(vec![], dir.join("cache"), PregenerationConfig::default())
            .unwrap(),
    );
    state.set_registry_path(Some(registry));
    state.set_allow_database_management(true);
    let app = Router::new()
        .route("/import", post(ts_sync::import_route))
        .route("/jobs", get(ts_sync::status_route))
        .with_state(state);
    let (status, response) = call(app.clone(), "POST", "/import", body).await;
    assert_eq!(status, 202, "{response}");
    for _ in 0..150 {
        let (_, jobs) = call(app.clone(), "GET", "/jobs", Value::Null).await;
        if let Some(job) = jobs["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"] == response["data"]["id"])
            && job["running"] == false
        {
            return job.clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("import did not finish");
}

#[tokio::test]
async fn standalone_import_keeps_psf_data_and_uses_an_advanced_destination() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("old-catalog.sqlite");
    let target = dir.path().join("restored-catalog.sqlite");
    let db = psf_guard::ts_schema::create_fresh_db(&source).unwrap();
    db.execute_batch("CREATE TABLE psf_guard_existing_note(value TEXT); INSERT INTO psf_guard_existing_note VALUES('my analysis');
        CREATE TABLE psf_guard_ts_sync_pending(guid TEXT,grade INTEGER,reason TEXT);
        INSERT INTO psf_guard_ts_sync_pending VALUES('old-capture',2,'saved review');").unwrap();
    drop(db);
    let before = std::fs::read(&source).unwrap();
    let job = run_import_in_fixture(dir.path(), json!({"name":"Restored","db_path":source,"destination_path":target,"image_dirs":[dir.path()],"connect_source":false,"slug":"restored"})).await;
    assert!(job["error"].is_null(), "{job}");
    assert_eq!(job["source_connected"], false);
    let registry = DbRegistry::load_or_init(&dir.path().join("config.json")).unwrap();
    let entry = registry.find("restored").unwrap();
    assert!(entry.ts_sync.is_none());
    assert_eq!(
        std::fs::canonicalize(&entry.db_path).unwrap(),
        std::fs::canonicalize(&target).unwrap()
    );
    let copied = Connection::open(target).unwrap();
    assert_eq!(
        copied
            .query_row("SELECT value FROM psf_guard_existing_note", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "my analysis"
    );
    assert_eq!(
        copied
            .query_row("SELECT reason FROM psf_guard_ts_sync_pending", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        "saved review"
    );
    assert_eq!(std::fs::read(&source).unwrap(), before);
}

#[tokio::test]
async fn advanced_import_destination_never_overwrites_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.sqlite");
    let target = dir.path().join("existing.sqlite");
    drop(psf_guard::ts_schema::create_fresh_db(&source).unwrap());
    std::fs::write(&target, b"existing catalog must survive").unwrap();
    let job = run_import_in_fixture(dir.path(), json!({"name":"Import","db_path":source,"destination_path":target,"image_dirs":[dir.path()]})).await;
    assert!(
        job["error"].as_str().unwrap().contains("already exists"),
        "{job}"
    );
    assert_eq!(
        std::fs::read(target).unwrap(),
        b"existing catalog must survive"
    );
    assert!(DbRegistry::load_or_init(&dir.path().join("config.json"))
        .unwrap()
        .databases
        .is_empty());
}
