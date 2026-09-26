//! Automatic import: on open, on schedule, and on demand, always cheap for
//! frames the catalog already has.

use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post, put};
use axum::Router;
use http_body_util::BodyExt;
use psf_guard::server::{autoimport, handlers, state::AppState};
use serde_json::{json, Value};
use tempfile::tempdir;
use tower::ServiceExt;

fn build_app(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/api/databases/create",
            post(handlers::create_database_route),
        )
        .route(
            "/api/databases/{db_id}",
            put(handlers::update_database_route),
        )
        .route("/api/databases", get(handlers::list_databases))
        .route("/api/db/{db_id}/import", get(handlers::get_import_progress))
        .route(
            "/api/db/{db_id}/autoimport",
            get(handlers::get_autoimport_status),
        )
        .route(
            "/api/db/{db_id}/autoimport/run",
            post(handlers::run_autoimport_now),
        )
        .with_state(state)
}

fn card(out: &mut Vec<u8>, text: &str) {
    let mut bytes = text.as_bytes().to_vec();
    bytes.resize(80, b' ');
    out.extend_from_slice(&bytes);
}

fn write_fits(path: &std::path::Path, object: &str, date_obs: &str) {
    let mut header = Vec::new();
    card(&mut header, "SIMPLE  =                    T");
    card(&mut header, "BITPIX  =                   16");
    card(&mut header, "NAXIS   =                    2");
    card(&mut header, "NAXIS1  =                   10");
    card(&mut header, "NAXIS2  =                   10");
    card(&mut header, "IMAGETYP= 'LIGHT   '");
    card(&mut header, &format!("OBJECT  = '{object}'"));
    card(&mut header, "FILTER  = 'L       '");
    card(&mut header, &format!("DATE-OBS= '{date_obs}'"));
    card(&mut header, "EXPTIME =                300.0");
    card(&mut header, "GAIN    =                  100");
    card(&mut header, "XBINNING=                    1");
    card(&mut header, "YBINNING=                    1");
    card(&mut header, "RA      =            10.684700");
    card(&mut header, "DEC     =            41.268700");
    card(&mut header, "TELESCOP= 'TestScope'");
    card(&mut header, "INSTRUME= 'TestCam '");
    card(&mut header, "END");
    header.resize(header.len().div_ceil(2880) * 2880, b' ');
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(&header).unwrap();
    file.write_all(&[0u8; 2880]).unwrap();
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
    state.set_registry_path(Some(dir.join("config.json")));
    state.set_allow_database_management(true);
    state
}

/// A catalog created from an empty folder, with its creation import over.
async fn empty_catalog(state: &Arc<AppState>, images: &std::path::Path) -> String {
    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases/create",
        Some(json!({ "name": "Auto", "image_dirs": [images], "backfill": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create response: {body}");
    let slug = body["data"]["database"]["id"].as_str().unwrap().to_string();
    let progress = wait_for_import(state, &slug).await;
    assert_eq!(progress["trigger"], "manual");
    slug
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_open_then_schedule_imports_only_what_is_new() {
    let dir = tempdir().unwrap();
    let images = dir.path().join("incoming");
    std::fs::create_dir_all(&images).unwrap();
    let state = state_with_management(dir.path());
    let slug = empty_catalog(&state, &images).await;

    // Nothing is configured yet: a tick does nothing and the status says so.
    autoimport::tick(&state, Instant::now()).await;
    let (status, body) = json_request(
        build_app(state.clone()),
        "GET",
        &format!("/api/db/{slug}/autoimport"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["settings"].is_null(), "{body}");
    assert!(body["data"]["progress"].is_null(), "{body}");

    write_fits(
        &images.join("m31_001.fits"),
        "M31",
        "2026-01-10T01:00:00.000",
    );
    write_fits(
        &images.join("m31_002.fits"),
        "M31",
        "2026-01-10T01:05:00.000",
    );

    let (status, body) = json_request(
        build_app(state.clone()),
        "PUT",
        &format!("/api/databases/{slug}"),
        Some(json!({
            "autoimport": {
                "enabled": true,
                "on_open": true,
                "interval_minutes": 30,
                "scope": "all",
                "backfill": false
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "update response: {body}");
    assert_eq!(body["data"]["autoimport"]["interval_minutes"], 30);
    let saved = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    assert!(saved.contains("\"autoimport\""), "{saved}");

    // First sight of the enabled database: the on-open run.
    let opened = Instant::now();
    autoimport::tick(&state, opened).await;
    let progress = wait_for_import(&state, &slug).await;
    assert_eq!(progress["trigger"], "automatic", "{progress}");
    assert_eq!(progress["stage"], "complete", "{progress}");
    assert_eq!(progress["outcome"]["imported"], 2, "{progress}");
    assert_eq!(progress["prefiltered"], 0);

    let (_, body) = json_request(
        build_app(state.clone()),
        "GET",
        &format!("/api/db/{slug}/autoimport"),
        None,
    )
    .await;
    assert_eq!(body["data"]["settings"]["enabled"], true);
    assert!(body["data"]["last_started_at"].is_i64(), "{body}");
    assert!(body["data"]["next_run_at"].is_i64(), "{body}");
    assert_eq!(body["data"]["progress"]["outcome"]["imported"], 2);

    // Too soon for the schedule: nothing starts, so the job still shows the
    // on-open run (a second run would have prefiltered the two frames).
    write_fits(
        &images.join("m31_003.fits"),
        "M31",
        "2026-01-10T01:10:00.000",
    );
    autoimport::tick(&state, opened + Duration::from_secs(10 * 60)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let progress = wait_for_import(&state, &slug).await;
    assert_eq!(progress["prefiltered"], 0, "no second run yet: {progress}");
    assert_eq!(progress["outcome"]["imported"], 2, "{progress}");

    // The schedule comes due; the two known frames are dropped before any
    // header read and only the new one is imported.
    autoimport::tick(&state, opened + Duration::from_secs(31 * 60)).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let progress = wait_for_import(&state, &slug).await;
    assert_eq!(progress["outcome"]["imported"], 1, "{progress}");
    assert_eq!(progress["outcome"]["skipped_existing"], 2, "{progress}");
    assert_eq!(progress["prefiltered"], 2, "{progress}");
    assert_eq!(
        progress["total_files"], 1,
        "only the new file had its header read"
    );

    // Nothing new: a run reads no headers and touches no table.
    autoimport::tick(&state, opened + Duration::from_secs(62 * 60)).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let progress = wait_for_import(&state, &slug).await;
    assert_eq!(progress["outcome"]["imported"], 0, "{progress}");
    assert_eq!(progress["outcome"]["scanned"], 3, "{progress}");
    assert_eq!(progress["prefiltered"], 3, "{progress}");
    assert_eq!(progress["total_files"], 0, "{progress}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_now_works_without_a_schedule_and_off_settings_clear_the_entry() {
    let dir = tempdir().unwrap();
    let images = dir.path().join("incoming");
    std::fs::create_dir_all(&images).unwrap();
    let state = state_with_management(dir.path());
    let slug = empty_catalog(&state, &images).await;
    write_fits(
        &images.join("ngc7000_001.fits"),
        "NGC 7000",
        "2026-02-01T02:00:00.000",
    );

    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        &format!("/api/db/{slug}/autoimport/run"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["started"], true);
    let progress = wait_for_import(&state, &slug).await;
    assert_eq!(
        progress["trigger"], "automatic",
        "run now is the automatic import by hand"
    );
    assert_eq!(progress["outcome"]["imported"], 1, "{progress}");

    // A scheduled-only database is never "opened" into a run; its schedule
    // counts from the first tick that saw it. (Saving settings reopens the
    // context, so the job store starts empty here.)
    let (status, _) = json_request(
        build_app(state.clone()),
        "PUT",
        &format!("/api/databases/{slug}"),
        Some(
            json!({ "autoimport": { "enabled": true, "on_open": false, "interval_minutes": 60 } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    autoimport::tick(&state, Instant::now()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (_, body) = json_request(
        build_app(state.clone()),
        "GET",
        &format!("/api/db/{slug}/autoimport"),
        None,
    )
    .await;
    // "Run now" counted as a run, so the hour counts from it.
    let last = body["data"]["last_started_at"]
        .as_i64()
        .expect("run now recorded");
    assert!(body["data"]["progress"].is_null(), "{body}");
    assert_eq!(body["data"]["next_run_at"], last + 3600, "{body}");

    // Invalid: enabled with neither trigger.
    let (status, body) = json_request(
        build_app(state.clone()),
        "PUT",
        &format!("/api/databases/{slug}"),
        Some(json!({ "autoimport": { "enabled": true, "on_open": false, "interval_minutes": 0 } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // Off with defaults clears the entry rather than storing a disabled block.
    let (status, body) = json_request(
        build_app(state.clone()),
        "PUT",
        &format!("/api/databases/{slug}"),
        Some(json!({ "autoimport": { "enabled": false } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"].get("autoimport").is_none(), "{body}");
    let saved = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    assert!(!saved.contains("\"autoimport\""), "{saved}");
}
