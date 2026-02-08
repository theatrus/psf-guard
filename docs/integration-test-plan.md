# Integration Test Plan: Sequence Analysis API

## Approach Decision

### Strategy: In-Process Router Testing with `tower::ServiceExt`

We will use Axum's built-in support for testing via `tower::ServiceExt` to send HTTP
requests directly to the `Router` without binding to a TCP port. This approach:

- Requires no real HTTP server or port allocation
- Tests the full middleware stack (CORS, tracing layers)
- Validates JSON serialization/deserialization end-to-end
- Uses an in-memory SQLite database with test fixtures
- Runs as standard `cargo test` (no external dependencies)

**Why not E2E/Playwright?** The sequence analysis endpoints return JSON data with no
browser interaction. In-process testing is faster, deterministic, and sufficient.

**Why not snapshot testing?** The quality scores are floating-point values sensitive to
algorithm tuning. Snapshot tests would be brittle. Instead we test structural correctness
and behavioral invariants.

### Key Technical Details

- **axum 0.8** with `tower 0.5` -- use `tower::ServiceExt::oneshot` to send requests
- **rusqlite** with `:memory:` -- in-memory SQLite for test fixtures, no file I/O
- **AppState construction** -- `AppState::new()` validates that the database file and
  image directories exist on disk. For integration tests, we need a test helper that
  bypasses these checks and accepts a pre-opened `Connection` directly. We will add a
  `#[cfg(test)]` constructor `AppState::new_for_test(conn, cache_dir)`.
- **No image directory needed** -- sequence analysis endpoints only read from the database
  (metadata JSON), not from FITS files on disk.
- **tempfile crate** -- already in `[dev-dependencies]`, used for temporary cache directories.

## Test Fixture Schema

### Database Setup

Create an in-memory SQLite database with the N.I.N.A. Target Scheduler schema:

```sql
CREATE TABLE project (
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
    dec REAL,
    FOREIGN KEY (projectId) REFERENCES project(Id)
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
    profileId TEXT,
    FOREIGN KEY (projectId) REFERENCES project(Id),
    FOREIGN KEY (targetId) REFERENCES target(Id)
);
```

Note: This is the "old schema" without `guid` columns. The `SchemaCapabilities::detect()`
will correctly report `has_*_guid = false` and queries will work without guid columns.

### Fixture Data Sets

#### Fixture 1: Normal Sequence (10 images, mixed quality)

- Project: "Test Project" (id=1), Target: "M42" (id=1), Filter: "L"
- 10 images spaced 5 minutes apart (300s)
- Star counts: 300-350 range (normal variation)
- HFR: 2.3-2.7 range
- All with DetectedStars, HFR in metadata JSON
- Some with SNR and Eccentricity, some without (tests missing metrics)

#### Fixture 2: Cloud Passage Event

- Project: "Test Project" (id=1), Target: "M42" (id=1), Filter: "Ha"
- 8 images with 2 cloud-affected frames
- Cloud frames: DetectedStars drops to ~100 (from ~300), background rises from 1200 to 1800
- Tests that cloud detection works through the full API path

#### Fixture 3: Session Gap (Two Sessions)

- Same target, same filter
- 5 images in session 1, then 2+ hour gap, then 5 images in session 2
- Tests session splitting via the API

#### Fixture 4: Short Sequence (< min_sequence_length)

- 2 images only
- Should return quality_score = 1.0 for all images

#### Fixture 5: Missing Metrics

- Images with only DetectedStars (no HFR, no SNR, no Eccentricity, no Background)
- Tests graceful handling of sparse metadata

#### Fixture 6: Empty Target

- Target with no images at all
- Should return empty sequences array

### Metadata JSON Format

Each `acquiredimage.metadata` field is a JSON string like:

```json
{
    "FileName": "2024-01-15\\M42\\Light\\L\\M42_L_300s_001.fits",
    "FilterName": "L",
    "HFR": 2.5,
    "DetectedStars": 342,
    "Eccentricity": 0.35,
    "SNR": 45.2,
    "Background": 1200.5,
    "ExposureStartTime": "2024-01-15T22:00:00Z"
}
```

## Test Cases

### File: `tests/integration_sequence_analysis.rs`

All tests share a common test harness module.

---

### Test 1: `test_analyze_sequence_normal`

**Endpoint:** `GET /api/analysis/sequence?target_id=1&filter_name=L`

**Setup:** Fixture 1 (10 normal images)

**Assertions:**
- Response status 200
- `success == true`
- `data.sequences` has exactly 1 sequence (single session)
- `sequences[0].target_id == 1`
- `sequences[0].target_name == "M42"`
- `sequences[0].filter_name == "L"`
- `sequences[0].image_count == 10`
- `sequences[0].session_start` and `session_end` are present
- `sequences[0].reference_values` has `best_star_count`, `best_hfr` populated
- All `sequences[0].images[*].quality_score` are between 0.0 and 1.0
- Summary counts sum to `image_count`

---

### Test 2: `test_analyze_sequence_cloud_detection`

**Endpoint:** `GET /api/analysis/sequence?target_id=1&filter_name=Ha`

**Setup:** Fixture 2 (cloud passage)

**Assertions:**
- Cloud-affected frames have lower `quality_score` than good frames
- At least one image has `category == "likely_clouds"`
- `summary.cloud_events_detected >= 1`
- Good frames have `quality_score > 0.5`

---

### Test 3: `test_analyze_sequence_session_split`

**Endpoint:** `GET /api/analysis/sequence?target_id=1`

**Setup:** Fixture 3 (two sessions with gap)

**Assertions:**
- `sequences` has 2 entries
- Each sequence has `image_count == 5`
- Session 1 ends before session 2 starts
- Gap between `session_end` of seq[0] and `session_start` of seq[1] > 60 minutes

---

### Test 4: `test_analyze_sequence_short`

**Endpoint:** `GET /api/analysis/sequence?target_id=X` (target with 2 images)

**Setup:** Fixture 4

**Assertions:**
- Single sequence returned
- `image_count == 2`
- All images have `quality_score == 1.0` (short sequence default)
- `summary.excellent_count == 2`

---

### Test 5: `test_analyze_sequence_missing_metrics`

**Endpoint:** `GET /api/analysis/sequence?target_id=X`

**Setup:** Fixture 5

**Assertions:**
- Does not error (200 OK)
- `normalized_metrics` fields that had no data are `null`
- `quality_score` is still computed using available metrics

---

### Test 6: `test_analyze_sequence_empty_target`

**Endpoint:** `GET /api/analysis/sequence?target_id=X` (target with no images)

**Setup:** Fixture 6

**Assertions:**
- 200 OK
- `sequences` is an empty array

---

### Test 7: `test_analyze_sequence_nonexistent_target`

**Endpoint:** `GET /api/analysis/sequence?target_id=9999`

**Assertions:**
- Returns 400 Bad Request (target not found)
- `success == false`
- `error` message contains "not found"

---

### Test 8: `test_image_quality_context`

**Endpoint:** `GET /api/analysis/image/{image_id}`

**Setup:** Fixture 1

**Assertions:**
- 200 OK
- `image_id` matches request
- `quality` is present with `quality_score` between 0.0 and 1.0
- `sequence_target_id == 1`
- `sequence_filter_name == "L"`
- `sequence_image_count >= 3`
- `reference_values` has populated fields

---

### Test 9: `test_image_quality_nonexistent_image`

**Endpoint:** `GET /api/analysis/image/99999`

**Assertions:**
- Returns 404 Not Found

---

### Test 10: `test_analyze_sequence_custom_weights`

**Endpoint:** `GET /api/analysis/sequence?target_id=1&filter_name=L&weight_star_count=1.0&weight_hfr=0.0&weight_eccentricity=0.0&weight_snr=0.0&weight_background=0.0`

**Setup:** Fixture 1

**Assertions:**
- 200 OK
- Scores exist and are between 0.0 and 1.0
- With star_count as the only weight, images with higher star counts should score higher

---

### Test 11: `test_analyze_sequence_custom_session_gap`

**Endpoint:** `GET /api/analysis/sequence?target_id=1&session_gap_minutes=5`

**Setup:** Fixture 3 (two sessions with 2hr gap; images 5 min apart)

**Assertions:**
- With a 5-minute gap threshold, each 5-minute-spaced image becomes its own session
  OR sequences split more aggressively
- More sequences than the default (2)

---

### Test 12: `test_json_contract_structure`

**Endpoint:** `GET /api/analysis/sequence?target_id=1&filter_name=L`

**Purpose:** Validate JSON structure matches TypeScript interfaces

**Assertions:**
- Response deserializes into the expected Rust types (proving serde works)
- Additionally, parse raw JSON and verify:
  - `sequences[0].images[0]` has keys: `image_id`, `quality_score`, `temporal_anomaly_score`, `category`, `normalized_metrics`, `details`
  - `normalized_metrics` has keys: `star_count`, `hfr`, `eccentricity`, `snr`, `background`
  - `summary` has keys: `excellent_count`, `good_count`, `fair_count`, `poor_count`, `bad_count`, `cloud_events_detected`, `focus_drift_detected`, `tracking_issues_detected`
  - `reference_values` has keys: `best_star_count`, `best_hfr`, `best_eccentricity`, `best_snr`, `best_background`
  - Enum values use snake_case (e.g., `"likely_clouds"` not `"LikelyClouds"`)

---

### Test 13: `test_api_response_wrapper_structure`

**Endpoint:** Any analysis endpoint

**Purpose:** Validate the `ApiResponse<T>` wrapper

**Assertions:**
- Top-level JSON has keys: `success`, `data`, `error`, `status`
- `status` is `"ready"` for successful responses
- Error responses have `success == false` and `error` populated

## Code Structure

### New Test Helper: `AppState::new_for_test`

Add to `src/server/state.rs` inside a `#[cfg(test)]` block:

```rust
#[cfg(test)]
impl AppState {
    /// Create an AppState for integration testing with a pre-opened database connection.
    /// Skips file system validation (no image dirs or cache dir needed).
    pub fn new_for_test(conn: Connection) -> Self {
        Self {
            database_path: ":memory:".to_string(),
            image_dirs: vec![],
            cache_dir: "/tmp/psf-guard-test".to_string(),
            image_dir_paths: vec![],
            cache_dir_path: std::path::PathBuf::from("/tmp/psf-guard-test"),
            db_connection: Arc::new(Mutex::new(conn)),
            file_check_cache: Arc::new(RwLock::new(FileCheckCache::new())),
            directory_tree_cache: Arc::new(RwLock::new(None)),
            refresh_mutex: Arc::new(TokioMutex::new(())),
            pregeneration_config: crate::cli::PregenerationConfig::default(),
        }
    }
}
```

### Integration Test File: `tests/integration_sequence_analysis.rs`

```rust
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
        );"
    ).unwrap();
}

fn insert_project(conn: &Connection, id: i32, name: &str) {
    conn.execute(
        "INSERT INTO project (Id, profileId, name) VALUES (?1, 'default', ?2)",
        rusqlite::params![id, name],
    ).unwrap();
}

fn insert_target(conn: &Connection, id: i32, project_id: i32, name: &str) {
    conn.execute(
        "INSERT INTO target (Id, projectId, name, active) VALUES (?1, ?2, ?3, 1)",
        rusqlite::params![id, project_id, name],
    ).unwrap();
}

fn insert_image(
    conn: &Connection,
    id: i32,
    project_id: i32,
    target_id: i32,
    timestamp: i64,
    filter: &str,
    metadata: &serde_json::Value,
) {
    conn.execute(
        "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![id, project_id, target_id, timestamp, filter,
                          serde_json::to_string(metadata).unwrap()],
    ).unwrap();
}

fn build_metadata(stars: f64, hfr: f64, bg: Option<f64>, snr: Option<f64>, ecc: Option<f64>) -> Value {
    let mut m = serde_json::json!({
        "FileName": "test.fits",
        "DetectedStars": stars,
        "HFR": hfr
    });
    if let Some(v) = bg { m["Background"] = serde_json::json!(v); }
    if let Some(v) = snr { m["SNR"] = serde_json::json!(v); }
    if let Some(v) = ecc { m["Eccentricity"] = serde_json::json!(v); }
    m
}

fn create_test_app(conn: Connection) -> Router {
    use axum::routing::get;
    use psf_guard::server::handlers;
    use psf_guard::server::state::AppState;

    let state = Arc::new(AppState::new_for_test(conn));

    Router::new()
        .route("/api/analysis/sequence", get(handlers::analyze_sequence))
        .route("/api/analysis/image/{image_id}", get(handlers::get_image_quality))
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

fn load_normal_sequence(conn: &Connection) { /* Fixture 1 */ }
fn load_cloud_passage(conn: &Connection) { /* Fixture 2 */ }
fn load_session_gap(conn: &Connection) { /* Fixture 3 */ }
fn load_short_sequence(conn: &Connection) { /* Fixture 4 */ }
fn load_missing_metrics(conn: &Connection) { /* Fixture 5 */ }
fn load_empty_target(conn: &Connection) { /* Fixture 6 */ }

// ---- Tests ----

#[tokio::test]
async fn test_analyze_sequence_normal() {
    let conn = Connection::open_in_memory().unwrap();
    create_test_schema(&conn);
    load_normal_sequence(&conn);
    let app = create_test_app(conn);

    let (status, json) = get_json(app, "/api/analysis/sequence?target_id=1&filter_name=L").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // ... detailed assertions
}

// ... remaining tests follow the same pattern
```

### Required Changes to Production Code

1. **`src/server/state.rs`**: Add `#[cfg(test)] pub fn new_for_test(conn: Connection)` method
2. **`Cargo.toml`**: Add `http-body-util` to `[dev-dependencies]` for response body collection
3. **No other production code changes needed**

### Dev Dependencies to Add

```toml
[dev-dependencies]
tempfile = "3.23"           # Already present
http-body-util = "0.1"     # For collecting response bodies in tests
tower = { version = "0.5", features = ["util"] }  # For ServiceExt::oneshot
```

Note: `tower` is already in `[dependencies]` with `features = ["util"]`, so `ServiceExt`
is available. We only need `http-body-util` as an additional dev dependency.

## How to Run the Tests

```bash
# Run all integration tests
cargo test --test integration_sequence_analysis

# Run a specific test
cargo test --test integration_sequence_analysis test_analyze_sequence_cloud_detection

# Run with output visible
cargo test --test integration_sequence_analysis -- --nocapture

# Run integration tests alongside unit tests
cargo test
```

## Contract Validation Strategy

Rather than generating JSON schemas, we validate contracts pragmatically:

1. **Rust-side**: Deserialize API responses back into the same response types
   (`SequenceAnalysisResponse`, `ImageQualityContextResponse`) to prove round-trip
   serialization works.

2. **Field presence**: Parse as raw `serde_json::Value` and assert that all fields
   expected by the TypeScript interfaces exist with the correct types.

3. **Enum serialization**: Verify that `IssueCategory` variants serialize as snake_case
   strings (e.g., `"likely_clouds"`, `"focus_drift"`) matching the TypeScript string
   union type.

4. **Optional field behavior**: Verify that `None` values serialize as JSON `null`
   (matching TypeScript's `?: type` optional fields).

## Summary

| Component | Count |
|---|---|
| Test fixtures | 6 data sets |
| Test cases | 13 tests |
| Production code changes | 1 file (state.rs: test constructor) |
| New dev dependencies | 1 (http-body-util) |
| New files | 1 (tests/integration_sequence_analysis.rs) |
