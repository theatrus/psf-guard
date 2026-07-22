//! End-to-end tests for the create-and-import server flow:
//! `POST /api/databases/create` and `POST/GET /api/db/{db_id}/import`.
//!
//! Spins up an in-process axum app shaped like the production router,
//! generates small synthetic N.I.N.A.-style FITS files, and drives the whole
//! flow: create DB → background import job → poll progress → verify rows.

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use http_body_util::BodyExt;
use psf_guard::server::handlers;
use psf_guard::server::state::AppState;
use serde_json::Value;
use tempfile::tempdir;
use tower::ServiceExt;

fn build_app(state: Arc<AppState>) -> Router {
    use axum::routing::put;
    let db_routes: Router<Arc<AppState>> = Router::new()
        .route("/projects", get(handlers::list_projects))
        .route(
            "/projects/{project_id}",
            put(handlers::update_project_route),
        )
        .route(
            "/projects/{project_id}/merge",
            post(handlers::merge_project_route),
        )
        .route("/targets/{target_id}", put(handlers::update_target_route))
        .route(
            "/import",
            post(handlers::start_import_route).get(handlers::get_import_progress),
        )
        .route("/export", get(handlers::export_archive_route))
        .route("/export/local", post(handlers::export_local_route));

    Router::new()
        .route(
            "/api/databases",
            get(handlers::list_databases).post(handlers::add_database_route),
        )
        .route(
            "/api/databases/create",
            post(handlers::create_database_route),
        )
        .nest("/api/db/{db_id}", db_routes)
        .with_state(state)
}

/// Write one FITS card, padded to 80 bytes.
fn card(out: &mut Vec<u8>, text: &str) {
    let mut bytes = text.as_bytes().to_vec();
    assert!(bytes.len() <= 80, "card too long: {text}");
    bytes.resize(80, b' ');
    out.extend_from_slice(&bytes);
}

/// Minimal N.I.N.A.-flavored light frame: valid header + 10x10 zero payload.
fn write_fits(path: &std::path::Path, object: &str, filter: &str, date_obs: &str, ra: f64) {
    let mut header = Vec::new();
    card(&mut header, "SIMPLE  =                    T");
    card(&mut header, "BITPIX  =                   16");
    card(&mut header, "NAXIS   =                    2");
    card(&mut header, "NAXIS1  =                   10");
    card(&mut header, "NAXIS2  =                   10");
    card(&mut header, "IMAGETYP= 'LIGHT   '");
    card(&mut header, &format!("OBJECT  = '{object}'"));
    card(&mut header, &format!("FILTER  = '{filter}'"));
    card(&mut header, &format!("DATE-OBS= '{date_obs}'"));
    card(&mut header, "EXPTIME =                300.0");
    card(&mut header, "GAIN    =                  100");
    card(&mut header, "OFFSET  =                   30");
    card(&mut header, "XBINNING=                    1");
    card(&mut header, "YBINNING=                    1");
    card(&mut header, &format!("RA      = {ra:>20.6}"));
    card(&mut header, "DEC     =            41.268700");
    card(&mut header, "TELESCOP= 'TestScope'");
    card(&mut header, "INSTRUME= 'TestCam '");
    card(&mut header, "FOCALLEN=                518.0");
    card(&mut header, "END");
    header.resize(header.len().div_ceil(2880) * 2880, b' ');

    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(&header).unwrap();
    file.write_all(&[0u8; 2880]).unwrap(); // 10*10*2 bytes, padded
}

async fn json_request(
    app: Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let req = builder
        .body(match body {
            Some(v) => Body::from(serde_json::to_vec(&v).unwrap()),
            None => Body::empty(),
        })
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

/// Poll the import progress endpoint until the job finishes (or panic after
/// ~30s — synthetic imports complete in well under a second).
async fn wait_for_import(state: &Arc<AppState>, slug: &str) -> Value {
    for _ in 0..300 {
        let (status, body) = json_request(
            build_app(state.clone()),
            "GET",
            &format!("/api/db/{slug}/import"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let progress = &body["data"]["progress"];
        if progress["running"] == Value::Bool(false) && progress["stage"] != "" {
            return progress.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("import job did not finish in time");
}

fn state_with_management(dir: &std::path::Path) -> Arc<AppState> {
    let registry_path = dir.join("config.json");
    let cache_dir = dir.join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let state = Arc::new(
        AppState::from_databases(
            vec![],
            cache_dir.to_string_lossy().into_owned(),
            psf_guard::cli::PregenerationConfig::default(),
        )
        .unwrap(),
    );
    state.set_registry_path(Some(registry_path));
    state.set_allow_database_management(true);
    state
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_database_imports_fits_folders() {
    let dir = tempdir().unwrap();
    let images = dir.path().join("lights");
    std::fs::create_dir_all(&images).unwrap();
    write_fits(
        &images.join("m31_ha_0001.fits"),
        "M31",
        "Ha",
        "2026-01-15T04:00:00.000",
        10.6847,
    );
    write_fits(
        &images.join("m31_ha_0002.fits"),
        "M31",
        "Ha",
        "2026-01-15T04:05:10.000",
        10.6851,
    );
    write_fits(
        &images.join("m33_oiii_0001.fits"),
        "M33",
        "OIII",
        "2026-01-15T05:00:00.000",
        23.4621,
    );

    let state = state_with_management(dir.path());

    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases/create",
        Some(serde_json::json!({
            "name": "Imported Rig",
            "image_dirs": [images.to_string_lossy()],
            "backfill": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {body}");
    let slug = body["data"]["database"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["data"]["database"]["name"], "Imported Rig");
    // The managed DB file lives under <registry dir>/databases/.
    let db_path = body["data"]["database"]["database_path"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        db_path.contains("databases"),
        "unexpected db location: {db_path}"
    );

    let progress = wait_for_import(&state, &slug).await;
    assert_eq!(progress["stage"], "complete", "progress: {progress}");
    let outcome = &progress["outcome"];
    assert_eq!(outcome["imported"], 3);
    assert_eq!(outcome["projects_created"], 1);
    assert_eq!(outcome["targets_created"], 2);
    assert_eq!(outcome["dry_run"], false);

    // The DB is live in AppState: per-DB routes serve the imported project.
    let (status, body) = json_request(
        build_app(state.clone()),
        "GET",
        &format!("/api/db/{slug}/projects"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let projects = body["data"].as_array().unwrap();
    assert_eq!(projects.len(), 1, "projects: {body}");

    // And the file itself is a real v23 Target Scheduler database.
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 23);
    let pending: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM acquiredimage WHERE gradingStatus = 0 AND guid IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_into_existing_db_is_idempotent() {
    let dir = tempdir().unwrap();
    let images = dir.path().join("lights");
    std::fs::create_dir_all(&images).unwrap();
    write_fits(
        &images.join("m42_l_0001.fits"),
        "M42",
        "L",
        "2026-02-01T02:00:00.000",
        83.822,
    );

    let state = state_with_management(dir.path());

    // Create with one frame.
    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases/create",
        Some(serde_json::json!({
            "name": "Orion",
            "image_dirs": [images.to_string_lossy()],
            "backfill": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let slug = body["data"]["database"]["id"].as_str().unwrap().to_string();
    let progress = wait_for_import(&state, &slug).await;
    assert_eq!(progress["outcome"]["imported"], 1);

    // Add one new frame, then re-import the same directory.
    write_fits(
        &images.join("m42_l_0002.fits"),
        "M42",
        "L",
        "2026-02-01T02:06:00.000",
        83.823,
    );
    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        &format!("/api/db/{slug}/import"),
        Some(serde_json::json!({ "backfill": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "import failed: {body}");
    assert_eq!(body["data"]["started"], true);

    let progress = wait_for_import(&state, &slug).await;
    let outcome = &progress["outcome"];
    assert_eq!(outcome["scanned"], 2);
    assert_eq!(outcome["skipped_existing"], 1);
    assert_eq!(outcome["imported"], 1, "outcome: {outcome}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_dry_run_writes_nothing() {
    let dir = tempdir().unwrap();
    let images = dir.path().join("lights");
    std::fs::create_dir_all(&images).unwrap();
    write_fits(
        &images.join("m45_l_0001.fits"),
        "M45",
        "L",
        "2026-03-01T02:00:00.000",
        56.75,
    );

    let state = state_with_management(dir.path());
    let empty_dir = dir.path().join("empty");
    std::fs::create_dir_all(&empty_dir).unwrap();

    // Create an empty DB first (no frames imported from the empty dir).
    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases/create",
        Some(serde_json::json!({
            "name": "Dry",
            "image_dirs": [empty_dir.to_string_lossy()],
            "backfill": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let slug = body["data"]["database"]["id"].as_str().unwrap().to_string();
    let db_path = body["data"]["database"]["database_path"]
        .as_str()
        .unwrap()
        .to_string();
    wait_for_import(&state, &slug).await;

    // Dry-run import of the real folder.
    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        &format!("/api/db/{slug}/import"),
        Some(serde_json::json!({
            "image_dirs": [images.to_string_lossy()],
            "dry_run": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "import failed: {body}");
    let progress = wait_for_import(&state, &slug).await;
    let outcome = &progress["outcome"];
    assert_eq!(outcome["imported"], 1);
    assert_eq!(outcome["dry_run"], true);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM acquiredimage", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0, "dry run must not write");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_requires_management_flag() {
    let dir = tempdir().unwrap();
    let images = dir.path().join("lights");
    std::fs::create_dir_all(&images).unwrap();

    let state = state_with_management(dir.path());
    state.set_allow_database_management(false);

    let (status, _) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases/create",
        Some(serde_json::json!({
            "name": "Nope",
            "image_dirs": [images.to_string_lossy()],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_rejects_missing_image_dir() {
    let dir = tempdir().unwrap();
    let state = state_with_management(dir.path());

    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases/create",
        Some(serde_json::json!({
            "name": "Ghost",
            "image_dirs": [dir.path().join("does-not-exist").to_string_lossy()],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_import_is_refused_while_running() {
    let dir = tempdir().unwrap();
    let images = dir.path().join("lights");
    std::fs::create_dir_all(&images).unwrap();
    for i in 0..40 {
        write_fits(
            &images.join(format!("m1_l_{i:04}.fits")),
            "M1",
            "L",
            &format!("2026-03-01T02:{:02}:00.000", i % 60),
            83.633,
        );
    }

    let state = state_with_management(dir.path());
    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases/create",
        Some(serde_json::json!({
            "name": "Busy",
            "image_dirs": [images.to_string_lossy()],
            "backfill": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let slug = body["data"]["database"]["id"].as_str().unwrap().to_string();

    // Immediately try a second import; either the first is still running
    // (started=false) or it already finished (started=true). Both are valid —
    // what matters is that the DB never double-imports.
    let (status, _) = json_request(
        build_app(state.clone()),
        "POST",
        &format!("/api/db/{slug}/import"),
        Some(serde_json::json!({ "backfill": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    wait_for_import(&state, &slug).await;
    // Whatever interleaving happened, every frame exists exactly once.
    let (_, body) = json_request(
        build_app(state.clone()),
        "GET",
        &format!("/api/db/{slug}/import"),
        None,
    )
    .await;
    let db_path = {
        let (_, listing) =
            json_request(build_app(state.clone()), "GET", "/api/databases", None).await;
        listing["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == slug.as_str())
            .unwrap()["database_path"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let _ = body;
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM acquiredimage", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 40, "no duplicates from concurrent import attempts");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn organize_rename_move_and_merge() {
    let dir = tempdir().unwrap();
    let images = dir.path().join("lights");
    std::fs::create_dir_all(&images).unwrap();
    // Two sessions 60 days apart → two projects (same rig, same object name).
    write_fits(
        &images.join("veil_ha_0001.fits"),
        "Veil",
        "Ha",
        "2026-01-01T02:00:00.000",
        311.0,
    );
    write_fits(
        &images.join("veil_ha_0101.fits"),
        "Veil",
        "Ha",
        "2026-03-02T02:00:00.000",
        311.0,
    );

    let state = state_with_management(dir.path());
    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases/create",
        Some(serde_json::json!({
            "name": "Organize",
            "image_dirs": [images.to_string_lossy()],
            "backfill": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let slug = body["data"]["database"]["id"].as_str().unwrap().to_string();
    let db_path = body["data"]["database"]["database_path"]
        .as_str()
        .unwrap()
        .to_string();
    let progress = wait_for_import(&state, &slug).await;
    assert_eq!(progress["outcome"]["projects_created"], 2);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let mut ids = conn
        .prepare("SELECT Id FROM project ORDER BY Id")
        .unwrap()
        .query_map([], |row| row.get::<_, i32>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(ids.len(), 2);
    let (p1, p2) = (ids.remove(0), ids.remove(0));

    // Rename project 1.
    let (status, _) = json_request(
        build_app(state.clone()),
        "PUT",
        &format!("/api/db/{slug}/projects/{p1}"),
        Some(serde_json::json!({ "name": "Veil Campaign" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let name: String = conn
        .query_row("SELECT name FROM project WHERE Id = ?", [p1], |r| r.get(0))
        .unwrap();
    assert_eq!(name, "Veil Campaign");

    // Rename + move project-2's target into project 1; its image follows.
    let t2: i32 = conn
        .query_row("SELECT Id FROM target WHERE projectid = ?", [p2], |r| {
            r.get(0)
        })
        .unwrap();
    let (status, body) = json_request(
        build_app(state.clone()),
        "PUT",
        &format!("/api/db/{slug}/targets/{t2}"),
        Some(serde_json::json!({ "name": "Veil East", "project_id": p1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "move failed: {body}");
    assert_eq!(body["data"]["images_moved"], 1);
    let (tgt_project, tgt_name): (i32, String) = conn
        .query_row(
            "SELECT projectid, name FROM target WHERE Id = ?",
            [t2],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((tgt_project, tgt_name.as_str()), (p1, "Veil East"));
    let orphaned: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM acquiredimage WHERE projectId = ?",
            [p2],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphaned, 0, "images must follow the moved target");

    // Merge the (now empty) project 2 into project 1: it disappears.
    let (status, _) = json_request(
        build_app(state.clone()),
        "POST",
        &format!("/api/db/{slug}/projects/{p2}/merge"),
        Some(serde_json::json!({ "into_project_id": p1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let projects: i64 = conn
        .query_row("SELECT COUNT(*) FROM project", [], |r| r.get(0))
        .unwrap();
    assert_eq!(projects, 1);
    let stale_weights: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM ruleweight WHERE projectid = ?",
            [p2],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        stale_weights, 0,
        "merge must delete the source rule weights"
    );

    // Self-merge and merging into a missing project are rejected.
    let (status, _) = json_request(
        build_app(state.clone()),
        "POST",
        &format!("/api/db/{slug}/projects/{p1}/merge"),
        Some(serde_json::json!({ "into_project_id": p1 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = json_request(
        build_app(state.clone()),
        "PUT",
        &format!("/api/db/{slug}/targets/{t2}"),
        Some(serde_json::json!({ "project_id": 9999 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// Raw request helper for binary responses (the export zip).
async fn raw_request(app: Router, uri: &str) -> (StatusCode, Vec<u8>, String) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, bytes.to_vec(), content_type)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_streams_zip_of_non_rejected_lights() {
    let dir = tempdir().unwrap();
    let images = dir.path().join("lights");
    std::fs::create_dir_all(&images).unwrap();
    write_fits(
        &images.join("m81_l_0001.fits"),
        "M81",
        "L",
        "2026-04-01T02:00:00.000",
        148.888,
    );
    write_fits(
        &images.join("m81_l_0002.fits"),
        "M81",
        "L",
        "2026-04-01T02:06:00.000",
        148.889,
    );

    let state = state_with_management(dir.path());
    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases/create",
        Some(serde_json::json!({
            "name": "ExportMe",
            "image_dirs": [images.to_string_lossy()],
            "backfill": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let slug = body["data"]["database"]["id"].as_str().unwrap().to_string();
    let db_path = body["data"]["database"]["database_path"]
        .as_str()
        .unwrap()
        .to_string();
    wait_for_import(&state, &slug).await;

    // All frames are Pending: the accepted-only default has nothing to send.
    let (status, _, _) =
        raw_request(build_app(state.clone()), &format!("/api/db/{slug}/export")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Reject one frame, then export with pending included: only the
    // non-rejected frame is in the archive.
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE acquiredimage SET gradingStatus = 2 WHERE metadata LIKE '%m81_l_0002%'",
            [],
        )
        .unwrap();
    }
    let (status, bytes, content_type) = raw_request(
        build_app(state.clone()),
        &format!("/api/db/{slug}/export?include_pending=true"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "application/zip");
    assert_eq!(&bytes[..4], b"PK\x03\x04", "zip magic");
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(
        has(b"M81/LIGHT/L/m81_l_0001.fits"),
        "expected entry present"
    );
    assert!(
        !has(b"m81_l_0002.fits"),
        "rejected frame must not be exported"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_export_places_files_and_respects_management_gate() {
    let dir = tempdir().unwrap();
    let images = dir.path().join("lights");
    std::fs::create_dir_all(&images).unwrap();
    write_fits(
        &images.join("ic434_ha_0001.fits"),
        "IC434",
        "Ha",
        "2026-05-01T02:00:00.000",
        85.25,
    );

    let state = state_with_management(dir.path());
    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases/create",
        Some(serde_json::json!({
            "name": "LocalExport",
            "image_dirs": [images.to_string_lossy()],
            "backfill": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let slug = body["data"]["database"]["id"].as_str().unwrap().to_string();
    wait_for_import(&state, &slug).await;

    // Local export (pending included; link mode falls back gracefully).
    let dest = dir.path().join("takeout");
    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        &format!("/api/db/{slug}/export/local"),
        Some(serde_json::json!({
            "dest": dest.to_string_lossy(),
            "include_pending": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "local export failed: {body}");
    let summary = &body["data"];
    let placed = summary["copied"].as_u64().unwrap() + summary["linked"].as_u64().unwrap();
    assert_eq!(placed, 1, "summary: {summary}");
    assert!(dest.join("IC434/LIGHT/Ha/ic434_ha_0001.fits").is_file());

    // Second run is a no-op.
    let (_, body) = json_request(
        build_app(state.clone()),
        "POST",
        &format!("/api/db/{slug}/export/local"),
        Some(serde_json::json!({
            "dest": dest.to_string_lossy(),
            "include_pending": true,
        })),
    )
    .await;
    assert_eq!(body["data"]["skipped_existing"], 1);

    // Management gate: local export writes server-side files.
    state.set_allow_database_management(false);
    let (status, _) = json_request(
        build_app(state.clone()),
        "POST",
        &format!("/api/db/{slug}/export/local"),
        Some(serde_json::json!({ "dest": dest.to_string_lossy() })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reimport_attaches_to_existing_targets_after_preview() {
    let dir = tempdir().unwrap();
    let images = dir.path().join("lights");
    std::fs::create_dir_all(&images).unwrap();
    write_fits(
        &images.join("m31_ha_0001.fits"),
        "M31",
        "Ha",
        "2026-01-15T04:00:00.000",
        10.6847,
    );

    let state = state_with_management(dir.path());
    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases/create",
        Some(serde_json::json!({
            "name": "MergeSafe",
            "image_dirs": [images.to_string_lossy()],
            "backfill": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let slug = body["data"]["database"]["id"].as_str().unwrap().to_string();
    let db_path = body["data"]["database"]["database_path"]
        .as_str()
        .unwrap()
        .to_string();
    wait_for_import(&state, &slug).await;

    // A new night lands more M31 subs (different basenames, same object).
    write_fits(
        &images.join("m31_ha_0002.fits"),
        "M31",
        "Ha",
        "2026-01-16T04:00:00.000",
        10.6851,
    );

    // Step 1: PREVIEW (dry run) — reports the attach, writes nothing.
    let (status, _) = json_request(
        build_app(state.clone()),
        "POST",
        &format!("/api/db/{slug}/import"),
        Some(serde_json::json!({ "dry_run": true, "backfill": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let progress = wait_for_import(&state, &slug).await;
    let outcome = &progress["outcome"];
    assert_eq!(outcome["dry_run"], true);
    assert_eq!(outcome["attached"], 1, "outcome: {outcome}");
    assert_eq!(outcome["projects_created"], 0, "no duplicated structure");
    assert_eq!(outcome["attach_summaries"][0]["target"], "M31");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM acquiredimage", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "preview must not write");
    }

    // Step 2: confirmed live import — attaches to the existing target.
    let (status, _) = json_request(
        build_app(state.clone()),
        "POST",
        &format!("/api/db/{slug}/import"),
        Some(serde_json::json!({ "backfill": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let progress = wait_for_import(&state, &slug).await;
    assert_eq!(progress["outcome"]["attached"], 1);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let (projects, targets, images_n, distinct_targets): (i64, i64, i64, i64) = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM project), (SELECT COUNT(*) FROM target),
                    (SELECT COUNT(*) FROM acquiredimage),
                    (SELECT COUNT(DISTINCT targetId) FROM acquiredimage)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        (projects, targets, images_n, distinct_targets),
        (1, 1, 2, 1),
        "both frames on the one existing target"
    );
}
