//! The calibration settings over HTTP: viewers read them, editors change
//! them, every value is checked against its setting, and a rig's own values
//! are kept beside the server's and put in force at once.

use std::io::Write as _;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{header::COOKIE, Method, Request, StatusCode},
    middleware,
    routing::{get, post, put},
    Router,
};
use http_body_util::BodyExt;
use psf_guard::{
    auth_registry::{AccessRole, AuthRegistry, AuthUserRecord},
    config::ServerAuthConfig,
    server::{
        auth::{self, ServerAuth},
        calibration_settings,
        database_context::DatabaseContext,
        state::AppState,
    },
};
use rusqlite::Connection;
use serde_json::{json, Value};
use tower::ServiceExt;

/// The settings are one block for the whole process; the tests take turns.
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn write_fits(path: &std::path::Path, kind: &str, captured: &str) {
    let cards = [
        "SIMPLE  =                    T".to_string(),
        "BITPIX  =                   16".to_string(),
        "NAXIS   =                    2".to_string(),
        "NAXIS1  =                   16".to_string(),
        "NAXIS2  =                   16".to_string(),
        "BZERO   =                32768".to_string(),
        format!("IMAGETYP= '{kind}'"),
        format!("DATE-OBS= '{captured}'"),
        "FILTER  = 'Ha'".to_string(),
        "EXPTIME =                  2.0".to_string(),
        "GAIN    =                  100".to_string(),
        "OFFSET  =                   20".to_string(),
        "XBINNING=                    1".to_string(),
        "YBINNING=                    1".to_string(),
        "CCD-TEMP=                -10.0".to_string(),
        "TELESCOP= 'SpaceCat61'".to_string(),
        "INSTRUME= 'ASI2600MM'".to_string(),
        "END".to_string(),
    ];
    let mut header = Vec::new();
    for card in cards {
        let mut bytes = card.into_bytes();
        bytes.resize(80, b' ');
        header.extend(bytes);
    }
    header.resize(header.len().div_ceil(2880) * 2880, b' ');
    let mut payload = vec![0u8; 16 * 16 * 2];
    payload.resize(payload.len().div_ceil(2880) * 2880, 0);
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(&header).unwrap();
    file.write_all(&payload).unwrap();
}

struct Server {
    dir: tempfile::TempDir,
    app: Router,
    /// The rig the catalog's flats came from.
    rig_uuid: String,
}

/// A server with a viewer and an editor, a registry file, and one catalog
/// holding a few flats from one rig.
fn server() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let database = root.join("rig.sqlite");
    let mut connection = Connection::open(&database).unwrap();
    psf_guard::ts_schema::apply_schema(&connection).unwrap();
    let mut flats = Vec::new();
    for index in 0..3 {
        let path = root.join(format!("flat-{index}.fits"));
        write_fits(&path, "FLAT", "2026-09-09T12:00:00");
        flats.push(psf_guard::commands::import::headers::read_frame_meta(&path));
    }
    let tx = connection.transaction().unwrap();
    psf_guard::calibration::import_calibration_frames(&tx, &flats, Some("p")).unwrap();
    tx.commit().unwrap();
    let rig_uuid: String = connection
        .query_row("SELECT rig_uuid FROM psf_guard_rig", [], |row| row.get(0))
        .unwrap();
    drop(connection);

    let mut users = AuthRegistry::default();
    users
        .add(
            AuthUserRecord::new("viewer", AccessRole::ReadOnly, "viewer-secret").unwrap(),
            false,
        )
        .unwrap();
    users
        .add(
            AuthUserRecord::new("editor", AccessRole::ReadWrite, "editor-secret").unwrap(),
            false,
        )
        .unwrap();
    let config = ServerAuthConfig {
        session_hours: Some(1),
        secure_cookie: Some(false),
        allow_read_only_compute: false,
    };
    let state = Arc::new(AppState::new_for_test(
        Connection::open_in_memory().unwrap(),
    ));
    state.set_server_auth(Some(
        ServerAuth::from_sources(Some(&config), &users, 3000)
            .unwrap()
            .unwrap(),
    ));
    state.set_registry_path(Some(root.join("config.json")));
    // Settings are not database management: no server flag needed.
    state.set_allow_database_management(false);
    let context = DatabaseContext::new(
        "rig".into(),
        "Observatory".into(),
        database.display().to_string(),
        vec![root.display().to_string()],
        None,
        None,
        None,
        root.join("cache"),
    )
    .unwrap();
    state
        .databases
        .write()
        .unwrap()
        .insert("rig".into(), Arc::new(context));

    let api = Router::new()
        .route("/auth/login", post(auth::login))
        .route(
            "/settings/calibration",
            get(calibration_settings::get_calibration_settings)
                .put(calibration_settings::update_calibration_settings),
        )
        .route(
            "/settings/calibration/rigs/{rig_uuid}",
            put(calibration_settings::update_rig_calibration_settings)
                .delete(calibration_settings::delete_rig_calibration_settings),
        )
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::authorize_api,
        ))
        .with_state(state);
    Server {
        dir,
        app: Router::new().nest("/api", api),
        rig_uuid,
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    cookie: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if !cookie.is_empty() {
        request = request.header(COOKIE, cookie);
    }
    let request = match body {
        Some(body) => request
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value)
}

async fn login(app: &Router, username: &str, password: &str) -> String {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "username": username, "password": password }).to_string(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .to_string()
}

const SETTINGS: &str = "/api/settings/calibration";

fn rig_path(rig_uuid: &str) -> String {
    format!("{SETTINGS}/rigs/{rig_uuid}")
}

/// Put the defaults back in force for the next test.
fn reset() {
    psf_guard::calibration::configure(None);
}

#[tokio::test]
async fn viewers_read_the_settings_and_only_editors_change_them() {
    let _turn = TURN.lock().await;
    let server = server();
    let viewer = login(&server.app, "viewer", "viewer-secret").await;
    let editor = login(&server.app, "editor", "editor-secret").await;

    let (status, body) = send(&server.app, Method::GET, SETTINGS, &viewer, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["rules"], json!({}), "nothing set");
    assert!(data["fields"].as_array().unwrap().len() > 30);
    let rig = data["rigs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rig| rig["rig_uuid"] == server.rig_uuid.as_str())
        .unwrap_or_else(|| panic!("the catalog's rig is listed: {body}"));
    assert_eq!(rig["name"], "SpaceCat61 · ASI2600MM");
    assert_eq!(rig["databases"], json!(["Observatory"]));
    assert_eq!(rig["frames"], 3);
    assert_eq!(rig["detail"], "16×16 · bin 1×1");

    let global = json!({ "rules": { "dark_temperature_tolerance_c": 2 } });
    let own = json!({ "name": "SpaceCat61 · ASI2600MM", "rules": { "check_no_flat": "off" } });
    let path = rig_path(&server.rig_uuid);
    for (method, uri, body) in [
        (Method::PUT, SETTINGS, Some(global.clone())),
        (Method::PUT, path.as_str(), Some(own.clone())),
        (Method::DELETE, path.as_str(), None),
    ] {
        let (status, _) = send(&server.app, method.clone(), uri, &viewer, body.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "a viewer's {method} {uri}");
        let (status, _) = send(&server.app, method.clone(), uri, "", body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "nobody's {method} {uri}");
    }
    assert_eq!(
        psf_guard::calibration::rule_book()
            .global()
            .dark_temperature_c,
        3.0
    );

    let (status, body) = send(&server.app, Method::PUT, SETTINGS, &editor, Some(global)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["rules"],
        json!({ "dark_temperature_tolerance_c": 2 })
    );
    let (status, body) = send(&server.app, Method::PUT, &path, &editor, Some(own)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = send(&server.app, Method::DELETE, &path, &editor, None).await;
    assert_eq!(status, StatusCode::OK);
    reset();
}

#[tokio::test]
async fn every_value_is_checked_against_its_setting() {
    let _turn = TURN.lock().await;
    let server = server();
    let editor = login(&server.app, "editor", "editor-secret").await;
    let path = rig_path(&server.rig_uuid);
    for (uri, body, says) in [
        (
            SETTINGS,
            json!({ "rules": { "dark_temperatur_c": 2 } }),
            "dark_temperatur_c",
        ),
        (
            SETTINGS,
            json!({ "rules": { "dark_temperature_tolerance_c": 5 } }),
            "from 0 to 3",
        ),
        (
            SETTINGS,
            json!({ "rules": { "thin_master_frames": "five" } }),
            "whole number",
        ),
        (
            SETTINGS,
            json!({ "rules": { "check_no_flat": "sometimes" } }),
            "flag, advisory or off",
        ),
        (
            SETTINGS,
            json!({ "rules": { "grade_flat_dim": { "suspect": 0.01, "poor": 0.05 } } }),
            "suspect before",
        ),
        (
            SETTINGS,
            json!({ "rotation_tolerance_deg": 200 }),
            "0 to 180",
        ),
        // Stacks hold a master flat to Seiza's 2° and 1 mm for now, so a
        // wider tolerance would match flats the final pass refuses.
        (
            SETTINGS,
            json!({ "rules": { "rotation_tolerance_deg": 5 } }),
            "can't be wider than 2° yet",
        ),
        (
            path.as_str(),
            json!({ "rules": { "focal_length_tolerance_mm": 5 } }),
            "can't be wider than 1 mm yet",
        ),
        (
            path.as_str(),
            json!({ "rules": { "flat_session_hours": 0 } }),
            "Flat session",
        ),
        (
            path.as_str(),
            json!({ "name": "x".repeat(201), "rules": {} }),
            "at most 200",
        ),
        (
            "/api/settings/calibration/rigs/not-a-rig",
            json!({ "rules": {} }),
            "not a rig",
        ),
    ] {
        let (status, body) = send(&server.app, Method::PUT, uri, &editor, Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri} {body}");
        assert!(
            body["error"].as_str().unwrap_or_default().contains(says),
            "{uri}: {body} should say {says}"
        );
    }
    // A key the request shape does not know is refused before any value is
    // looked at.
    for (uri, body) in [
        (SETTINGS, json!({ "rotation": 2 })),
        (path.as_str(), json!({ "rules": {}, "colour": "red" })),
    ] {
        let (status, _) = send(&server.app, Method::PUT, uri, &editor, Some(body)).await;
        assert!(status.is_client_error(), "{uri}: {status}");
    }
    // Nothing was saved.
    let (_, body) = send(&server.app, Method::GET, SETTINGS, &editor, None).await;
    assert_eq!(body["data"]["rules"], json!({}));
    assert!(body["data"]["rigs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|rig| rig["rules"] == json!({})));
    reset();
}

#[tokio::test]
async fn a_rigs_values_are_kept_beside_the_global_ones_and_put_in_force() {
    let _turn = TURN.lock().await;
    let server = server();
    let editor = login(&server.app, "editor", "editor-secret").await;
    let path = rig_path(&server.rig_uuid);

    let (status, body) = send(
        &server.app,
        Method::PUT,
        SETTINGS,
        &editor,
        Some(json!({
            "rules": {
                "flat_max_age_days": 90,
                "check_mixed_flats": "off",
                // The default is not written down.
                "thin_master_frames": 5,
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["rules"],
        json!({ "flat_max_age_days": 90, "check_mixed_flats": "off" })
    );
    // The legacy fields still read what they always did.
    assert_eq!(body["data"]["flat_max_age_days"], 90.0);

    let (status, body) = send(
        &server.app,
        Method::PUT,
        &path,
        &editor,
        Some(json!({
            "name": "SpaceCat61 · ASI2600MM",
            "rules": {
                "flat_max_age_days": 365,
                "grade_flat_illumination": { "suspect": 0.05, "poor": 0.1 },
                "use_darks": null,
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rig = body["data"]["rigs"][0].clone();
    assert_eq!(
        rig["rig_uuid"],
        server.rig_uuid.as_str(),
        "rigs with values first"
    );
    assert_eq!(
        rig["rules"],
        json!({
            "flat_max_age_days": 365,
            "grade_flat_illumination": { "suspect": 0.05, "poor": 0.1 },
        })
    );

    // In force at once: the rig's value, then the global, then the default.
    let book = psf_guard::calibration::rule_book();
    let rules = book.for_rig(&server.rig_uuid);
    assert_eq!(rules.flat_max_age_days, 365.0);
    assert_eq!(rules.flat.illumination.suspect, 0.05);
    assert_eq!(
        rules.mode(psf_guard::calibration::Check::MixedFlats),
        psf_guard::calibration::CheckMode::Off
    );
    assert_eq!(rules.dark_reach_days, 183.0);
    assert_eq!(book.for_rig("someone-else").flat_max_age_days, 90.0);

    // Kept in the registry, so a restart reads them back.
    let saved: Value = serde_json::from_str(
        &std::fs::read_to_string(server.dir.path().join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["calibration"]["flat_max_age_days"], 90);
    assert_eq!(
        saved["calibration"]["rigs"][&server.rig_uuid]["flat_max_age_days"],
        365
    );
    assert_eq!(
        saved["calibration"]["rigs"][&server.rig_uuid]["name"],
        "SpaceCat61 · ASI2600MM"
    );

    // Upper-case UUIDs are the same rig.
    let (status, body) = send(
        &server.app,
        Method::DELETE,
        &rig_path(&server.rig_uuid.to_uppercase()),
        &editor,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["rigs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|rig| rig["rules"] == json!({})));
    assert_eq!(
        psf_guard::calibration::rule_book()
            .for_rig(&server.rig_uuid)
            .flat_max_age_days,
        90.0,
        "back on the global value"
    );

    // An empty set of values is the same as no values.
    send(
        &server.app,
        Method::PUT,
        &path,
        &editor,
        Some(json!({ "rules": { "use_darks": false } })),
    )
    .await;
    let (_, body) = send(
        &server.app,
        Method::PUT,
        &path,
        &editor,
        Some(json!({ "rules": {} })),
    )
    .await;
    assert!(body["data"]["rigs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|rig| rig["rules"] == json!({})));

    // Clearing the global values leaves the registry without the block.
    send(
        &server.app,
        Method::PUT,
        SETTINGS,
        &editor,
        Some(json!({ "rules": { "flat_max_age_days": null, "check_mixed_flats": null } })),
    )
    .await;
    let saved: Value = serde_json::from_str(
        &std::fs::read_to_string(server.dir.path().join("config.json")).unwrap(),
    )
    .unwrap();
    assert!(saved.get("calibration").is_none(), "{saved}");
    reset();
}

/// A save that fails changes nothing in force: the settings go into force
/// only once the registry holds them, so a restart cannot take them back.
#[cfg(unix)]
#[tokio::test]
async fn a_change_the_registry_could_not_save_is_not_put_in_force() {
    use std::os::unix::fs::PermissionsExt;
    let _turn = TURN.lock().await;
    let server = server();
    let editor = login(&server.app, "editor", "editor-secret").await;
    // A registry that reads but cannot be written: its folder is read-only.
    let (status, body) = send(
        &server.app,
        Method::PUT,
        SETTINGS,
        &editor,
        Some(json!({ "rules": { "thin_master_frames": 4 } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let folder = server.dir.path().to_path_buf();
    let writable = std::fs::metadata(&folder).unwrap().permissions();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let (status, body) = send(
        &server.app,
        Method::PUT,
        SETTINGS,
        &editor,
        Some(json!({ "rules": { "thin_master_frames": 8 } })),
    )
    .await;
    let (rig_status, _) = send(
        &server.app,
        Method::PUT,
        &rig_path(&server.rig_uuid),
        &editor,
        Some(json!({ "rules": { "flat_max_age_days": 400 } })),
    )
    .await;
    std::fs::set_permissions(&folder, writable).unwrap();
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(rig_status, StatusCode::INTERNAL_SERVER_ERROR);
    // Still the saved value, for every rig.
    let book = psf_guard::calibration::rule_book();
    assert_eq!(book.global().thin_master_frames, 4);
    assert_eq!(book.for_rig(&server.rig_uuid).flat_max_age_days, 60.0);
    reset();
}

/// One editor cannot make the registry hold settings for any number of
/// rigs.
#[tokio::test]
async fn the_number_of_rigs_with_settings_of_their_own_is_bounded() {
    let _turn = TURN.lock().await;
    let server = server();
    let editor = login(&server.app, "editor", "editor-secret").await;
    for index in 0..256u32 {
        let (status, body) = send(
            &server.app,
            Method::PUT,
            &rig_path(&format!("00000000-0000-0000-0000-{index:012}")),
            &editor,
            Some(json!({ "rules": { "thin_master_frames": 4 } })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let (status, body) = send(
        &server.app,
        Method::PUT,
        &rig_path(&server.rig_uuid),
        &editor,
        Some(json!({ "rules": { "thin_master_frames": 3 } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("at most 256 rigs"),
        "{body}"
    );
    // A rig already listed may still change.
    let (status, body) = send(
        &server.app,
        Method::PUT,
        &rig_path("00000000-0000-0000-0000-000000000007"),
        &editor,
        Some(json!({ "rules": { "thin_master_frames": 3 } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    reset();
}
