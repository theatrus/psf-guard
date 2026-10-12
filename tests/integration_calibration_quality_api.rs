//! Calibration frame grades over HTTP: the progress route counts the
//! library's grades, the library listing carries each frame's grade, and a
//! person's choice to use a poor frame or keep a good one out is refused when
//! malformed and recorded when not.

use std::io::Write as _;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::{get, put},
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
        .with_state(state);
    Server {
        _dir: dir,
        database,
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
