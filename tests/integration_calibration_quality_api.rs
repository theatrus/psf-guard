//! Calibration frame grades over HTTP: the progress route counts the
//! library's grades, the library listing carries each frame's grade, and a
//! person's choice to use a poor frame or keep a good one out is refused when
//! malformed and recorded when not. Changing the library takes an editor,
//! not the database-management gate.

use std::io::Write as _;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::{delete, get, put},
    Router,
};
use http_body_util::BodyExt;
use psf_guard::server::{
    calibration_quality, database_context::DatabaseContext, handlers, state::AppState,
};
use rusqlite::Connection;
use serde_json::{json, Value};
use tower::ServiceExt;

/// A 64x64 unsigned 16-bit dark at `level` with a little noise.
fn write_dark(path: &std::path::Path, level: f64, seed: u64) {
    let cards = [
        "SIMPLE  =                    T".to_string(),
        "BITPIX  =                   16".to_string(),
        "NAXIS   =                    2".to_string(),
        "NAXIS1  =                   64".to_string(),
        "NAXIS2  =                   64".to_string(),
        "BZERO   =                32768".to_string(),
        "IMAGETYP= 'DARK'".to_string(),
        "DATE-OBS= '2026-09-10T10:00:00'".to_string(),
        "EXPTIME =                300.0".to_string(),
        "GAIN    =                  100".to_string(),
        "OFFSET  =                   20".to_string(),
        "XBINNING=                    1".to_string(),
        "YBINNING=                    1".to_string(),
        "CCD-TEMP=                -10.0".to_string(),
        "INSTRUME= 'Camera'".to_string(),
        "END".to_string(),
    ];
    let mut header = Vec::new();
    for card in cards {
        let mut bytes = card.into_bytes();
        bytes.resize(80, b' ');
        header.extend(bytes);
    }
    header.resize(header.len().div_ceil(2880) * 2880, b' ');
    let mut payload = Vec::new();
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    for _ in 0..64 * 64 {
        // Uniform noise of about five counts' spread.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let jitter = (state % 17) as f64 - 8.0;
        let value = (level + jitter) as u16;
        payload.extend(((i32::from(value) - 32_768) as i16).to_be_bytes());
    }
    payload.resize(payload.len().div_ceil(2880) * 2880, 0);
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(&header).unwrap();
    file.write_all(&payload).unwrap();
}

struct Server {
    _dir: tempfile::TempDir,
    database: std::path::PathBuf,
    state: Arc<AppState>,
    app: Router,
}

fn server() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let database = root.join("rig.sqlite");
    let mut connection = Connection::open(&database).unwrap();
    psf_guard::ts_schema::apply_schema(&connection).unwrap();
    let mut darks = Vec::new();
    for (index, level) in [503.0, 503.0, 503.0, 900.0].into_iter().enumerate() {
        let path = root.join(format!("dark-{index}.fits"));
        write_dark(&path, level, index as u64 + 1);
        darks.push(psf_guard::commands::import::headers::read_frame_meta(&path));
    }
    let tx = connection.transaction().unwrap();
    psf_guard::calibration::import_calibration_frames(&tx, &darks, Some("p")).unwrap();
    tx.commit().unwrap();
    drop(connection);

    let state = Arc::new(AppState::new_for_test(
        Connection::open_in_memory().unwrap(),
    ));
    let context = DatabaseContext::new(
        "rig".into(),
        "Rig".into(),
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
    let app = Router::new()
        .route(
            "/api/db/{db_id}/calibrations/details",
            get(handlers::get_calibration_library_details),
        )
        .route(
            "/api/db/{db_id}/calibrations/frames/quality",
            put(handlers::set_calibration_quality_override),
        )
        .route(
            "/api/db/{db_id}/calibrations/quality",
            get(calibration_quality::get_progress),
        )
        .route(
            "/api/db/{db_id}/calibrations/frames/validity",
            put(handlers::set_calibration_validity),
        )
        .route(
            "/api/db/{db_id}/calibrations/frames/{frame_uuid}",
            delete(handlers::forget_calibration_frame),
        )
        .route(
            "/api/db/{db_id}/calibrations/frames",
            delete(handlers::forget_calibration_frames),
        )
        .route(
            "/api/db/{db_id}/calibrations/masters",
            delete(handlers::clear_calibration_masters),
        )
        .with_state(Arc::clone(&state));
    state.set_allow_database_management(true);
    Server {
        _dir: dir,
        database,
        state,
        app,
    }
}

async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    let body = if body.is_null() {
        Body::empty()
    } else {
        request = request.header("content-type", "application/json");
        Body::from(body.to_string())
    };
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

const DETAILS: &str = "/api/db/rig/calibrations/details";
const QUALITY: &str = "/api/db/rig/calibrations/quality";
const CHOICE: &str = "/api/db/rig/calibrations/frames/quality";
const VALIDITY: &str = "/api/db/rig/calibrations/frames/validity";
const FRAMES: &str = "/api/db/rig/calibrations/frames";
const MASTERS: &str = "/api/db/rig/calibrations/masters";

fn frame<'a>(details: &'a Value, name: &str) -> &'a Value {
    details["data"]["frames"]
        .as_array()
        .unwrap()
        .iter()
        .find(|frame| frame["source_path"].as_str().unwrap().ends_with(name))
        .unwrap()
}

#[tokio::test]
async fn grades_are_counted_listed_and_overridden() {
    let server = server();
    let (status, body) = call(&server.app, "GET", QUALITY, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["counts"]["frames"], 4);
    assert_eq!(body["data"]["counts"]["measured"], 0);
    assert_eq!(body["data"]["progress"]["running"], false);

    // What the background pass does, on the catalog itself.
    let connection = Connection::open(&server.database).unwrap();
    psf_guard::calibration::measure_and_grade(&connection).unwrap();
    drop(connection);

    let (_, body) = call(&server.app, "GET", QUALITY, Value::Null).await;
    let counts = &body["data"]["counts"];
    assert_eq!(
        (counts["measured"].as_u64(), counts["poor"].as_u64()),
        (Some(4), Some(1))
    );
    assert_eq!(counts["ok"], 3);

    let (_, details) = call(&server.app, "GET", DETAILS, Value::Null).await;
    let leaked = frame(&details, "dark-3.fits");
    assert_eq!(leaked["grade"], "poor", "{leaked}");
    assert_eq!(leaked["grade_code"], "level_excess");
    assert!(leaked["kept_out"]
        .as_str()
        .unwrap()
        .starts_with("stray light"));
    let leaked_uuid = leaked["frame_uuid"].as_str().unwrap().to_string();
    let quiet_uuid = frame(&details, "dark-0.fits")["frame_uuid"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, _) = call(
        &server.app,
        "PUT",
        CHOICE,
        json!({"frame_uuids": [leaked_uuid], "choice": "maybe"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &server.app,
        "PUT",
        CHOICE,
        json!({"frame_uuids": [], "choice": "use"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &server.app,
        "PUT",
        CHOICE,
        json!({"frame_uuids": ["no-such-frame"], "choice": "use"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = call(
        &server.app,
        "PUT",
        CHOICE,
        json!({"frame_uuids": [leaked_uuid], "choice": "use"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["updated"], 1);
    let (status, _) = call(
        &server.app,
        "PUT",
        CHOICE,
        json!({"frame_uuids": [quiet_uuid], "choice": "exclude"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, details) = call(&server.app, "GET", DETAILS, Value::Null).await;
    let leaked = frame(&details, "dark-3.fits");
    assert_eq!(leaked["quality_override"], "use");
    assert_eq!(
        leaked["grade"], "poor",
        "the grade stands beside the choice"
    );
    assert!(leaked["kept_out"].is_null());
    let quiet = frame(&details, "dark-0.fits");
    assert_eq!(quiet["quality_override"], "exclude");
    assert_eq!(quiet["kept_out"], "by hand");

    let (_, body) = call(&server.app, "GET", QUALITY, Value::Null).await;
    assert_eq!(body["data"]["counts"]["kept_out_by_hand"], 1);
    assert_eq!(body["data"]["counts"]["used_anyway"], 1);

    let (status, _) = call(
        &server.app,
        "PUT",
        CHOICE,
        json!({"frame_uuids": [leaked_uuid, quiet_uuid], "choice": "clear"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, details) = call(&server.app, "GET", DETAILS, Value::Null).await;
    assert!(frame(&details, "dark-0.fits")["quality_override"].is_null());
    assert!(frame(&details, "dark-3.fits")["kept_out"].is_string());
}

/// The real API with user accounts, as the server wires it, over the same
/// catalog: a viewer and an editor, database management off.
async fn signed_in(server: &Server) -> (Router, String, String, String) {
    use psf_guard::auth_registry::{AccessRole, AuthRegistry, AuthUserRecord};
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
    let registry = server._dir.path().join("config.json");
    users
        .save(&AuthRegistry::path_for_database_registry(&registry))
        .unwrap();
    let config = psf_guard::config::ServerAuthConfig {
        session_hours: Some(1),
        secure_cookie: Some(false),
        allow_read_only_compute: false,
    };
    server.state.set_server_auth(Some(
        psf_guard::server::auth::ServerAuth::from_sources(Some(&config), &users, 3000)
            .unwrap()
            .unwrap(),
    ));
    server.state.set_registry_path(Some(registry));
    server.state.set_allow_database_management(false);
    let app = Router::new().nest(
        "/api",
        psf_guard::server::api_router(Arc::clone(&server.state)),
    );
    let login = |username: &'static str, password: &'static str| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/auth/login")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({"username": username, "password": password}).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            response.headers()["set-cookie"]
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_string()
        }
    };
    let viewer = login("viewer", "viewer-secret").await;
    let editor = login("editor", "editor-secret").await;
    let minted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/tokens")
                .header("cookie", &editor)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"label": "script", "read_only": true}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(minted.status(), StatusCode::OK);
    let bytes = minted.into_body().collect().await.unwrap().to_bytes();
    let minted: Value = serde_json::from_slice(&bytes).unwrap();
    let token = minted["data"]["token"].as_str().unwrap().to_string();
    (app, viewer, editor, token)
}

/// Send as a signed-in user (`cookie`) or with a bearer token.
async fn send_as(
    app: &Router,
    who: (&str, &str),
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    request = request.header(who.0, who.1);
    let body = if body.is_null() {
        Body::empty()
    } else {
        request = request.header("content-type", "application/json");
        Body::from(body.to_string())
    };
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn editors_change_the_library_without_database_management_and_viewers_cannot() {
    // Keep out, validity marks, Forget and Clear masters edit catalog rows
    // and the masters PSF Guard made: an editor's work, like grading. The
    // database-management gate guards paths a client names, not these.
    let server = server();
    let (app, viewer, editor, token) = signed_in(&server).await;
    let bearer = format!("Bearer {token}");
    let (_, details) = send_as(&app, ("cookie", &editor), "GET", DETAILS, Value::Null).await;
    let uuid = |name: &str| {
        frame(&details, name)["frame_uuid"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (first, second) = (uuid("dark-0.fits"), uuid("dark-1.fits"));
    let writes = [
        (
            "PUT",
            CHOICE.to_string(),
            json!({"frame_uuids": [first], "choice": "exclude"}),
        ),
        (
            "PUT",
            VALIDITY.to_string(),
            json!({"frame_uuids": [first], "direction": "forward"}),
        ),
        ("DELETE", MASTERS.to_string(), Value::Null),
        (
            "DELETE",
            FRAMES.to_string(),
            json!({"frame_uuids": [second]}),
        ),
        ("DELETE", format!("{FRAMES}/{first}"), Value::Null),
    ];

    for who in [
        ("cookie", viewer.as_str()),
        ("authorization", bearer.as_str()),
    ] {
        for (method, uri, body) in &writes {
            let (status, response) = send_as(&app, who, method, uri, body.clone()).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {response}");
            assert!(
                response["error"]
                    .as_str()
                    .unwrap()
                    .contains("read-only access"),
                "{method} {uri}: {response}"
            );
        }
    }
    let (_, unchanged) = send_as(&app, ("cookie", &editor), "GET", DETAILS, Value::Null).await;
    assert_eq!(unchanged["data"]["frames"].as_array().unwrap().len(), 4);
    assert!(frame(&unchanged, "dark-0.fits")["quality_override"].is_null());

    assert!(!server.state.database_management_allowed());
    for (method, uri, body) in &writes {
        let (status, response) =
            send_as(&app, ("cookie", &editor), method, uri, body.clone()).await;
        assert_eq!(status, StatusCode::OK, "{method} {uri}: {response}");
    }
    let (_, changed) = send_as(&app, ("cookie", &editor), "GET", DETAILS, Value::Null).await;
    assert_eq!(changed["data"]["frames"].as_array().unwrap().len(), 2);
}
