//! Rejecting the lights the library cannot calibrate, over HTTP: the check
//! lists them with a digest, a stale digest is refused, and the apply sets
//! Rejected with the reason so Target Scheduler shoots them again.

use std::io::Write as _;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::get,
    Router,
};
use http_body_util::BodyExt;
use psf_guard::server::{database_context::DatabaseContext, handlers, state::AppState};
use rusqlite::Connection;
use serde_json::{json, Value};
use tower::ServiceExt;

/// A tiny FITS frame with the readings calibration matching compares.
fn write_fits(path: &std::path::Path, kind: &str, rotation: f64) {
    let cards = [
        "SIMPLE  =                    T".to_string(),
        "BITPIX  =                   16".to_string(),
        "NAXIS   =                    2".to_string(),
        "NAXIS1  =                    4".to_string(),
        "NAXIS2  =                    4".to_string(),
        format!("IMAGETYP= '{kind}'"),
        "FILTER  = 'Ha'".to_string(),
        "EXPTIME =                300.0".to_string(),
        "GAIN    =                  100".to_string(),
        "OFFSET  =                   20".to_string(),
        "XBINNING=                    1".to_string(),
        "YBINNING=                    1".to_string(),
        "CCD-TEMP=                -10.0".to_string(),
        "TELESCOP= 'Scope'".to_string(),
        "INSTRUME= 'Camera'".to_string(),
        format!("ROTATANG= {rotation:20.1}"),
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
    for _ in 0..16 {
        payload.extend(1_000i16.to_be_bytes());
    }
    payload.resize(2880, 0);
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
    // Two lights at the flats' angle, one turned away from them.
    for (id, rotation) in [(1, 94.7), (2, 94.7), (3, 120.0)] {
        let name = format!("M42_Ha_{id:03}.fits");
        write_fits(&images.join(&name), "LIGHT", rotation);
        connection
            .execute(
                "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid)
                 VALUES (?1, 1, 1, ?2, 'Ha', 1, ?3, ?4)",
                rusqlite::params![
                    id,
                    1_759_300_000 + id * 300,
                    json!({"FileName": name}).to_string(),
                    format!("light-{id}")
                ],
            )
            .unwrap();
    }
    let mut flats = Vec::new();
    for index in 0..3 {
        let path = root.join(format!("flat-{index}.fits"));
        write_fits(&path, "FLAT", 94.7);
        flats.push(psf_guard::commands::import::headers::read_frame_meta(&path));
    }
    let tx = connection.transaction().unwrap();
    psf_guard::calibration::import_calibration_frames(&tx, &flats, Some("p")).unwrap();
    tx.commit().unwrap();
    drop(connection);

    let state = Arc::new(AppState::new_for_test(
        Connection::open_in_memory().unwrap(),
    ));
    let context = DatabaseContext::new(
        "rig".into(),
        "Rig".into(),
        database.display().to_string(),
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
            "/api/db/{db_id}/projects/{project_id}/calibration-report/rejects",
            get(handlers::get_project_calibration_gaps).post(handlers::reject_uncalibrated_lights),
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

const REJECTS: &str = "/api/db/rig/projects/1/calibration-report/rejects";

#[tokio::test]
async fn lights_the_library_cannot_calibrate_are_listed_then_rejected() {
    let server = server();
    let (status, body) = call(&server.app, "GET", REJECTS, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let check = &body["data"];
    assert_eq!(check["checked"], 3);
    assert_eq!(check["lights"].as_array().unwrap().len(), 1, "{check}");
    assert_eq!(check["lights"][0]["image_id"], 3);
    assert_eq!(
        check["lights"][0]["reason"],
        "No matching flat: the nearest Ha flats are 25° off its rotation"
    );
    let digest = check["digest"].as_str().unwrap().to_string();

    let (status, _) = call(
        &server.app,
        "POST",
        REJECTS,
        json!({"digest": "not-the-list-shown"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, body) = call(&server.app, "POST", REJECTS, json!({"digest": digest})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["updated"], 1);
    assert_eq!(body["data"]["previous"][0]["status"], "accepted");

    let connection = Connection::open(&server.database).unwrap();
    let (grade, reason): (i32, String) = connection
        .query_row(
            "SELECT gradingStatus, rejectreason FROM acquiredimage WHERE Id = 3",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(grade, 2);
    assert_eq!(
        reason,
        "Cannot be calibrated: No matching flat: the nearest Ha flats are 25° off its rotation"
    );

    // A rejected light is not listed again.
    let (_, body) = call(&server.app, "GET", REJECTS, Value::Null).await;
    assert_eq!(body["data"]["lights"].as_array().unwrap().len(), 0);
    assert_eq!(body["data"]["checked"], 2);
}
