//! Calibration health over HTTP: the Library's list and a project's detail
//! come from the background pass's kept results only, a result whose key no
//! longer matches the catalog is not shown, and a view that finds one asks
//! for a pass, though not more often than every couple of minutes. A pass judges only what changed, reads only the headers of
//! lights whose file changed, and a directory refresh that finds a light's
//! file again judges its project again.

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
        .route(
            "/api/db/{db_id}/calibration-health/lights",
            get(calibration_health::get_light_marks),
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
const LIGHTS: &str = "/api/db/rig/calibration-health/lights";

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

/// Wait until no pass runs over the catalog.
async fn wait_for_quiet(server: &Server) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (_, body) = call(&server.app, LIST).await;
        if body["data"]["running"] == false {
            return;
        }
        assert!(Instant::now() < deadline, "the pass never ended");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Project 1's detail once a pass has judged it as `ready` wants.
async fn wait_for_detail(app: &Router, ready: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (_, body) = call(app, DETAIL).await;
        let health = &body["data"]["health"];
        if !health.is_null() && ready(health) {
            return health.clone();
        }
        assert!(Instant::now() < deadline, "no such result: {body}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn health_is_shown_only_once_worked_out_and_while_it_still_holds() {
    let server = server();
    // Nothing is worked out on a request: the view asks for a pass, which
    // waits here while interactive work runs.
    let busy = server.state.begin_interactive_job();
    let (_, body) = call(&server.app, LIST).await;
    assert_eq!(body["data"]["projects"], json!([]));
    let (_, body) = call(&server.app, DETAIL).await;
    assert!(body["data"]["health"].is_null());
    assert_eq!(body["data"]["running"], true, "the view asked for a pass");
    drop(busy);

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
    assert_eq!(health["findings"][0]["image_ids"], json!([1, 2]));

    // The image views read each light's mark.
    let (_, body) = call(&server.app, LIGHTS).await;
    let marked = json!([
        { "image_id": 1, "level": "poor", "codes": ["no_library"] },
        { "image_id": 2, "level": "poor", "codes": ["no_library"] },
    ]);
    assert_eq!(body["data"]["lights"], marked);
    assert_eq!(body["data"]["stale_projects"], json!([]));
    let (_, body) = call(&server.app, &format!("{LIGHTS}?project_id=2")).await;
    assert_eq!(body["data"]["lights"], json!([]));

    // A pass with nothing changed judges nothing again.
    wait_for_quiet(&server).await;
    let work = calibration_health::work_done(&server.database);
    assert_eq!((work.projects, work.headers_read), (1, 2));
    calibration_health::request(&server.state, &server.database);
    wait_for_quiet(&server).await;
    assert_eq!(calibration_health::work_done(&server.database), work);

    // A light rejected since, as the CLI does it: the kept result no longer
    // holds, so it is not shown until the next pass replaces it. The view
    // asked for a pass a moment ago, so it does not ask again so soon.
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
    assert_eq!(body["data"]["running"], false, "views ask at most so often");
    // A reject in the app asks for a pass itself.
    let busy = server.state.begin_interactive_job();
    calibration_health::request(&server.state, &server.database);
    // The marks keep the last result, and say it is stale, so the grid
    // does not lose them while the next pass runs.
    let (_, body) = call(&server.app, LIGHTS).await;
    assert_eq!(body["data"]["lights"], marked);
    assert_eq!(body["data"]["stale_projects"], json!([1]));
    drop(busy);
    let project = wait_for_project(&server.app).await;
    assert_eq!(project["poor_lights"], 1);
    let (_, body) = call(&server.app, LIGHTS).await;
    assert_eq!(body["data"]["lights"][0]["image_id"], 1);
    assert_eq!(body["data"]["lights"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["stale_projects"], json!([]));
    // The light still judged kept its header: nothing was read again.
    wait_for_quiet(&server).await;
    let after = calibration_health::work_done(&server.database);
    assert_eq!(after.projects, work.projects + 1);
    assert_eq!(after.headers_read, work.headers_read);
}

#[tokio::test]
async fn a_light_found_again_is_judged_again() {
    let server = server();
    let context = server.state.databases.read().unwrap()["rig"].clone();
    calibration_health::request(&server.state, &server.database);
    let health = wait_for_detail(&server.app, |health| health["checked"] == 2).await;
    assert_eq!(health["missing_files"], 0);

    // The share drops one light, and a directory refresh notices: the
    // catalog is unchanged, but the project is judged again.
    let images = server._dir.path().join("images");
    let aside = server._dir.path().join("M42_Ha_001.fits");
    std::fs::rename(images.join("M42_Ha_001.fits"), &aside).unwrap();
    context.clear_directory_tree_cache();
    context.get_directory_tree().unwrap();
    let health = wait_for_detail(&server.app, |health| health["missing_files"] == 1).await;
    assert_eq!(health["checked"], 1);

    // It comes back: the next refresh finds it, and nothing is missing.
    std::fs::rename(&aside, images.join("M42_Ha_001.fits")).unwrap();
    context.clear_directory_tree_cache();
    context.get_directory_tree().unwrap();
    let health = wait_for_detail(&server.app, |health| health["missing_files"] == 0).await;
    assert_eq!(health["checked"], 2);
}
