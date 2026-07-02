//! Integration tests for the spatial-scan endpoints and the merge of scanned
//! spatial metrics into sequence analysis.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use rusqlite::Connection;
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

use psf_guard::server::spatial_scan::StoredSpatialMetrics;
use psf_guard::server::state::AppState;

// ---- Test harness (mirrors integration_sequence_analysis.rs) ----

fn create_test_schema(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE project (
            Id INTEGER PRIMARY KEY,
            profileId TEXT,
            name TEXT NOT NULL,
            description TEXT
        );
        CREATE TABLE target (
            Id INTEGER PRIMARY KEY,
            projectId INTEGER NOT NULL,
            name TEXT NOT NULL,
            active INTEGER NOT NULL DEFAULT 1,
            ra REAL,
            dec REAL
        );
        CREATE TABLE acquiredimage (
            Id INTEGER PRIMARY KEY,
            projectId INTEGER NOT NULL,
            targetId INTEGER NOT NULL,
            acquireddate INTEGER,
            filtername TEXT NOT NULL,
            gradingStatus INTEGER NOT NULL DEFAULT 0,
            metadata TEXT NOT NULL DEFAULT '{}',
            rejectreason TEXT,
            profileId TEXT
        );",
    )
    .unwrap();
}

fn seed_target_with_images(conn: &Connection, n: usize) {
    conn.execute(
        "INSERT INTO project (Id, profileId, name) VALUES (1, 'default', 'P')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO target (Id, projectId, name, active) VALUES (1, 1, 'NGC 6820', 1)",
        [],
    )
    .unwrap();

    let base_ts: i64 = 1705352400;
    for i in 0..n {
        let metadata = serde_json::json!({
            "FileName": format!("frame_{:04}.fits", i),
            "DetectedStars": 4500.0 - (i as f64) * 10.0,
            "HFR": 2.5,
        });
        conn.execute(
            "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, metadata)
             VALUES (?1, 1, 1, ?2, 'R', ?3)",
            rusqlite::params![i as i32 + 1, base_ts + (i as i64) * 300, metadata.to_string()],
        )
        .unwrap();
    }
}

/// Build the app; per-test temp cache dir so persisted spatial metrics never
/// leak between tests.
fn create_test_app(conn: Connection, cache_dir: &std::path::Path) -> (Router, Arc<AppState>) {
    use axum::routing::{get, post};
    use psf_guard::server::database_context::DatabaseContext;
    use psf_guard::server::handlers;

    let state = Arc::new(AppState::new_for_test(conn));
    {
        let mut dbs = state.databases.write().unwrap();
        let ctx = dbs.get("test").unwrap();
        let mut isolated: DatabaseContext = (**ctx).clone();
        isolated.cache_dir_path = cache_dir.to_path_buf();
        isolated.cache_dir = cache_dir.to_string_lossy().into_owned();
        dbs.insert("test".to_string(), Arc::new(isolated));
    }

    let db_routes: Router<Arc<AppState>> = Router::new()
        .route("/analysis/sequence", get(handlers::analyze_sequence))
        .route(
            "/analysis/spatial-scan",
            post(handlers::start_spatial_scan).get(handlers::get_spatial_scan_progress),
        );

    let app = Router::new()
        .nest("/api/db/{db_id}", db_routes)
        .with_state(state.clone());
    (app, state)
}

async fn get_json(app: Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}

async fn post_json(app: Router, uri: &str, body: &Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}

fn stored_entry(image_id: i32, filename: &str, dead: f64, bg_spread: f64) -> StoredSpatialMetrics {
    StoredSpatialMetrics {
        image_id,
        filename: filename.to_string(),
        star_count: 4000,
        avg_hfr: 2.5,
        dead_cell_fraction: Some(dead),
        star_uniformity: Some(0.7),
        bg_cell_spread: bg_spread,
        bg_cell_max_dev: bg_spread,
        median_adu: 1500.0,
        computed_at: 0,
    }
}

// ---- Tests ----

#[tokio::test]
async fn spatial_scan_progress_idle_by_default() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    let tmp = tempfile::tempdir().unwrap();
    let (app, _state) = create_test_app(conn, tmp.path());

    let (status, json) = get_json(app, "/api/db/test/analysis/spatial-scan").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["started"], false);
    assert_eq!(json["data"]["progress"]["running"], false);
    assert_eq!(json["data"]["cached_count"], 0);
}

#[tokio::test]
async fn spatial_scan_runs_and_reports_missing_files_as_errors() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    seed_target_with_images(&conn, 4);
    let tmp = tempfile::tempdir().unwrap();
    let (app, state) = create_test_app(conn, tmp.path());

    let (status, json) = post_json(
        app.clone(),
        "/api/db/test/analysis/spatial-scan",
        &serde_json::json!({"target_id": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["started"], true);
    assert_eq!(json["data"]["progress"]["total"], 4);

    // The test context has no image dirs, so every file resolution fails and
    // the scan finishes quickly. Poll until done.
    let ctx = state.get_database("test").unwrap();
    for _ in 0..100 {
        {
            let s = ctx.spatial_metrics.read().unwrap();
            if !s.progress.running {
                break;
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }

    let (status, json) = get_json(app, "/api/db/test/analysis/spatial-scan").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["progress"]["running"], false);
    assert_eq!(json["data"]["progress"]["processed"], 4);
    assert_eq!(json["data"]["progress"]["errors"], 4);
    assert!(json["data"]["progress"]["finished_at"].is_i64());
}

#[tokio::test]
async fn scan_start_rejects_unknown_target() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    let tmp = tempfile::tempdir().unwrap();
    let (app, _state) = create_test_app(conn, tmp.path());

    let (status, _json) = post_json(
        app,
        "/api/db/test/analysis/spatial-scan",
        &serde_json::json!({"target_id": 99}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn scanned_metrics_merge_into_sequence_analysis() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    seed_target_with_images(&conn, 10);
    let tmp = tempfile::tempdir().unwrap();
    let (app, state) = create_test_app(conn, tmp.path());

    // Simulate a completed scan: clean frames except image 8, which has a
    // heavily occluded frame (40% dead cells) that DB metadata alone would
    // never reveal (its star count is normal).
    {
        let ctx = state.get_database("test").unwrap();
        let mut store = ctx.spatial_metrics.write().unwrap();
        for i in 0..10i32 {
            let dead = if i == 7 { 0.40 } else { 0.02 };
            let entry = stored_entry(i + 1, &format!("frame_{:04}.fits", i), dead, 0.05);
            store.metrics.insert(i + 1, entry);
        }
    }

    let (status, json) = get_json(app, "/api/db/test/analysis/sequence?target_id=1").await;
    assert_eq!(status, StatusCode::OK, "body: {}", json);
    let sequences = json["data"]["sequences"].as_array().unwrap();
    assert_eq!(sequences.len(), 1);
    let images = sequences[0]["images"].as_array().unwrap();

    let occluded = images
        .iter()
        .find(|img| img["image_id"] == 8)
        .expect("image 8 present");
    assert_eq!(
        occluded["category"], "possible_obstruction",
        "occluded frame should be classified from scanned metrics: {}",
        occluded
    );
    let coverage = occluded["normalized_metrics"]["spatial_coverage"]
        .as_f64()
        .expect("spatial_coverage populated from scan store");
    assert!(coverage < 0.5, "occluded frame coverage: {}", coverage);

    let clean = images.iter().find(|img| img["image_id"] == 3).unwrap();
    assert!(
        clean["normalized_metrics"]["spatial_coverage"]
            .as_f64()
            .unwrap()
            > 0.9
    );
    assert!(clean["category"].is_null());
}

#[tokio::test]
async fn stale_filename_entries_are_ignored() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    seed_target_with_images(&conn, 5);
    let tmp = tempfile::tempdir().unwrap();
    let (app, state) = create_test_app(conn, tmp.path());

    // Entry recorded under a filename that no longer matches the metadata.
    {
        let ctx = state.get_database("test").unwrap();
        let mut store = ctx.spatial_metrics.write().unwrap();
        store
            .metrics
            .insert(1, stored_entry(1, "old_name.fits", 0.9, 0.5));
    }

    let (status, json) = get_json(app, "/api/db/test/analysis/sequence?target_id=1").await;
    assert_eq!(status, StatusCode::OK);
    let images = json["data"]["sequences"][0]["images"].as_array().unwrap();
    let img1 = images.iter().find(|img| img["image_id"] == 1).unwrap();
    assert!(
        img1["normalized_metrics"]["spatial_coverage"].is_null(),
        "stale entry must not be merged: {}",
        img1
    );
}
