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
        "INSERT INTO target (Id, projectId, name, active, ra, dec)
         VALUES (1, 1, 'NGC 6820', 1, 5.0, 10.0)",
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
        isolated.use_storage_for_test(cache_dir);
        dbs.insert("test".to_string(), Arc::new(isolated));
    }

    let db_routes: Router<Arc<AppState>> = Router::new()
        .route("/images", get(handlers::get_images))
        .route("/analysis/sequence", get(handlers::analyze_sequence))
        .route(
            "/analysis/spatial-scan",
            post(handlers::start_spatial_scan).get(handlers::get_spatial_scan_progress),
        )
        .route(
            "/analysis/quality-backfill",
            post(handlers::start_quality_backfill_route)
                .get(handlers::get_quality_backfill_progress),
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
    let cells = 8 * 6;
    let dead_cells = (dead * cells as f64).round() as usize;
    let star_dead_cells: Vec<bool> = (0..cells).map(|cell| cell < dead_cells).collect();
    StoredSpatialMetrics {
        image_id,
        filename: filename.to_string(),
        source_revision: None,
        detector: psf_guard::server::spatial_scan::QUALITY_DETECTOR.to_string(),
        detector_version: psf_guard::server::spatial_scan::QUALITY_DETECTOR_VERSION,
        star_count: 4000,
        avg_hfr: 2.5,
        dead_cell_fraction: Some(dead),
        star_uniformity: Some(0.7),
        bg_cell_spread: bg_spread,
        bg_cell_max_dev: bg_spread,
        median_adu: 1500.0,
        sky_noise_adu: None,
        zero_point: None,
        zero_point_version: 0,
        computed_at: 0,
        catalog: psf_guard::photometry::FrameCatalog::default(),
        star_cell_counts: star_dead_cells
            .iter()
            .map(|&flagged| if flagged { 0.0 } else { 100.0 })
            .collect(),
        star_dead_cells,
        bg_cell_medians: vec![1500.0; cells],
        grid_cols: 8,
        grid_rows: 6,
        width: 800,
        height: 600,
        exposure_s: None,
        bg_glow_max: 0.0,
        bg_glow_cells: vec![],
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
    assert!(json["data"].get("scope").is_none());
}

#[tokio::test]
async fn scoped_status_reports_new_and_outdated_quality_work() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    seed_target_with_images(&conn, 3);
    let tmp = tempfile::tempdir().unwrap();
    let (app, state) = create_test_app(conn, tmp.path());

    let uri = "/api/db/test/analysis/spatial-scan?target_id=1&filter_name=R";
    let (status, json) = get_json(app.clone(), uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["scope"]["total_frames"], 3);
    assert_eq!(json["data"]["scope"]["pending_frames"], 3);
    assert_eq!(json["data"]["scope"]["new_frames"], 3);
    assert_eq!(json["data"]["scope"]["outdated_frames"], 0);
    assert_eq!(json["data"]["scope"]["needs_analysis"], true);

    {
        let ctx = state.get_database("test").unwrap();
        let mut store = ctx.spatial_metrics.write().unwrap();
        for i in 0..3i32 {
            store.metrics.insert(
                i + 1,
                stored_entry(i + 1, &format!("frame_{i:04}.fits"), 0.02, 0.05),
            );
        }
    }

    let (_, json) = get_json(app.clone(), uri).await;
    assert_eq!(json["data"]["scope"]["pending_frames"], 0);
    assert_eq!(json["data"]["scope"]["needs_analysis"], false);

    {
        let ctx = state.get_database("test").unwrap();
        let mut store = ctx.spatial_metrics.write().unwrap();
        store.metrics.get_mut(&2).unwrap().detector_version =
            psf_guard::server::spatial_scan::QUALITY_DETECTOR_VERSION.saturating_sub(1);
    }

    let (_, json) = get_json(app, uri).await;
    assert_eq!(json["data"]["scope"]["pending_frames"], 1);
    assert_eq!(json["data"]["scope"]["new_frames"], 0);
    assert_eq!(json["data"]["scope"]["outdated_frames"], 1);
    assert_eq!(json["data"]["scope"]["needs_analysis"], true);
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

/// While a person's work runs, the database-wide analysis is listed as
/// waiting for it, by name, with the progress it had; it goes on after.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn database_quality_backfill_says_it_waits_for_a_persons_work() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    seed_target_with_images(&conn, 2);
    let tmp = tempfile::tempdir().unwrap();
    let images = tmp.path().join("images");
    std::fs::create_dir_all(&images).unwrap();
    for index in 0..2 {
        // Found, so the scan reaches its frames; not a FITS, so each fails
        // fast once it may read them.
        std::fs::write(images.join(format!("frame_{index:04}.fits")), b"not a fits").unwrap();
    }
    let (app, state) = create_test_app(conn, tmp.path());
    {
        let mut dbs = state.databases.write().unwrap();
        let mut ctx = (**dbs.get("test").unwrap()).clone();
        ctx.image_dirs = vec![images.to_string_lossy().into_owned()];
        ctx.image_dir_paths = vec![images.clone()];
        dbs.insert("test".to_string(), Arc::new(ctx));
    }

    let person = state.begin_named_interactive_job("a stack build");
    let (status, json) = post_json(
        app.clone(),
        "/api/db/test/analysis/quality-backfill",
        &serde_json::json!({"force": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    assert_eq!(json["data"]["started"], true);

    let mut scan = Value::Null;
    for _ in 0..250 {
        let (_, json) = get_json(app.clone(), "/api/db/test/analysis/spatial-scan").await;
        scan = json["data"]["progress"].clone();
        if scan["waiting_for"].is_string() {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }
    assert_eq!(scan["running"], true, "{scan}");
    assert_eq!(scan["automatic"], true, "{scan}");
    assert_eq!(scan["waiting_for"], "a stack build", "{scan}");
    let (_, backfill) = get_json(app.clone(), "/api/db/test/analysis/quality-backfill").await;
    assert_eq!(backfill["data"]["progress"]["running"], true);

    drop(person);
    let ctx = state.get_database("test").unwrap();
    for _ in 0..250 {
        if !ctx.quality_backfill.read().unwrap().progress.running {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }
    let (_, json) = get_json(app.clone(), "/api/db/test/analysis/spatial-scan").await;
    let scan = &json["data"]["progress"];
    assert_eq!(scan["running"], false, "{scan}");
    assert!(scan["waiting_for"].is_null(), "{scan}");
    let (_, backfill) = get_json(app, "/api/db/test/analysis/quality-backfill").await;
    assert_eq!(backfill["data"]["progress"]["processed_targets"], 1);
}

#[tokio::test]
async fn database_quality_backfill_runs_as_a_separate_job() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    seed_target_with_images(&conn, 2);
    let tmp = tempfile::tempdir().unwrap();
    let (app, state) = create_test_app(conn, tmp.path());

    let (status, json) = post_json(
        app.clone(),
        "/api/db/test/analysis/quality-backfill",
        &serde_json::json!({"force": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    assert_eq!(json["data"]["started"], true);
    assert_eq!(json["data"]["progress"]["total_targets"], 1);
    assert_eq!(json["data"]["progress"]["force"], true);

    let ctx = state.get_database("test").unwrap();
    for _ in 0..100 {
        if !ctx.quality_backfill.read().unwrap().progress.running {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }

    let (status, json) = get_json(app, "/api/db/test/analysis/quality-backfill").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["progress"]["running"], false);
    assert_eq!(json["data"]["progress"]["processed_targets"], 1);
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

    // Simulate a completed scan: clean frames except images 8-9, which show
    // a persistent occlusion (40%/35% dead cells) that DB metadata alone
    // would never reveal (star counts are normal). Two consecutive elevated
    // frames are required: single-frame blips are deliberately not
    // classified (neighbor-corroboration rule).
    {
        let ctx = state.get_database("test").unwrap();
        let mut store = ctx.spatial_metrics.write().unwrap();
        for i in 0..10i32 {
            let dead = match i {
                7 => 0.40,
                8 => 0.35,
                _ => 0.02,
            };
            let entry = stored_entry(i + 1, &format!("frame_{:04}.fits", i), dead, 0.05);
            store.metrics.insert(i + 1, entry);
        }
    }

    let (status, json) = get_json(app, "/api/db/test/analysis/sequence?target_id=1").await;
    assert_eq!(status, StatusCode::OK, "body: {}", json);
    let sequences = json["data"]["sequences"].as_array().unwrap();
    assert_eq!(sequences.len(), 1);
    assert_eq!(
        sequences[0]["reference_values"]["best_star_count"], 4000.0,
        "quality cache should replace stale N.I.N.A. star counts"
    );
    assert_eq!(
        sequences[0]["reference_values"]["best_hfr"], 2.5,
        "quality cache should supply freshly measured HFR"
    );
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
    let overlay = &occluded["spatial_overlay"];
    assert_eq!(overlay["grid_cols"], 8);
    assert_eq!(overlay["grid_rows"], 6);
    assert!(
        overlay["low_star_cells"]
            .as_array()
            .expect("localized obstruction should expose measured cells")
            .iter()
            .filter(|flagged| flagged.as_bool() == Some(true))
            .count()
            >= 16
    );

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
async fn legacy_detector_cache_keeps_scheduler_star_metrics() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    seed_target_with_images(&conn, 10);
    let tmp = tempfile::tempdir().unwrap();
    let (app, state) = create_test_app(conn, tmp.path());

    {
        let ctx = state.get_database("test").unwrap();
        let mut store = ctx.spatial_metrics.write().unwrap();
        for i in 0..10i32 {
            let dead = if matches!(i, 7 | 8) { 0.40 } else { 0.02 };
            let mut entry = stored_entry(i + 1, &format!("frame_{:04}.fits", i), dead, 0.05);
            entry.detector.clear();
            entry.detector_version = 0;
            entry.star_count = 1;
            entry.avg_hfr = 99.0;
            store.metrics.insert(i + 1, entry);
        }
    }

    let (status, json) = get_json(app, "/api/db/test/analysis/sequence?target_id=1").await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    let sequence = &json["data"]["sequences"][0];
    assert_eq!(sequence["reference_values"]["best_star_count"], 4500.0);
    assert_eq!(sequence["reference_values"]["best_hfr"], 2.5);
    assert_eq!(
        sequence["images"]
            .as_array()
            .unwrap()
            .iter()
            .find(|image| image["image_id"] == 8)
            .unwrap()["category"],
        "possible_obstruction",
        "legacy cache should still supply spatial obstruction evidence"
    );
}

#[tokio::test]
async fn cached_astrometry_reduces_score_and_marks_regrade_reason() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    seed_target_with_images(&conn, 6);
    let tmp = tempfile::tempdir().unwrap();
    let astrometry_dir = tmp.path().join("astrometry");
    std::fs::create_dir_all(&astrometry_dir).unwrap();
    let source_path = tmp.path().join("frame_0003.fits");
    std::fs::write(&source_path, b"x").unwrap();
    let source_path = source_path.canonicalize().unwrap();
    let source_metadata = std::fs::metadata(&source_path).unwrap();
    let source_modified = source_metadata
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let analysis = serde_json::json!({
        "image_id": 4,
        "status": "solved",
        "mode": "hinted",
        "expected_source": {"ra_deg": 75.0, "dec_deg": 10.0, "source": "target_scheduler"},
        "solution": {
            "center_ra_deg": 75.2,
            "center_dec_deg": 10.0,
            "pixel_scale_arcsec_per_pixel": 1.0,
            "matched_stars": 30,
            "rms_arcsec": 0.8,
            "image_width": 1000,
            "image_height": 800,
            "wcs": {
                "crval": [75.2, 10.0], "crpix": [500.0, 400.0],
                "cd": [[-0.0002777778, 0.0], [0.0, 0.0002777778]],
                "ctype": ["RA---TAN", "DEC--TAN"], "cunit": ["deg", "deg"],
                "radesys": "ICRS", "equinox": 2000.0
            },
            "footprint": [], "objects": []
        },
        "catalog_hits": [],
        "pointing": {
            "expected_ra_deg": 75.0, "expected_dec_deg": 10.0,
            "east_offset_arcsec": 709.0, "north_offset_arcsec": 0.0,
            "separation_arcsec": 709.0, "target_in_frame": false,
            "target_edge_margin_px": -120.0
        },
        "source_fingerprint": {
            "canonical_path": source_path.to_string_lossy(),
            "size_bytes": source_metadata.len(),
            "modified_unix_seconds": source_modified.as_secs(),
            "modified_subsec_nanos": source_modified.subsec_nanos()
        },
        "solver_provenance": {
            "seiza_version": "0.12.2", "seiza_fits_version": "0.2.1",
            "detection_backend": "mtf_u8",
            "star_catalog": {
                "name": "stars", "path": "/data/stars.bin", "format": "test",
                "size_bytes": 1, "modified_unix_seconds": 1
            }
        },
        "solve_attempt": {
            "outcome": "solved", "modes_attempted": ["hinted"],
            "detected_stars": 100, "duration_ms": 10,
            "image_quality_evidence": true, "cacheable": true
        },
        "computed_at": 1
    });
    std::fs::write(
        astrometry_dir.join("4.json"),
        serde_json::to_vec(&analysis).unwrap(),
    )
    .unwrap();

    let (app, _state) = create_test_app(conn, tmp.path());
    let (status, json) = get_json(app.clone(), "/api/db/test/analysis/sequence?target_id=1").await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    let image = &json["data"]["sequences"][0]["images"][3];
    assert_eq!(image["category"], "off_target");
    assert!(image["quality_score"].as_f64().unwrap() <= 0.20);
    assert!(image["regrade_reason"]
        .as_str()
        .is_some_and(|reason| reason.contains("Off target")));

    // A same-name replacement must invalidate the solve evidence rather than
    // grading a different exposure from stale WCS.
    std::fs::write(&source_path, b"replacement").unwrap();
    let (status, json) = get_json(app, "/api/db/test/analysis/sequence?target_id=1").await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    let image = &json["data"]["sequences"][0]["images"][3];
    assert!(image["pointing"].is_null());
    assert_ne!(image["category"], "off_target");
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

/// The image list tells the grid each frame's size, so its thumbnail takes
/// the frame's shape. Frames of one camera setup share a size, read once
/// from the header of the first that can be read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_image_list_carries_each_frames_size_from_one_header() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    seed_target_with_images(&conn, 3);
    let tmp = tempfile::tempdir().unwrap();
    let images = tmp.path().join("images");
    std::fs::create_dir_all(&images).unwrap();
    // The first frame cannot be read; the second answers for all three.
    std::fs::write(images.join("frame_0000.fits"), b"not a fits").unwrap();
    let cards = [
        "SIMPLE  =                    T".to_string(),
        format!("BITPIX  = {:>20}", 16),
        "NAXIS   =                    2".to_string(),
        format!("NAXIS1  = {:>20}", 3200),
        format!("NAXIS2  = {:>20}", 3200),
        "END".to_string(),
    ];
    let mut header: Vec<u8> = cards
        .iter()
        .flat_map(|card| format!("{card:<80}").into_bytes())
        .collect();
    header.resize(2880, b' ');
    std::fs::write(images.join("frame_0001.fits"), header).unwrap();

    let (app, state) = create_test_app(conn, tmp.path());
    {
        let mut dbs = state.databases.write().unwrap();
        let mut ctx = (**dbs.get("test").unwrap()).clone();
        ctx.image_dirs = vec![images.to_string_lossy().into_owned()];
        ctx.image_dir_paths = vec![images.clone()];
        dbs.insert("test".to_string(), Arc::new(ctx));
    }

    let (status, json) =
        get_json(app, "/api/db/test/images?project_id=1&target_id=1&limit=10").await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    let listed = json["data"].as_array().unwrap();
    assert_eq!(listed.len(), 3);
    for image in listed {
        assert_eq!(
            image["frame_size"],
            serde_json::json!({ "width": 3200, "height": 3200 }),
            "{image}"
        );
    }
}

/// The image list names each light's night, split at its profile's rig's
/// local noon, not where the catalog's quiet hours fall: 19:00 UTC at a
/// site at 121° W is late morning there, still the night before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_image_list_names_each_lights_night_at_its_rigs_local_noon() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    seed_target_with_images(&conn, 1);
    // 2024-01-16 19:00 UTC, the next day's late morning at the site.
    conn.execute(
        "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, metadata)
         VALUES (2, 1, 1, 1705431600, 'R', '{\"FileName\":\"frame_0001.fits\"}')",
        [],
    )
    .unwrap();
    let light = psf_guard::commands::import::headers::FrameMeta {
        camera: Some("ASI2600MM".into()),
        telescope: Some("C925".into()),
        width: Some(6248),
        height: Some(4176),
        timestamp: Some(1705352400),
        site_longitude: Some(-121.163),
        site_latitude: Some(38.66),
        ..Default::default()
    };
    psf_guard::calibration::note_light_rig(&conn, Some("default"), &light).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let (app, _state) = create_test_app(conn, tmp.path());
    let (status, json) =
        get_json(app, "/api/db/test/images?project_id=1&target_id=1&limit=10").await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    let mut nights: Vec<(i64, String)> = json["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|image| {
            (
                image["id"].as_i64().unwrap(),
                image["night"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    nights.sort();
    // 2024-01-15 21:00 UTC is that evening; 2024-01-16 19:00 UTC, before
    // the site's 20:05 UTC noon, still belongs to it. The catalog's quiet
    // hours would have put the second in the night of the 16th.
    assert_eq!(
        nights,
        vec![(1, "2024-01-15".to_string()), (2, "2024-01-15".to_string())]
    );
}
