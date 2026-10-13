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
        .route(
            "/api/db/{db_id}/import",
            get(handlers::get_import_progress).post(handlers::start_import_route),
        )
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

const M31: Option<(f64, f64)> = Some((10.6847, 41.2687));
const NGC_7000: Option<(f64, f64)> = Some((314.68, 44.3));

fn write_fits(path: &std::path::Path, object: &str, date_obs: &str) {
    write_frame(path, Some("LIGHT"), Some(object), M31, date_obs, 300.0);
}

/// A frame with any of IMAGETYP, OBJECT and its coordinates left out.
fn write_frame(
    path: &std::path::Path,
    image_type: Option<&str>,
    object: Option<&str>,
    coordinates: Option<(f64, f64)>,
    date_obs: &str,
    exposure_s: f64,
) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut header = Vec::new();
    card(&mut header, "SIMPLE  =                    T");
    card(&mut header, "BITPIX  =                   16");
    card(&mut header, "NAXIS   =                    2");
    card(&mut header, "NAXIS1  =                   10");
    card(&mut header, "NAXIS2  =                   10");
    if let Some(image_type) = image_type {
        card(&mut header, &format!("IMAGETYP= '{image_type}'"));
    }
    if let Some(object) = object {
        card(&mut header, &format!("OBJECT  = '{object}'"));
    }
    card(&mut header, "FILTER  = 'L       '");
    card(&mut header, &format!("DATE-OBS= '{date_obs}'"));
    card(&mut header, &format!("EXPTIME =  {exposure_s:>19.1}"));
    card(&mut header, "GAIN    =                  100");
    card(&mut header, "XBINNING=                    1");
    card(&mut header, "YBINNING=                    1");
    if let Some((ra, dec)) = coordinates {
        card(&mut header, &format!("RA      =  {ra:>19.6}"));
        card(&mut header, &format!("DEC     =  {dec:>19.6}"));
    }
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
                "backfill": false,
                "new_targets": "create"
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "update response: {body}");
    assert_eq!(body["data"]["autoimport"]["interval_minutes"], 30);
    assert_eq!(body["data"]["autoimport"]["new_targets"], "create");
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
    // With no settings saved, the run asks first: NGC 7000 matches no
    // target, so it waits for review instead of becoming a project.
    assert_eq!(progress["outcome"]["imported"], 0, "{progress}");
    assert_eq!(progress["outcome"]["new_targets"], "ask", "{progress}");
    assert_eq!(progress["outcome"]["skipped_unmatched"], 1, "{progress}");
    assert_eq!(progress["outcome"]["projects_created"], 0, "{progress}");

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

/// A catalog created from `images` as it stands, with its creation import
/// over, and the path of its database file.
async fn catalog_from(state: &Arc<AppState>, images: &std::path::Path) -> (String, String) {
    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases/create",
        Some(json!({ "name": "Auto", "image_dirs": [images], "backfill": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create response: {body}");
    let slug = body["data"]["database"]["id"].as_str().unwrap().to_string();
    let path = body["data"]["database"]["database_path"]
        .as_str()
        .unwrap()
        .to_string();
    wait_for_import(state, &slug).await;
    (slug, path)
}

/// Rows in each table an import that creates structure writes to.
fn structure_counts(db_path: &str) -> [i64; 5] {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    let count = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    };
    [
        count("project"),
        count("target"),
        count("exposureplan"),
        count("exposuretemplate"),
        count("ruleweight"),
    ]
}

fn target_names(db_path: &str) -> Vec<String> {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM target ORDER BY name")
        .unwrap();
    stmt.query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

async fn put_settings(state: &Arc<AppState>, slug: &str, body: Value) -> (StatusCode, Value) {
    json_request(
        build_app(state.clone()),
        "PUT",
        &format!("/api/databases/{slug}"),
        Some(body),
    )
    .await
}

async fn run_now(state: &Arc<AppState>, slug: &str) -> Value {
    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        &format!("/api/db/{slug}/autoimport/run"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["started"], true, "{body}");
    wait_for_import(state, slug).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn by_default_a_run_attaches_known_targets_and_holds_new_ones() {
    let dir = tempdir().unwrap();
    let images = dir.path().join("incoming");
    write_fits(
        &images.join("M31/m31_001.fits"),
        "M31",
        "2026-01-10T01:00:00.000",
    );
    let state = state_with_management(dir.path());
    let (slug, db_path) = catalog_from(&state, &images).await;
    let before = structure_counts(&db_path);
    assert_eq!(before[0], 1, "the creation import made M31's project");

    write_fits(
        &images.join("M31/m31_002.fits"),
        "M31",
        "2026-01-11T01:00:00.000",
    );
    for (index, time) in ["02:00", "02:05"].iter().enumerate() {
        write_frame(
            &images.join(format!("NGC7000/n7000_{:03}.fits", index + 1)),
            Some("LIGHT"),
            Some("NGC 7000"),
            NGC_7000,
            &format!("2026-01-11T{time}:00.000"),
            300.0,
        );
    }

    // Turned on with nothing chosen for new targets: Ask first.
    let (status, body) = put_settings(
        &state,
        &slug,
        json!({ "autoimport": { "enabled": true, "on_open": true, "backfill": false } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["autoimport"]["new_targets"], "ask", "{body}");

    let progress = run_now(&state, &slug).await;
    let outcome = &progress["outcome"];
    assert_eq!(outcome["attached"], 1, "{progress}");
    assert_eq!(outcome["imported"], 1, "{progress}");
    assert_eq!(outcome["new_targets"], "ask", "{progress}");
    assert_eq!(outcome["skipped_unmatched"], 2, "{progress}");
    assert_eq!(outcome["unmatched"][0]["target"], "NGC 7000", "{progress}");
    assert_eq!(outcome["unmatched"][0]["frames"], 2, "{progress}");
    assert_eq!(
        outcome["unmatched"][0]["folders"][0],
        images.join("NGC7000").display().to_string(),
        "{progress}"
    );
    assert_eq!(
        structure_counts(&db_path),
        before,
        "no project, target, exposure plan, template or rule weight was written"
    );

    // Leave out: the same frames are listed, and still nothing is written.
    let (status, body) = put_settings(
        &state,
        &slug,
        json!({ "autoimport": { "enabled": true, "on_open": true, "backfill": false, "new_targets": "skip" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let progress = run_now(&state, &slug).await;
    let outcome = &progress["outcome"];
    assert_eq!(outcome["new_targets"], "skip", "{progress}");
    assert_eq!(outcome["skipped_unmatched"], 2, "{progress}");
    assert_eq!(outcome["imported"], 0, "{progress}");
    assert_eq!(structure_counts(&db_path), before);

    // Create: the run does what it always did.
    let (status, body) = put_settings(
        &state,
        &slug,
        json!({ "autoimport": { "enabled": true, "on_open": true, "backfill": false, "new_targets": "create" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let progress = run_now(&state, &slug).await;
    assert_eq!(progress["outcome"]["imported"], 2, "{progress}");
    assert_eq!(progress["outcome"]["projects_created"], 1, "{progress}");
    assert_eq!(target_names(&db_path), vec!["M31", "NGC 7000"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn autofocus_frames_stay_out_by_pattern_ignore_file_or_target_name() {
    let dir = tempdir().unwrap();
    let images = dir.path().join("Images");
    std::fs::create_dir_all(&images).unwrap();
    let state = state_with_management(dir.path());
    let (slug, db_path) = catalog_from(&state, &images).await;

    // The reporter's layout: Hocus Focus autofocus frames, with no OBJECT,
    // beside the lights.
    write_fits(
        &images.join("M31/m31_001.fits"),
        "M31",
        "2026-01-10T01:00:00.000",
    );
    let autofocus = images.join("Hocus Focus/AutoFocus_20260110_010203/attempt01");
    for step in 1..=3 {
        write_frame(
            &autofocus.join(format!("af_{step:02}.fits")),
            Some("LIGHT"),
            None,
            None,
            &format!("2026-01-10T00:5{step}:00.000"),
            2.0,
        );
    }
    let create =
        json!({ "enabled": true, "on_open": true, "backfill": false, "new_targets": "create" });

    // An exclude pattern: the folder is never entered.
    let (status, body) = put_settings(
        &state,
        &slug,
        json!({ "autoimport": create, "scan_rules": { "exclude": ["**/hocus focus/**"] } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["scan_rules"]["exclude"][0], "**/hocus focus/**",
        "{body}"
    );
    let progress = run_now(&state, &slug).await;
    assert_eq!(progress["outcome"]["imported"], 1, "{progress}");
    assert_eq!(
        progress["outcome"]["scan_rules"]["folders_skipped"], 1,
        "{progress}"
    );
    assert_eq!(progress["total_files"], 1, "only M31's header was read");
    assert_eq!(target_names(&db_path), vec!["M31"]);

    // A .psfguardignore file, with no rules at all.
    let (status, body) = put_settings(&state, &slug, json!({ "scan_rules": {} })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"].get("scan_rules").is_none(), "{body}");
    std::fs::write(images.join("Hocus Focus/.psfguardignore"), "").unwrap();
    let progress = run_now(&state, &slug).await;
    assert_eq!(
        progress["outcome"]["scan_rules"]["ignored_folders"], 1,
        "{progress}"
    );
    assert_eq!(progress["total_files"], 0, "{progress}");
    assert_eq!(target_names(&db_path), vec!["M31"]);

    // The target-name filter: the frames are read and left out by reason.
    std::fs::remove_file(images.join("Hocus Focus/.psfguardignore")).unwrap();
    let (status, body) = put_settings(
        &state,
        &slug,
        json!({ "scan_rules": { "require_object": true } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let progress = run_now(&state, &slug).await;
    let outcome = &progress["outcome"];
    assert_eq!(outcome["skipped_by_filter"], 3, "{progress}");
    assert_eq!(
        outcome["left_out"][0]["reason"], "no_target_name",
        "{progress}"
    );
    assert_eq!(outcome["left_out"][0]["frames"], 3, "{progress}");
    assert_eq!(outcome["projects_created"], 0, "{progress}");
    assert_eq!(target_names(&db_path), vec!["M31"]);

    // Without any of them, the frames become "Unknown Target", which is
    // what the reporter had to clean out by hand.
    let (status, _) = put_settings(&state, &slug, json!({ "scan_rules": {} })).await;
    assert_eq!(status, StatusCode::OK);
    let progress = run_now(&state, &slug).await;
    assert_eq!(progress["outcome"]["imported"], 3, "{progress}");
    assert_eq!(target_names(&db_path), vec!["M31", "Unknown Target"]);

    // Autofocus frames taken at the target carry its position and no
    // OBJECT. With position matching off, they match nothing and are held.
    let at_m31 = images.join("Hocus Focus/AutoFocus_20260110_020304/attempt01");
    for step in 1..=2 {
        write_frame(
            // A name the catalog does not have yet: it drops known
            // basenames before reading a header.
            &at_m31.join(format!("af_m31_{step:02}.fits")),
            Some("LIGHT"),
            None,
            M31,
            &format!("2026-01-10T02:0{step}:00.000"),
            2.0,
        );
    }
    let ask = json!({ "enabled": true, "on_open": true, "backfill": false });
    let (status, body) = put_settings(
        &state,
        &slug,
        json!({ "autoimport": ask, "scan_rules": { "match_by_position": false } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["scan_rules"]["match_by_position"], false);
    let progress = run_now(&state, &slug).await;
    let outcome = &progress["outcome"];
    assert_eq!(outcome["attached"], 0, "{progress}");
    assert_eq!(outcome["skipped_unmatched"], 2, "{progress}");
    assert!(outcome["unmatched"][0]["target"].is_null(), "{progress}");

    // On, the default, they attach to M31 by position, which is why
    // "Lights need a target name" is the rule that keeps them out.
    let (status, body) = put_settings(
        &state,
        &slug,
        json!({ "autoimport": create, "scan_rules": { "match_by_position": true } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["data"].get("scan_rules").is_none(),
        "defaults store as none: {body}"
    );
    let progress = run_now(&state, &slug).await;
    let outcome = &progress["outcome"];
    assert_eq!(outcome["attached"], 2, "{progress}");
    assert_eq!(
        outcome["attach_summaries"][0]["matched_by"], "coordinates",
        "{progress}"
    );
    assert_eq!(target_names(&db_path), vec!["M31", "Unknown Target"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scan_rules_save_check_their_patterns_and_clear_when_empty() {
    let dir = tempdir().unwrap();
    let images = dir.path().join("incoming");
    std::fs::create_dir_all(&images).unwrap();
    let state = state_with_management(dir.path());
    let slug = empty_catalog(&state, &images).await;

    let (status, body) = put_settings(
        &state,
        &slug,
        json!({ "scan_rules": { "exclude": ["**/PierCam/**", "[abc"] } }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.to_string().contains("exclude pattern \\\"[abc\\\""),
        "the error says which pattern: {body}"
    );

    let (status, body) = put_settings(
        &state,
        &slug,
        json!({ "scan_rules": {
            "exclude": ["  **/PierCam/**  ", ""],
            "include": ["*LIGHT*"],
            "lights_must_say_light": true,
            "min_exposure_seconds": 10.0
        } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rules = &body["data"]["scan_rules"];
    assert_eq!(rules["exclude"], json!(["**/PierCam/**"]), "{body}");
    assert_eq!(rules["include"], json!(["*LIGHT*"]), "{body}");
    assert_eq!(rules["lights_must_say_light"], true);
    assert_eq!(rules["min_exposure_seconds"], 10.0);
    let saved = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    assert!(saved.contains("\"scan_rules\""), "{saved}");

    let (_, body) = json_request(build_app(state.clone()), "GET", "/api/databases", None).await;
    let listed = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|db| db["id"] == slug.as_str())
        .unwrap()
        .clone();
    assert_eq!(listed["scan_rules"]["include"], json!(["*LIGHT*"]));

    // Rules that change nothing are stored as none.
    let (status, body) = put_settings(
        &state,
        &slug,
        json!({ "scan_rules": { "exclude": [" "], "min_exposure_seconds": 0 } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"].get("scan_rules").is_none(), "{body}");
    let saved = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    assert!(!saved.contains("\"scan_rules\""), "{saved}");
}

/// Rows in the tables that decide what Target Scheduler shoots: templates,
/// plans and their acquired counts. Image rows only record what was shot.
fn scheduling_state(db_path: &str) -> (i64, i64, i64) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    let count = |sql: &str| -> i64 { conn.query_row(sql, [], |row| row.get(0)).unwrap() };
    (
        count("SELECT COUNT(*) FROM exposuretemplate"),
        count("SELECT COUNT(*) FROM exposureplan"),
        count("SELECT IFNULL(SUM(acquired), 0) FROM exposureplan"),
    )
}

fn image_rows(db_path: &str) -> i64 {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.query_row("SELECT COUNT(*) FROM acquiredimage", [], |row| row.get(0))
        .unwrap()
}

fn set_project_description(db_path: &str, description: &str) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.execute("UPDATE project SET description = ?1", [description])
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_that_asks_first_changes_no_plan_in_a_scheduler_project() {
    let dir = tempdir().unwrap();
    let images = dir.path().join("Images");
    write_fits(
        &images.join("M31/2026-01-10/LIGHT/m31_001.fits"),
        "M31",
        "2026-01-10T01:00:00.000",
    );
    let state = state_with_management(dir.path());
    let (slug, db_path) = catalog_from(&state, &images).await;
    // Target Scheduler's own project, with a plan for L 300 s.
    set_project_description(&db_path, "Made in Target Scheduler");
    let before = scheduling_state(&db_path);
    let images_before = image_rows(&db_path);

    // The night: another L 300 s frame, and a 3 s test frame of M31 that no
    // plan covers.
    write_fits(
        &images.join("M31/2026-01-11/LIGHT/m31_002.fits"),
        "M31",
        "2026-01-11T01:00:00.000",
    );
    // Joined part by part: the server reports the folder with the
    // platform's separator, and Windows would keep a `/` written here.
    let snapshot = images.join("M31").join("2026-01-11").join("SNAPSHOT");
    write_frame(
        &snapshot.join("m31_test.fits"),
        Some("LIGHT"),
        Some("M31"),
        M31,
        "2026-01-11T00:50:00.000",
        3.0,
    );
    let (status, body) = put_settings(
        &state,
        &slug,
        json!({ "autoimport": { "enabled": true, "on_open": true, "backfill": false } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let progress = run_now(&state, &slug).await;
    let outcome = &progress["outcome"];
    assert_eq!(outcome["attached"], 1, "{progress}");
    assert_eq!(
        outcome["attach_summaries"][0]["plans_raised"], 0,
        "{progress}"
    );
    assert_eq!(outcome["skipped_unplanned"], 1, "{progress}");
    let held = &outcome["unplanned"][0];
    assert_eq!(held["target"], "M31", "{progress}");
    assert_eq!(held["exposure_s"], 3.0, "{progress}");
    assert_eq!(
        held["folders"][0],
        snapshot.display().to_string(),
        "{progress}"
    );
    assert_eq!(outcome["plans_created"], 0, "{progress}");
    assert_eq!(outcome["templates_created"], 0, "{progress}");
    assert_eq!(
        scheduling_state(&db_path),
        before,
        "no template, no plan, no acquired count changed"
    );
    assert_eq!(image_rows(&db_path), images_before + 1);

    // A person's preview of that folder says what their import would change.
    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        &format!("/api/db/{slug}/import"),
        Some(json!({ "dry_run": true, "image_dirs": [snapshot] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let progress = wait_for_import(&state, &slug).await;
    let outcome = &progress["outcome"];
    assert_eq!(outcome["dry_run"], true, "{progress}");
    assert_eq!(
        outcome["attach_summaries"][0]["plans_created"], 1,
        "{progress}"
    );
    assert_eq!(outcome["plans_created"], 1, "{progress}");
    assert_eq!(
        scheduling_state(&db_path),
        before,
        "the preview wrote nothing"
    );

    // A project an import made gets its plan, as before.
    set_project_description(&db_path, "Imported by PSF Guard (1 frames)");
    let progress = run_now(&state, &slug).await;
    let outcome = &progress["outcome"];
    assert_eq!(outcome["attached"], 1, "{progress}");
    assert_eq!(outcome["skipped_unplanned"], 0, "{progress}");
    assert_eq!(
        outcome["attach_summaries"][0]["plans_created"], 1,
        "{progress}"
    );
    let (templates, plans, acquired) = scheduling_state(&db_path);
    assert_eq!(
        (templates, plans, acquired),
        (before.0, before.1 + 1, before.2 + 1)
    );
}
