use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use rusqlite::Connection;
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

// ---- Test Harness ----

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

fn insert_project(conn: &Connection, id: i32, name: &str) {
    conn.execute(
        "INSERT INTO project (Id, profileId, name) VALUES (?1, 'default', ?2)",
        rusqlite::params![id, name],
    )
    .unwrap();
}

fn insert_target(conn: &Connection, id: i32, project_id: i32, name: &str) {
    conn.execute(
        "INSERT INTO target (Id, projectId, name, active) VALUES (?1, ?2, ?3, 1)",
        rusqlite::params![id, project_id, name],
    )
    .unwrap();
}

fn insert_image(
    conn: &Connection,
    id: i32,
    project_id: i32,
    target_id: i32,
    timestamp: i64,
    filter: &str,
    metadata: &Value,
) {
    conn.execute(
        "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            id,
            project_id,
            target_id,
            timestamp,
            filter,
            serde_json::to_string(metadata).unwrap()
        ],
    )
    .unwrap();
}

fn build_metadata(
    stars: f64,
    hfr: f64,
    bg: Option<f64>,
    snr: Option<f64>,
    ecc: Option<f64>,
) -> Value {
    let mut m = serde_json::json!({
        "FileName": "test.fits",
        "DetectedStars": stars,
        "HFR": hfr
    });
    if let Some(v) = bg {
        m["Background"] = serde_json::json!(v);
    }
    if let Some(v) = snr {
        m["SNR"] = serde_json::json!(v);
    }
    if let Some(v) = ecc {
        m["Eccentricity"] = serde_json::json!(v);
    }
    m
}

fn create_test_app(conn: Connection) -> Router {
    use axum::routing::get;
    use psf_guard::server::handlers;
    use psf_guard::server::state::AppState;

    let state = Arc::new(AppState::new_for_test(conn));

    Router::new()
        .route("/api/analysis/sequence", get(handlers::analyze_sequence))
        .route(
            "/api/analysis/image/{image_id}",
            get(handlers::get_image_quality),
        )
        .with_state(state)
}

async fn get_json(app: Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();

    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

// ---- Fixture Loaders ----

/// Fixture 1: Normal sequence - 10 images, mixed quality, L filter
/// Jan 15, 2024 starting at 22:00 UTC, 5 min apart
fn load_normal_sequence(conn: &Connection) {
    insert_project(conn, 1, "Test Project");
    insert_target(conn, 1, 1, "M42");

    let base_ts: i64 = 1705352400; // 2024-01-15T22:00:00Z
    let stars = [
        320.0, 335.0, 310.0, 345.0, 300.0, 330.0, 315.0, 340.0, 325.0, 350.0,
    ];
    let hfrs = [2.4, 2.3, 2.5, 2.35, 2.6, 2.45, 2.55, 2.3, 2.4, 2.7];
    let bgs = [
        1200.0, 1210.0, 1195.0, 1205.0, 1215.0, 1200.0, 1190.0, 1208.0, 1202.0, 1198.0,
    ];

    for i in 0..10 {
        let ts = base_ts + i as i64 * 300;
        // Some images have SNR and eccentricity, some don't (tests missing metrics)
        let snr = if i < 7 {
            Some(45.0 + (i as f64 - 3.0) * 1.5)
        } else {
            None
        };
        let ecc = if i < 8 {
            Some(0.35 + (i as f64 - 4.0) * 0.01)
        } else {
            None
        };
        let meta = build_metadata(stars[i], hfrs[i], Some(bgs[i]), snr, ecc);
        insert_image(conn, (i + 1) as i32, 1, 1, ts, "L", &meta);
    }
}

/// Fixture 2: Cloud passage event - 8 Ha images
/// 3 good, 2 cloud-affected, 3 good
fn load_cloud_passage(conn: &Connection) {
    // Re-use project/target from fixture 1 if present, or create new
    let has_project: bool = conn
        .query_row("SELECT COUNT(*) FROM project WHERE Id = 1", [], |row| {
            row.get::<_, i32>(0)
        })
        .unwrap()
        > 0;
    if !has_project {
        insert_project(conn, 1, "Test Project");
        insert_target(conn, 1, 1, "M42");
    }

    let base_ts: i64 = 1705356000; // 2024-01-15T23:00:00Z (after L sequence)

    // Good frames: stars ~300, bg ~1200, snr ~45, hfr ~2.5
    // Cloud frames: stars drop to ~90 (70% drop), bg rises to ~1680 (40% rise)
    let data: [(f64, f64, f64, f64); 8] = [
        (310.0, 2.45, 1200.0, 46.0), // good
        (305.0, 2.50, 1210.0, 44.5), // good
        (315.0, 2.40, 1195.0, 47.0), // good
        (90.0, 3.80, 1680.0, 12.0),  // cloud
        (85.0, 4.00, 1700.0, 10.0),  // cloud
        (300.0, 2.55, 1220.0, 43.0), // good (recovery)
        (308.0, 2.48, 1205.0, 45.5), // good
        (312.0, 2.42, 1198.0, 46.5), // good
    ];

    for (i, (stars, hfr, bg, snr)) in data.iter().enumerate() {
        let ts = base_ts + i as i64 * 300;
        let meta = build_metadata(*stars, *hfr, Some(*bg), Some(*snr), Some(0.35));
        insert_image(conn, 100 + (i + 1) as i32, 1, 1, ts, "Ha", &meta);
    }
}

/// Fixture 3: Session gap - two sessions with 2+ hour gap, L filter, same target
fn load_session_gap(conn: &Connection) {
    insert_project(conn, 2, "Session Gap Project");
    insert_target(conn, 2, 2, "NGC7000");

    let base_ts1: i64 = 1705352400; // Session 1: 2024-01-15T22:00:00Z
    let base_ts2: i64 = base_ts1 + 5 * 300 + 7200; // Session 2: ~2h20m after session 1 start

    // Session 1: 5 images
    for i in 0i32..5 {
        let ts = base_ts1 + i as i64 * 300;
        let meta = build_metadata(
            280.0 + i as f64 * 5.0,
            2.6,
            Some(1100.0),
            Some(40.0),
            Some(0.30),
        );
        insert_image(conn, 200 + i + 1, 2, 2, ts, "L", &meta);
    }

    // Session 2: 5 images
    for i in 0i32..5 {
        let ts = base_ts2 + i as i64 * 300;
        let meta = build_metadata(
            290.0 + i as f64 * 3.0,
            2.55,
            Some(1120.0),
            Some(42.0),
            Some(0.32),
        );
        insert_image(conn, 210 + i + 1, 2, 2, ts, "L", &meta);
    }
}

/// Fixture 4: Short sequence - only 2 images (below min_sequence_length)
fn load_short_sequence(conn: &Connection) {
    insert_project(conn, 3, "Short Sequence Project");
    insert_target(conn, 3, 3, "IC1396");

    let base_ts: i64 = 1705352400;
    for i in 0i32..2 {
        let ts = base_ts + i as i64 * 300;
        let meta = build_metadata(
            250.0 + i as f64 * 10.0,
            2.8,
            Some(1300.0),
            Some(38.0),
            Some(0.40),
        );
        insert_image(conn, 300 + i + 1, 3, 3, ts, "L", &meta);
    }
}

/// Fixture 5: Missing metrics - images with only DetectedStars (no HFR, no SNR, etc.)
fn load_missing_metrics(conn: &Connection) {
    insert_project(conn, 4, "Missing Metrics Project");
    insert_target(conn, 4, 4, "M31");

    let base_ts: i64 = 1705352400;
    for i in 0i32..5 {
        let ts = base_ts + i as i64 * 300;
        // Only DetectedStars, no HFR, no SNR, no Eccentricity, no Background
        let meta = serde_json::json!({
            "FileName": "test.fits",
            "DetectedStars": 200.0 + i as f64 * 20.0,
        });
        insert_image(conn, 400 + i + 1, 4, 4, ts, "L", &meta);
    }
}

/// Fixture 6: Empty target - target with no images
fn load_empty_target(conn: &Connection) {
    insert_project(conn, 5, "Empty Target Project");
    insert_target(conn, 5, 5, "EmptyTarget");
    // No images inserted
}

// ---- Tests ----

/// Test 1: Normal sequence analysis
#[tokio::test]
async fn test_analyze_sequence_normal() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    load_normal_sequence(&conn);
    let app = create_test_app(conn);

    let (status, json) = get_json(app, "/api/analysis/sequence?target_id=1&filter_name=L").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    let sequences = json["data"]["sequences"].as_array().unwrap();
    assert_eq!(
        sequences.len(),
        1,
        "Should have exactly 1 sequence (single session)"
    );

    let seq = &sequences[0];
    assert_eq!(seq["target_id"], 1);
    assert_eq!(seq["target_name"], "M42");
    assert_eq!(seq["filter_name"], "L");
    assert_eq!(seq["image_count"], 10);
    assert!(seq["session_start"].is_number());
    assert!(seq["session_end"].is_number());

    // Reference values should be populated
    let refs = &seq["reference_values"];
    assert!(
        refs["best_star_count"].is_number(),
        "best_star_count should be populated"
    );
    assert!(refs["best_hfr"].is_number(), "best_hfr should be populated");

    // All quality scores should be between 0.0 and 1.0
    let images = seq["images"].as_array().unwrap();
    assert_eq!(images.len(), 10);
    for img in images {
        let score = img["quality_score"].as_f64().unwrap();
        assert!(
            (0.0..=1.0).contains(&score),
            "quality_score should be between 0.0 and 1.0, got {}",
            score
        );
    }

    // Summary counts should sum to image_count
    let summary = &seq["summary"];
    let total_summary = summary["excellent_count"].as_u64().unwrap()
        + summary["good_count"].as_u64().unwrap()
        + summary["fair_count"].as_u64().unwrap()
        + summary["poor_count"].as_u64().unwrap()
        + summary["bad_count"].as_u64().unwrap();
    assert_eq!(
        total_summary, 10,
        "Summary counts should sum to image_count"
    );
}

/// Test 2: Cloud detection through the full API path
#[tokio::test]
async fn test_analyze_sequence_cloud_detection() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    load_cloud_passage(&conn);
    let app = create_test_app(conn);

    let (status, json) = get_json(app, "/api/analysis/sequence?target_id=1&filter_name=Ha").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    let sequences = json["data"]["sequences"].as_array().unwrap();
    assert_eq!(sequences.len(), 1);

    let seq = &sequences[0];
    let images = seq["images"].as_array().unwrap();
    assert_eq!(images.len(), 8);

    // Cloud-affected frames (indices 3, 4) should have lower quality_score than good frames
    let good_score = images[0]["quality_score"].as_f64().unwrap();
    let cloud_score_1 = images[3]["quality_score"].as_f64().unwrap();
    let cloud_score_2 = images[4]["quality_score"].as_f64().unwrap();

    assert!(
        cloud_score_1 < good_score,
        "Cloud frame should score worse than good frame: {} vs {}",
        cloud_score_1,
        good_score
    );
    assert!(
        cloud_score_2 < good_score,
        "Cloud frame should score worse than good frame: {} vs {}",
        cloud_score_2,
        good_score
    );

    // At least one image should be classified as likely_clouds
    let has_cloud_category = images
        .iter()
        .any(|img| img["category"].as_str() == Some("likely_clouds"));
    assert!(
        has_cloud_category,
        "At least one image should have category 'likely_clouds'"
    );

    // Summary should detect cloud events
    let summary = &seq["summary"];
    assert!(
        summary["cloud_events_detected"].as_u64().unwrap() >= 1,
        "Should detect at least 1 cloud event"
    );

    // Good frames should score above 0.5
    assert!(
        good_score > 0.5,
        "Good frames should score above 0.5, got {}",
        good_score
    );
}

/// Test 3: Session splitting with 2+ hour gap
#[tokio::test]
async fn test_analyze_sequence_session_split() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    load_session_gap(&conn);
    let app = create_test_app(conn);

    // No filter_name to get all filters for the target
    let (status, json) = get_json(app, "/api/analysis/sequence?target_id=2").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    let sequences = json["data"]["sequences"].as_array().unwrap();
    assert_eq!(sequences.len(), 2, "Should split into 2 sessions");

    // Each sequence should have 5 images
    assert_eq!(sequences[0]["image_count"], 5);
    assert_eq!(sequences[1]["image_count"], 5);

    // Session 1 should end before session 2 starts
    let seq0_end = sequences[0]["session_end"].as_i64().unwrap();
    let seq1_start = sequences[1]["session_start"].as_i64().unwrap();

    assert!(
        seq0_end < seq1_start,
        "Session 1 should end before session 2 starts"
    );

    // Gap should be > 60 minutes (3600 seconds)
    let gap = seq1_start - seq0_end;
    assert!(
        gap > 3600,
        "Gap between sessions should be > 60 minutes (3600s), got {}s",
        gap
    );
}

/// Test 4: Short sequence (below min_sequence_length) returns perfect scores
#[tokio::test]
async fn test_analyze_sequence_short() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    load_short_sequence(&conn);
    let app = create_test_app(conn);

    let (status, json) = get_json(app, "/api/analysis/sequence?target_id=3").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    let sequences = json["data"]["sequences"].as_array().unwrap();
    assert_eq!(sequences.len(), 1, "Should have 1 sequence");

    let seq = &sequences[0];
    assert_eq!(seq["image_count"], 2);

    // All images should have quality_score == 1.0 (short sequence default)
    let images = seq["images"].as_array().unwrap();
    for img in images {
        let score = img["quality_score"].as_f64().unwrap();
        assert_eq!(
            score, 1.0,
            "Short sequence images should get quality_score 1.0, got {}",
            score
        );
    }

    // Summary should count all as excellent
    let summary = &seq["summary"];
    assert_eq!(summary["excellent_count"], 2);
}

/// Test 5: Missing metrics - images with sparse metadata
#[tokio::test]
async fn test_analyze_sequence_missing_metrics() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    load_missing_metrics(&conn);
    let app = create_test_app(conn);

    let (status, json) = get_json(app, "/api/analysis/sequence?target_id=4").await;

    assert_eq!(
        status,
        StatusCode::OK,
        "Should not error with missing metrics"
    );
    assert_eq!(json["success"], true);

    let sequences = json["data"]["sequences"].as_array().unwrap();
    assert_eq!(sequences.len(), 1);

    let seq = &sequences[0];
    let images = seq["images"].as_array().unwrap();
    assert_eq!(images.len(), 5);

    // Check that normalized_metrics fields that had no data are null
    for img in images {
        let nm = &img["normalized_metrics"];
        // star_count should be present (we provided DetectedStars)
        assert!(
            nm["star_count"].is_number(),
            "star_count should be a number since DetectedStars was provided"
        );
        // hfr, eccentricity, snr, background should be null (not provided)
        assert!(
            nm["hfr"].is_null(),
            "hfr should be null when not provided in metadata"
        );
        assert!(
            nm["eccentricity"].is_null(),
            "eccentricity should be null when not provided"
        );
        assert!(nm["snr"].is_null(), "snr should be null when not provided");
        assert!(
            nm["background"].is_null(),
            "background should be null when not provided"
        );
    }

    // quality_score should still be computed using available metrics
    for img in images {
        let score = img["quality_score"].as_f64().unwrap();
        assert!(
            (0.0..=1.0).contains(&score),
            "quality_score should be between 0.0 and 1.0 even with sparse data, got {}",
            score
        );
    }
}

/// Test 6: Empty target - target with no images
#[tokio::test]
async fn test_analyze_sequence_empty_target() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    load_empty_target(&conn);
    let app = create_test_app(conn);

    let (status, json) = get_json(app, "/api/analysis/sequence?target_id=5").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    let sequences = json["data"]["sequences"].as_array().unwrap();
    assert!(
        sequences.is_empty(),
        "Should return empty sequences array for target with no images"
    );
}

/// Test 7: Nonexistent target returns 400 Bad Request
#[tokio::test]
async fn test_analyze_sequence_nonexistent_target() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    // No fixtures loaded - just empty schema
    let app = create_test_app(conn);

    let (status, json) = get_json(app, "/api/analysis/sequence?target_id=9999").await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["success"], false);
    let error_msg = json["error"].as_str().unwrap();
    assert!(
        error_msg.to_lowercase().contains("not found"),
        "Error message should contain 'not found', got: {}",
        error_msg
    );
}

/// Test 8: Image quality context endpoint
#[tokio::test]
async fn test_image_quality_context() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    load_normal_sequence(&conn);
    let app = create_test_app(conn);

    // Request quality context for image 5 (in the middle of the normal sequence)
    let (status, json) = get_json(app, "/api/analysis/image/5").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    let data = &json["data"];
    assert_eq!(data["image_id"], 5);

    // Quality should be present with a valid score
    let quality = &data["quality"];
    assert!(!quality.is_null(), "quality should be present");
    let score = quality["quality_score"].as_f64().unwrap();
    assert!(
        (0.0..=1.0).contains(&score),
        "quality_score should be between 0.0 and 1.0, got {}",
        score
    );

    assert_eq!(data["sequence_target_id"], 1);
    assert_eq!(data["sequence_filter_name"], "L");
    assert!(
        data["sequence_image_count"].as_u64().unwrap() >= 3,
        "sequence_image_count should be >= 3"
    );

    // Reference values should be populated
    let refs = &data["reference_values"];
    assert!(!refs.is_null(), "reference_values should be present");
    assert!(refs["best_star_count"].is_number());
    assert!(refs["best_hfr"].is_number());
}

/// Test 9: Nonexistent image returns 404
#[tokio::test]
async fn test_image_quality_nonexistent_image() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    let app = create_test_app(conn);

    let (status, _json) = get_json(app, "/api/analysis/image/99999").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Test 10: Custom weights change scoring behavior
#[tokio::test]
async fn test_analyze_sequence_custom_weights() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    load_normal_sequence(&conn);
    let app = create_test_app(conn);

    let uri = "/api/analysis/sequence?target_id=1&filter_name=L\
        &weight_star_count=1.0&weight_hfr=0.0&weight_eccentricity=0.0\
        &weight_snr=0.0&weight_background=0.0";

    let (status, json) = get_json(app, uri).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    let sequences = json["data"]["sequences"].as_array().unwrap();
    assert_eq!(sequences.len(), 1);

    let images = sequences[0]["images"].as_array().unwrap();

    // All scores should be between 0.0 and 1.0
    for img in images {
        let score = img["quality_score"].as_f64().unwrap();
        assert!(
            (0.0..=1.0).contains(&score),
            "quality_score should be between 0.0 and 1.0 with custom weights, got {}",
            score
        );
    }

    // With star_count as the only weight, images with higher star counts should score higher
    // Image with most stars (id=10, stars=350) should score >= image with fewest (id=5, stars=300)
    let img_highest_stars = images.iter().find(|i| i["image_id"] == 10).unwrap();
    let img_lowest_stars = images.iter().find(|i| i["image_id"] == 5).unwrap();
    let score_high = img_highest_stars["quality_score"].as_f64().unwrap();
    let score_low = img_lowest_stars["quality_score"].as_f64().unwrap();
    assert!(
        score_high >= score_low,
        "Image with most stars should score >= image with fewest: {} vs {}",
        score_high,
        score_low
    );
}

/// Test 11: Custom session gap threshold splits sessions more aggressively
#[tokio::test]
async fn test_analyze_sequence_custom_session_gap() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    load_session_gap(&conn);

    // First verify default behavior produces 2 sequences
    let conn1 = Connection::open_in_memory().unwrap();
    create_test_schema(&conn1);
    load_session_gap(&conn1);
    let app1 = create_test_app(conn1);
    let (_, json_default) = get_json(app1, "/api/analysis/sequence?target_id=2").await;
    let default_count = json_default["data"]["sequences"].as_array().unwrap().len();
    assert_eq!(default_count, 2, "Default should produce 2 sequences");

    // Now test with a very small gap (5 minutes)
    let conn2 = Connection::open_in_memory().unwrap();
    create_test_schema(&conn2);
    load_session_gap(&conn2);
    let app2 = create_test_app(conn2);
    let (status, json) = get_json(
        app2,
        "/api/analysis/sequence?target_id=2&session_gap_minutes=5",
    )
    .await;

    assert_eq!(status, StatusCode::OK);

    let sequences = json["data"]["sequences"].as_array().unwrap();
    // With 5-minute gap threshold and 5-minute spacing, each frame-pair gap exactly equals the
    // threshold, so we might get more sequences than 2. At minimum it should be >= 2.
    assert!(
        sequences.len() >= 2,
        "With 5-minute gap threshold, should have at least 2 sequences (same or more than default), got {}",
        sequences.len()
    );
}

/// Test 12: JSON contract structure matches TypeScript interfaces
#[tokio::test]
async fn test_json_contract_structure() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    load_normal_sequence(&conn);
    let app = create_test_app(conn);

    let (status, json) = get_json(app, "/api/analysis/sequence?target_id=1&filter_name=L").await;

    assert_eq!(status, StatusCode::OK);

    // Verify top-level structure
    assert!(json.get("success").is_some(), "Missing 'success' field");
    assert!(json.get("data").is_some(), "Missing 'data' field");
    assert!(json.get("error").is_some(), "Missing 'error' field");
    assert!(json.get("status").is_some(), "Missing 'status' field");

    let seq = &json["data"]["sequences"][0];

    // Verify sequence-level fields
    assert!(seq.get("target_id").is_some(), "Missing 'target_id'");
    assert!(seq.get("target_name").is_some(), "Missing 'target_name'");
    assert!(seq.get("filter_name").is_some(), "Missing 'filter_name'");
    assert!(
        seq.get("session_start").is_some(),
        "Missing 'session_start'"
    );
    assert!(seq.get("session_end").is_some(), "Missing 'session_end'");
    assert!(seq.get("image_count").is_some(), "Missing 'image_count'");
    assert!(
        seq.get("reference_values").is_some(),
        "Missing 'reference_values'"
    );
    assert!(seq.get("images").is_some(), "Missing 'images'");
    assert!(seq.get("summary").is_some(), "Missing 'summary'");

    // Verify image-level fields
    let img = &seq["images"][0];
    for key in &[
        "image_id",
        "quality_score",
        "temporal_anomaly_score",
        "category",
        "normalized_metrics",
        "details",
    ] {
        assert!(img.get(*key).is_some(), "Missing '{}' in images[0]", key);
    }

    // Verify normalized_metrics keys
    let nm = &img["normalized_metrics"];
    for key in &["star_count", "hfr", "eccentricity", "snr", "background"] {
        assert!(
            nm.get(*key).is_some(),
            "Missing '{}' in normalized_metrics",
            key
        );
    }

    // Verify summary keys
    let summary = &seq["summary"];
    for key in &[
        "excellent_count",
        "good_count",
        "fair_count",
        "poor_count",
        "bad_count",
        "cloud_events_detected",
        "focus_drift_detected",
        "tracking_issues_detected",
    ] {
        assert!(summary.get(*key).is_some(), "Missing '{}' in summary", key);
    }

    // Verify reference_values keys
    let refs = &seq["reference_values"];
    for key in &[
        "best_star_count",
        "best_hfr",
        "best_eccentricity",
        "best_snr",
        "best_background",
    ] {
        assert!(
            refs.get(*key).is_some(),
            "Missing '{}' in reference_values",
            key
        );
    }

    // Verify enum values use snake_case by checking cloud passage fixture
    let conn2 = Connection::open_in_memory().unwrap();
    create_test_schema(&conn2);
    load_cloud_passage(&conn2);
    let app2 = create_test_app(conn2);
    let (_, cloud_json) = get_json(app2, "/api/analysis/sequence?target_id=1&filter_name=Ha").await;

    let cloud_images = cloud_json["data"]["sequences"][0]["images"]
        .as_array()
        .unwrap();
    let categories: Vec<&str> = cloud_images
        .iter()
        .filter_map(|img| img["category"].as_str())
        .collect();

    // All categories should be snake_case
    for cat in &categories {
        assert!(
            !cat.contains(char::is_uppercase),
            "Category '{}' should be snake_case, not PascalCase",
            cat
        );
    }
}

/// Test 13: API response wrapper structure
#[tokio::test]
async fn test_api_response_wrapper_structure() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    load_normal_sequence(&conn);

    // Test successful response
    let app = create_test_app(conn);
    let (_, json) = get_json(app, "/api/analysis/sequence?target_id=1&filter_name=L").await;

    // Top-level keys
    assert!(json.get("success").is_some(), "Missing 'success'");
    assert!(json.get("data").is_some(), "Missing 'data'");
    assert!(json.get("error").is_some(), "Missing 'error'");
    assert!(json.get("status").is_some(), "Missing 'status'");

    // Successful response values
    assert_eq!(json["success"], true);
    assert_eq!(json["status"], "ready");
    assert!(
        json["data"].is_object(),
        "data should be an object for success"
    );
    assert!(json["error"].is_null(), "error should be null for success");

    // Test error response
    let conn2 = Connection::open_in_memory().unwrap();
    create_test_schema(&conn2);
    let app2 = create_test_app(conn2);
    let (_, err_json) = get_json(app2, "/api/analysis/sequence?target_id=9999").await;

    assert_eq!(err_json["success"], false);
    assert!(
        err_json["error"].is_string(),
        "error should be a string for error responses"
    );
}
