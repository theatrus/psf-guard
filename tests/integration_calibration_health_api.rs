//! Calibration health over HTTP: the Library's list and a project's detail
//! come from the background pass's kept results only, a result whose key no
//! longer matches the catalog is not shown, and the next pass replaces it.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::get,
    Router,
};
use http_body_util::BodyExt;
use psf_guard::server::{calibration_health, database_context::DatabaseContext, state::AppState};
use rusqlite::Connection;
use serde_json::{json, Value};
use tower::ServiceExt;

/// A 64x64 unsigned 16-bit frame from `camera` at `level`.
fn write_fits(path: &std::path::Path, kind: &str, camera: &str, captured: &str, level: u16) {
    let cards = [
        "SIMPLE  =                    T".to_string(),
        "BITPIX  =                   16".to_string(),
        "NAXIS   =                    2".to_string(),
        "NAXIS1  =                   64".to_string(),
        "NAXIS2  =                   64".to_string(),
        "BZERO   =                32768".to_string(),
        format!("IMAGETYP= '{kind}'"),
        format!("DATE-OBS= '{captured}'"),
        "FILTER  = 'Ha'".to_string(),
        "EXPTIME =                300.0".to_string(),
        "GAIN    =                  100".to_string(),
        "OFFSET  =                   20".to_string(),
        "XBINNING=                    1".to_string(),
        "YBINNING=                    1".to_string(),
        "CCD-TEMP=                -10.0".to_string(),
        format!("INSTRUME= '{camera}'"),
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
    for index in 0..64 * 64u32 {
        let value = level + (index * 7 % 11) as u16;
        payload.extend(((i32::from(value) - 32_768) as i16).to_be_bytes());
    }
    payload.resize(payload.len().div_ceil(2880) * 2880, 0);
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(&header).unwrap();
    file.write_all(&payload).unwrap();
}

struct Server {
    _dir: tempfile::TempDir,
    database: String,
    state: Arc<AppState>,
    app: Router,
}

/// One project whose lights come from a camera the library has nothing
/// for, beside a library of bias frames from another camera.
fn server() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let images = root.join("images");
    std::fs::create_dir_all(&images).unwrap();
    let database = root.join("rig.sqlite");
    let mut connection = Connection::open(&database).unwrap();
    psf_guard::ts_schema::apply_schema(&connection).unwrap();
    connection
        .execute_batch(
            r#"INSERT INTO project (Id, profileId, name, guid) VALUES (1, 'p', 'M42', 'pg');
             INSERT INTO target (Id, name, active, epochcode, projectId, guid) VALUES (1, 'M42', 1, 0, 1, 'tg');"#,
        )
        .unwrap();
    for id in 1..=2 {
        let name = format!("M42_Ha_{id:03}.fits");
        write_fits(
            &images.join(&name),
            "LIGHT",
            "ASCOM1",
            "2026-09-10T04:00:00",
            1_000,
        );
        connection
            .execute(
                "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid)
                 VALUES (?1, 1, 1, ?2, 'Ha', 0, ?3, ?4)",
                rusqlite::params![
                    id,
                    1_789_012_800 + id * 300,
                    json!({"FileName": name}).to_string(),
                    format!("light-{id}")
                ],
            )
            .unwrap();
    }
    let mut bias = Vec::new();
    for index in 0..3 {
        let path = root.join(format!("bias-{index}.fits"));
        write_fits(&path, "BIAS", "Camera", "2026-09-09T12:00:00", 500);
        bias.push(psf_guard::commands::import::headers::read_frame_meta(&path));
    }
    let tx = connection.transaction().unwrap();
    psf_guard::calibration::import_calibration_frames(&tx, &bias, Some("p")).unwrap();
    tx.commit().unwrap();
    drop(connection);

    let state = Arc::new(AppState::new_for_test(
        Connection::open_in_memory().unwrap(),
    ));
    let database = database.display().to_string();
    let context = DatabaseContext::new(
        "rig".into(),
        "Rig".into(),
        database.clone(),
        vec![images.display().to_string()],
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
            "/api/db/{db_id}/calibration-health",
            get(calibration_health::get_health),
        )
        .route(
            "/api/db/{db_id}/projects/{project_id}/calibration-health",
            get(calibration_health::get_project_health),
        )
        .with_state(Arc::clone(&state));
    Server {
        _dir: dir,
        database,
        state,
        app,
    }
}

async fn call(app: &Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

const LIST: &str = "/api/db/rig/calibration-health";
const DETAIL: &str = "/api/db/rig/projects/1/calibration-health";

/// The Library's list once a pass has put project 1 in it.
async fn wait_for_project(app: &Router) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (status, body) = call(app, LIST).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if let Some(project) = body["data"]["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|project| project["project_id"] == 1)
        {
            return project.clone();
        }
        assert!(Instant::now() < deadline, "no result: {body}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn health_is_shown_only_once_worked_out_and_while_it_still_holds() {
    let server = server();
    // Nothing is worked out on a request.
    let (_, body) = call(&server.app, LIST).await;
    assert_eq!(body["data"]["projects"], json!([]));
    let (_, body) = call(&server.app, DETAIL).await;
    assert!(body["data"]["health"].is_null());

    calibration_health::request(&server.state, &server.database);
    let project = wait_for_project(&server.app).await;
    assert_eq!(project["level"], "poor");
    assert_eq!(project["poor_lights"], 2);
    assert_eq!(project["checked"], 2);

    let (_, body) = call(&server.app, DETAIL).await;
    let health = &body["data"]["health"];
    assert_eq!(health["findings"][0]["code"], "no_library");
    assert_eq!(
        health["findings"][0]["reason"],
        "No calibration frames match its camera and settings"
    );
    assert_eq!(health["findings"][0]["lights"], 2);
    assert_eq!(health["findings"][0]["nights"][0]["filter"], "Ha");

    // A pass with nothing changed judges nothing again.
    calibration_health::request(&server.state, &server.database);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (_, body) = call(&server.app, LIST).await;
        if body["data"]["running"] == false {
            assert_eq!(
                body["data"]["projects"][0]["computed_at"],
                project["computed_at"]
            );
            break;
        }
        assert!(Instant::now() < deadline, "the pass never ended");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // A light rejected since: the kept result no longer holds, so it is
    // not shown until the next pass.
    let connection = Connection::open(&server.database).unwrap();
    connection
        .execute(
            "UPDATE acquiredimage SET gradingStatus = 2 WHERE Id = 2",
            [],
        )
        .unwrap();
    drop(connection);
    let (_, body) = call(&server.app, LIST).await;
    assert_eq!(body["data"]["projects"], json!([]));
    calibration_health::request(&server.state, &server.database);
    let project = wait_for_project(&server.app).await;
    assert_eq!(project["poor_lights"], 1);
}
