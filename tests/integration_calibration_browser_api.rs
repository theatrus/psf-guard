//! The calibration browser over HTTP: frames and sessions a page at a time
//! with every filter, one frame's detail, a frame picture answered 202 while
//! it is drawn and served after, a flat against its session, and the
//! library's masters with their pictures and files.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::{get, post},
    Router,
};
use http_body_util::BodyExt;
use psf_guard::server::{calibration_browser, database_context::DatabaseContext, state::AppState};
use rusqlite::Connection;
use serde_json::{json, Value};
use tower::ServiceExt;

/// Settings of one synthetic frame.
struct Frame<'a> {
    kind: &'a str,
    captured: &'a str,
    exposure: f64,
    filter: &'a str,
}

/// A 96x96 unsigned 16-bit FITS frame.
fn write_frame(path: &Path, frame: &Frame<'_>, pixel: impl Fn(usize, usize) -> f64) {
    let cards = [
        "SIMPLE  =                    T".to_string(),
        "BITPIX  =                   16".to_string(),
        "NAXIS   =                    2".to_string(),
        "NAXIS1  =                   96".to_string(),
        "NAXIS2  =                   96".to_string(),
        "BZERO   =                32768".to_string(),
        format!("IMAGETYP= '{}'", frame.kind),
        format!("DATE-OBS= '{}'", frame.captured),
        format!("EXPTIME = {:20.1}", frame.exposure),
        format!("FILTER  = '{}'", frame.filter),
        "GAIN    =                  100".to_string(),
        "OFFSET  =                   20".to_string(),
        "XBINNING=                    1".to_string(),
        "YBINNING=                    1".to_string(),
        "CCD-TEMP=                -10.0".to_string(),
        "TELESCOP= 'Scope'".to_string(),
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
    for y in 0..96 {
        for x in 0..96 {
            let value = pixel(x, y).round().clamp(0.0, 65_535.0) as i32;
            payload.extend(((value - 32_768) as i16).to_be_bytes());
        }
    }
    payload.resize(payload.len().div_ceil(2880) * 2880, 0);
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(&header).unwrap();
    file.write_all(&payload).unwrap();
}

/// A little deterministic noise of about `spread` counts.
fn jitter(x: usize, y: usize, seed: u64, spread: f64) -> f64 {
    let mut state = seed
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(((y as u64) << 32) | x as u64);
    state ^= state >> 30;
    state = state.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    state ^= state >> 27;
    ((state >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * spread
}

fn dust(x: usize, y: usize) -> bool {
    (x as f64 - 24.0).hypot(y as f64 - 24.0) < 5.0
}

fn droplet(x: usize, y: usize) -> bool {
    (x as f64 - 66.0).hypot(y as f64 - 60.0) < 6.0
}

struct Server {
    _dir: tempfile::TempDir,
    root: PathBuf,
    database: PathBuf,
    app: Router,
}

fn server() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let frames = root.join("frames");
    std::fs::create_dir_all(&frames).unwrap();
    let database = root.join("rig.sqlite");
    let mut connection = Connection::open(&database).unwrap();
    psf_guard::ts_schema::apply_schema(&connection).unwrap();
    let mut written = Vec::new();
    let mut write = |name: &str, frame: Frame<'_>, pixel: &dyn Fn(usize, usize) -> f64| {
        let path = frames.join(name);
        write_frame(&path, &frame, pixel);
        written.push(path);
    };
    // Four Ha flats, all with the same dust; the third with a droplet.
    for index in 0..4u64 {
        let level = 24_000.0 + index as f64 * 400.0;
        let pixel = move |x: usize, y: usize| {
            let mut value = level * (1.0 - 0.0015 * x as f64) + jitter(x, y, index, 40.0);
            if dust(x, y) {
                value *= 0.9;
            }
            if index == 2 && droplet(x, y) {
                value *= 0.8;
            }
            value
        };
        write(
            &format!("flat-ha-{index}.fits"),
            Frame {
                kind: "FLAT",
                captured: &format!("2026-09-11T04:0{index}:00"),
                exposure: 2.0 + index as f64 * 0.1,
                filter: "Ha",
            },
            &pixel,
        );
    }
    for index in 0..3u64 {
        let pixel = move |x: usize, y: usize| 30_000.0 + jitter(x, y, 10 + index, 40.0);
        write(
            &format!("flat-oiii-{index}.fits"),
            Frame {
                kind: "FLAT",
                captured: &format!("2026-09-13T04:0{index}:00"),
                exposure: 3.0,
                filter: "OIII",
            },
            &pixel,
        );
    }
    for (index, exposure) in [300.0, 300.0, 300.0, 60.0, 60.0].into_iter().enumerate() {
        let pixel = move |x: usize, y: usize| 503.0 + jitter(x, y, 20 + index as u64, 10.0);
        write(
            &format!("dark-{index}.fits"),
            Frame {
                kind: "DARK",
                captured: &format!("2026-09-11T06:0{index}:00"),
                exposure,
                filter: "",
            },
            &pixel,
        );
    }
    for index in 0..2u64 {
        let pixel = move |x: usize, y: usize| 500.0 + jitter(x, y, 30 + index, 10.0);
        write(
            &format!("bias-{index}.fits"),
            Frame {
                kind: "BIAS",
                captured: &format!("2026-09-11T06:3{index}:00"),
                exposure: 0.0,
                filter: "",
            },
            &pixel,
        );
    }
    let metas: Vec<_> = written
        .iter()
        .map(|path| psf_guard::commands::import::headers::read_frame_meta(path))
        .collect();
    let tx = connection.transaction().unwrap();
    psf_guard::calibration::import_calibration_frames(&tx, &metas, Some("p")).unwrap();
    tx.execute(
        "INSERT INTO project (Id, profileId, name, guid) VALUES (1, 'p', 'Heart', 'pg')",
        [],
    )
    .unwrap();
    tx.commit().unwrap();
    psf_guard::calibration::measure_and_grade(&connection).unwrap();
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
            "/api/db/{db_id}/calibrations/frames",
            get(calibration_browser::list_frames),
        )
        .route(
            "/api/db/{db_id}/calibrations/sessions",
            get(calibration_browser::list_sessions),
        )
        .route(
            "/api/db/{db_id}/calibrations/frames/{frame_uuid}",
            get(calibration_browser::get_frame),
        )
        .route(
            "/api/db/{db_id}/calibrations/frames/{frame_uuid}/preview",
            get(calibration_browser::get_frame_preview),
        )
        .route(
            "/api/db/{db_id}/calibrations/generation-status",
            post(calibration_browser::post_generation_status),
        )
        .route(
            "/api/db/{db_id}/calibrations/masters",
            get(calibration_browser::list_masters),
        )
        .route(
            "/api/db/{db_id}/calibrations/masters/{master_uuid}/preview",
            get(calibration_browser::get_master_preview),
        )
        .route(
            "/api/db/{db_id}/calibrations/masters/{master_uuid}/fits",
            get(calibration_browser::download_master_fits),
        )
        .with_state(state);
    Server {
        _dir: dir,
        root,
        database,
        app,
    }
}

struct Reply {
    status: StatusCode,
    content_type: String,
    bytes: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.bytes).unwrap_or(Value::Null)
    }
}

async fn call(app: &Router, method: &str, uri: &str, body: Value) -> Reply {
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
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Reply {
        status,
        content_type,
        bytes: bytes.to_vec(),
    }
}

async fn get_json(app: &Router, uri: &str) -> Value {
    let reply = call(app, "GET", uri, Value::Null).await;
    assert_eq!(reply.status, StatusCode::OK, "{uri}: {}", reply.json());
    reply.json()["data"].clone()
}

const BASE: &str = "/api/db/rig/calibrations";

/// How many frames a query keeps.
async fn total(app: &Router, query: &str) -> Value {
    get_json(app, &format!("{BASE}/frames?{query}")).await["total"].clone()
}

fn uuid_of(frames: &Value, name: &str) -> String {
    frames
        .as_array()
        .unwrap()
        .iter()
        .find(|frame| frame["source_path"].as_str().unwrap().ends_with(name))
        .unwrap_or_else(|| panic!("{name} in {frames}"))["frame_uuid"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Poll the status route until `item` is ready, as the browser does.
async fn wait_ready(app: &Router, item: Value) {
    for _ in 0..600 {
        let reply = call(
            app,
            "POST",
            &format!("{BASE}/generation-status"),
            json!({ "requests": [item.clone()] }),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.json());
        let status = reply.json()["data"]["statuses"][0].clone();
        match status["state"].as_str() {
            Some("ready") => return,
            Some("generating") => tokio::time::sleep(Duration::from_millis(50)).await,
            _ => panic!("{item} failed: {status}"),
        }
    }
    panic!("{item} never became ready");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frames_and_sessions_page_and_filter() {
    let server = server();
    let all = get_json(&server.app, &format!("{BASE}/frames?per_page=500")).await;
    assert_eq!(all["total"], 14);
    let frames = all["frames"].clone();

    // Sessions: newest night first; flats ignore exposure, darks split by it.
    let sessions = get_json(&server.app, &format!("{BASE}/sessions")).await;
    assert_eq!(sessions["total"], 5, "{sessions}");
    assert_eq!(sessions["frames"], 14);
    let first = &sessions["sessions"][0];
    assert_eq!(
        (
            first["kind"].as_str(),
            first["filter"].as_str(),
            first["frames"].as_u64()
        ),
        (Some("flat"), Some("OIII"), Some(3))
    );
    assert_eq!(first["night"], "2026-09-12");
    let ha = sessions["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["filter"] == "Ha")
        .unwrap()
        .clone();
    assert_eq!(ha["frames"], 4);
    assert_eq!(
        (ha["exposure_min_s"].as_f64(), ha["exposure_max_s"].as_f64()),
        (Some(2.0), Some(2.3))
    );
    let darks: Vec<u64> = sessions["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|session| session["kind"] == "dark")
        .map(|session| session["frames"].as_u64().unwrap())
        .collect();
    assert_eq!(darks.iter().sum::<u64>(), 5);
    assert_eq!(darks.len(), 2);
    assert!(sessions["facets"]["filters"]
        .as_array()
        .unwrap()
        .iter()
        .any(|filter| filter == "OIII"));
    let paged = get_json(&server.app, &format!("{BASE}/sessions?per_page=2&page=2")).await;
    assert_eq!(paged["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(paged["total"], 5);

    // Frames: paged, sorted, narrowed.
    let page = get_json(
        &server.app,
        &format!("{BASE}/frames?kind=flat&sort=oldest&per_page=3&page=1"),
    )
    .await;
    assert_eq!(page["total"], 7);
    let names: Vec<&str> = page["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|frame| {
            frame["source_path"]
                .as_str()
                .unwrap()
                // Either separator: Windows puts `\` between folders.
                .rsplit(['/', '\\'])
                .next()
                .unwrap()
        })
        .collect();
    assert_eq!(
        names,
        ["flat-ha-3.fits", "flat-oiii-0.fits", "flat-oiii-1.fits"]
    );
    assert!(page["frames"][0]["source_exists"].as_bool().unwrap());
    assert_eq!(page["frames"][0]["night"], "2026-09-10");
    assert_eq!(total(&server.app, "kind=dark&exposure=60").await, 2);
    assert_eq!(total(&server.app, "filter=oiii").await, 3);
    assert_eq!(total(&server.app, "night_from=2026-09-11").await, 3);
    assert_eq!(total(&server.app, "night_to=2026-09-10").await, 11);
    assert_eq!(total(&server.app, "temp_min=-11&temp_max=-9").await, 14);
    assert_eq!(total(&server.app, "temp_min=-5").await, 0);
    let session = format!("session={}", ha["id"].as_str().unwrap());
    assert_eq!(total(&server.app, &session).await, 4);

    // Grades and keeping out: the library's own rule decides.
    let droplet_uuid = uuid_of(&frames, "flat-ha-2.fits");
    let connection = Connection::open(&server.database).unwrap();
    connection
        .execute(
            "UPDATE psf_guard_calibration_frame_quality
             SET grade = 'poor', grade_code = 'changing_features', reason = '30 spots that change'
             WHERE frame_uuid = ?1",
            [&droplet_uuid],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE psf_guard_calibration_frame_quality SET grade = 'ok'
             WHERE frame_uuid <> ?1",
            [&droplet_uuid],
        )
        .unwrap();
    drop(connection);
    assert_eq!(total(&server.app, "grade=poor").await, 1);
    assert_eq!(total(&server.app, "grade=ok").await, 13);
    assert_eq!(total(&server.app, "grade=ungraded").await, 0);
    assert_eq!(total(&server.app, "kept_out=true").await, 1);
    assert_eq!(total(&server.app, "kept_out=false").await, 13);
    let worst = get_json(&server.app, &format!("{BASE}/frames?sort=grade&per_page=1")).await;
    assert_eq!(worst["frames"][0]["frame_uuid"], droplet_uuid.as_str());
    let poor = get_json(&server.app, &format!("{BASE}/sessions?grade=poor")).await;
    assert_eq!(poor["total"], 1);
    assert_eq!(
        (
            poor["sessions"][0]["poor"].as_u64(),
            poor["sessions"][0]["kept_out"].as_u64()
        ),
        (Some(1), Some(1))
    );

    // A file that went missing.
    std::fs::remove_file(server.root.join("frames/bias-1.fits")).unwrap();
    let missing = get_json(&server.app, &format!("{BASE}/frames?missing=true")).await;
    assert_eq!(missing["total"], 1);
    assert_eq!(missing["frames"][0]["source_exists"], false);
    assert_eq!(
        get_json(&server.app, &format!("{BASE}/sessions?missing=true")).await["total"],
        1
    );

    // A malformed filter is refused rather than ignored.
    let refused = call(
        &server.app,
        "GET",
        &format!("{BASE}/frames?grade=awful"),
        Value::Null,
    )
    .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frame_detail_sets_it_beside_its_session() {
    let server = server();
    let frames =
        get_json(&server.app, &format!("{BASE}/frames?per_page=500")).await["frames"].clone();
    let flat = uuid_of(&frames, "flat-ha-2.fits");
    let detail = get_json(&server.app, &format!("{BASE}/frames/{flat}")).await;
    assert_eq!(detail["frame_uuid"], flat.as_str());
    assert_eq!(detail["session_frames"], 4);
    assert_eq!(detail["session_measured"], 4);
    assert_eq!(detail["compare"], true);
    assert_eq!(detail["source_exists"], true);
    assert_eq!(detail["measurement"]["type"], "flat");
    let level = detail["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|metric| metric["key"] == "level_fraction")
        .unwrap();
    assert!(level["value"].as_f64().unwrap() > 0.3);
    assert!(level["session"].as_f64().is_some(), "{level}");
    assert!(detail["findings"].is_array());

    let dark = uuid_of(&frames, "dark-0.fits");
    let detail = get_json(&server.app, &format!("{BASE}/frames/{dark}")).await;
    assert_eq!(detail["compare"], false);
    assert_eq!(detail["session_frames"], 3);
    let keys: Vec<&str> = detail["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|metric| metric["key"].as_str().unwrap())
        .collect();
    assert!(
        keys.contains(&"noise") && keys.contains(&"hot_pixels"),
        "{keys:?}"
    );

    let unknown = call(
        &server.app,
        "GET",
        &format!("{BASE}/frames/nothing-here"),
        Value::Null,
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frame_picture_is_queued_then_served() {
    let server = server();
    let frames =
        get_json(&server.app, &format!("{BASE}/frames?per_page=500")).await["frames"].clone();
    let dark = uuid_of(&frames, "dark-0.fits");
    let uri = format!("{BASE}/frames/{dark}/preview?size=thumb&stretch=tight");
    let first = call(&server.app, "GET", &uri, Value::Null).await;
    assert_eq!(first.status, StatusCode::ACCEPTED, "{}", first.json());
    assert_eq!(first.json()["data"]["state"], "generating");
    wait_ready(
        &server.app,
        json!({ "target": "frame", "id": dark, "size": "thumb", "stretch": "tight" }),
    )
    .await;
    let ready = call(&server.app, "GET", &uri, Value::Null).await;
    assert_eq!(ready.status, StatusCode::OK);
    assert_eq!(ready.content_type, "image/png");
    let picture = image::load_from_memory(&ready.bytes).unwrap();
    assert_eq!((picture.width(), picture.height()), (96, 96));

    // The same read drew the viewer's size.
    let screen = call(
        &server.app,
        "GET",
        &format!("{BASE}/frames/{dark}/preview?stretch=tight"),
        Value::Null,
    )
    .await;
    assert_eq!(screen.status, StatusCode::OK);

    // Pictures live below the database's cache folder.
    let pictures: Vec<_> = std::fs::read_dir(server.root.join("cache/rig/calibration-previews"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(pictures.len(), 2);

    // A frame whose file is gone is not found, and nothing is queued.
    let bias = uuid_of(&frames, "bias-0.fits");
    std::fs::remove_file(server.root.join("frames/bias-0.fits")).unwrap();
    let gone = call(
        &server.app,
        "GET",
        &format!("{BASE}/frames/{bias}/preview"),
        Value::Null,
    )
    .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    let status = call(
        &server.app,
        "POST",
        &format!("{BASE}/generation-status"),
        json!({ "requests": [{ "target": "frame", "id": bias }] }),
    )
    .await;
    assert_eq!(status.json()["data"]["statuses"][0]["state"], "error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_flat_against_its_session_keeps_the_droplet_and_loses_the_dust() {
    let server = server();
    let frames =
        get_json(&server.app, &format!("{BASE}/frames?per_page=500")).await["frames"].clone();
    let flat = uuid_of(&frames, "flat-ha-2.fits");
    let uri = format!("{BASE}/frames/{flat}/preview?view=session");
    let first = call(&server.app, "GET", &uri, Value::Null).await;
    assert_eq!(first.status, StatusCode::ACCEPTED);
    wait_ready(
        &server.app,
        json!({ "target": "frame", "id": flat, "view": "session" }),
    )
    .await;
    let ready = call(&server.app, "GET", &uri, Value::Null).await;
    assert_eq!(ready.status, StatusCode::OK);
    let ratio = image::load_from_memory(&ready.bytes).unwrap().to_luma8();
    let grey = ratio.get_pixel(40, 80)[0];
    assert!((90..=165).contains(&grey), "the field reads grey: {grey}");
    assert!(ratio.get_pixel(66, 60)[0] < 30, "the droplet stays");
    assert!(
        ratio.get_pixel(24, 24)[0].abs_diff(grey) < 40,
        "the dust divides out: {} against {grey}",
        ratio.get_pixel(24, 24)[0]
    );
    // The session's median is kept for the next flat.
    let medians = std::fs::read_dir(server.root.join("cache/rig/calibration-previews"))
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "median"))
        .count();
    assert_eq!(medians, 1);

    let dark = uuid_of(&frames, "dark-0.fits");
    let refused = call(
        &server.app,
        "GET",
        &format!("{BASE}/frames/{dark}/preview?view=session"),
        Value::Null,
    )
    .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn masters_are_listed_with_their_sources_and_users() {
    let server = server();
    let frames =
        get_json(&server.app, &format!("{BASE}/frames?per_page=500")).await["frames"].clone();
    let flats: Vec<String> = (0..4)
        .map(|index| uuid_of(&frames, &format!("flat-ha-{index}.fits")))
        .collect();
    let rig = frames[0]["rig_uuid"].as_str().unwrap().to_string();
    let masters = server.root.join("cache/rig/calibration-masters");
    std::fs::create_dir_all(&masters).unwrap();
    let flat_label = format!("flat-{}.fits", "a".repeat(64));
    let dark_label = format!("dark-{}.fits", "b".repeat(64));
    let image = seiza_stacking::LinearImage::new(
        64,
        64,
        1,
        (0..64 * 64)
            .map(|index| 1.0 - (index % 64) as f32 * 0.001)
            .collect(),
    )
    .unwrap();
    seiza_stacking::write_processed_image_fits_f32(masters.join(&flat_label), &image, &[], &[])
        .unwrap();
    let connection = Connection::open(&server.database).unwrap();
    let insert = |uuid: &str,
                  kind: &str,
                  label: &str,
                  sources: &[String],
                  created: i64,
                  statistics: Value| {
        connection
            .execute(
                "INSERT INTO psf_guard_calibration_master (
                    master_uuid, rig_uuid, kind, cache_path, source_set_hash, source_count,
                    source_frame_uuids, created_at, seiza_version, cache_version, filter_name,
                    statistics_json)
                 VALUES (?1, ?2, ?3, ?4, ?1, ?5, ?6, ?7, 'test', 3, 'Ha', ?8)",
                rusqlite::params![
                    uuid,
                    rig,
                    kind,
                    masters.join(label).to_string_lossy(),
                    sources.len() as i64,
                    serde_json::to_string(sources).unwrap(),
                    created,
                    statistics.to_string(),
                ],
            )
            .unwrap();
    };
    insert(
        "00000000-0000-0000-0000-00000000f1a7",
        "flat",
        &flat_label,
        &flats,
        2_000,
        json!({
            "rejection_method": "median_mad",
            "rejected_samples": 12,
            "defect_pixels_replaced": 3,
            "frame_quality": {"measured_frames": 4, "frames": 4, "master": {"grade": "ok", "findings": []},
                              "steady_features": 1, "changing_features": 2, "carried_features": 0}
        }),
    );
    insert(
        "00000000-0000-0000-0000-00000000da7c",
        "dark",
        &dark_label,
        &[
            uuid_of(&frames, "dark-0.fits"),
            uuid_of(&frames, "dark-1.fits"),
        ],
        1_000,
        // Recorded before thin masters were marked: judged by its count.
        json!({"rejection_method": "sigma_clip"}),
    );
    drop(connection);
    let stacks = server.root.join("cache/rig/stack-previews");
    std::fs::create_dir_all(&stacks).unwrap();
    std::fs::write(
        stacks.join("latest-project-1.json"),
        json!({"groups": [{"calibration": {"flat_master": flat_label}}]}).to_string(),
    )
    .unwrap();

    let catalog = get_json(&server.app, &format!("{BASE}/masters")).await;
    let list = catalog["masters"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    let flat = &list[0];
    assert_eq!(flat["kind"], "flat");
    assert_eq!(flat["available"], true);
    assert_eq!(flat["source_count"], 4);
    assert_eq!(flat["sources"]["frames"], 4);
    assert_eq!(flat["sources"]["first_night"], "2026-09-10");
    assert_eq!(flat["flat_set"]["changing_features"], 2);
    assert_eq!(flat["defect_pixels_replaced"], 3);
    assert_eq!(flat["used_by"], json!([{"project_id": 1, "name": "Heart"}]));
    assert_eq!(flat["usages"], json!([]));
    let dark = &list[1];
    assert_eq!(
        (dark["thin"].as_bool(), dark["available"].as_bool()),
        (Some(true), Some(false))
    );
    assert!(dark["unavailable_reason"].is_string());
    assert_eq!(dark["used_by"], json!([]));

    let id = flat["id"].as_str().unwrap();
    let preview = flat["preview_url"].as_str().unwrap();
    let first = call(&server.app, "GET", preview, Value::Null).await;
    assert_eq!(first.status, StatusCode::ACCEPTED);
    wait_ready(&server.app, json!({ "target": "master", "id": id })).await;
    let ready = call(&server.app, "GET", preview, Value::Null).await;
    assert_eq!(
        (ready.status, ready.content_type.as_str()),
        (StatusCode::OK, "image/png")
    );
    let fits = call(
        &server.app,
        "GET",
        flat["fits_url"].as_str().unwrap(),
        Value::Null,
    )
    .await;
    assert_eq!(fits.status, StatusCode::OK);
    assert_eq!(fits.content_type, "application/fits");
    assert!(fits.bytes.starts_with(b"SIMPLE"));

    let missing = call(
        &server.app,
        "GET",
        &format!("{BASE}/masters/00000000-0000-0000-0000-00000000da7c/preview"),
        Value::Null,
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    let status = call(
        &server.app,
        "POST",
        &format!("{BASE}/generation-status"),
        json!({ "requests": [{ "target": "master", "id": "00000000-0000-0000-0000-00000000da7c" }] }),
    )
    .await;
    assert_eq!(status.json()["data"]["statuses"][0]["state"], "error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frame_recorded_outside_the_databases_folders_reads_as_missing() {
    // A catalog synced from another machine can record any path. The file
    // exists, but outside this database's image and calibration folders,
    // so no route opens it or says it is there.
    let server = server();
    let frames =
        get_json(&server.app, &format!("{BASE}/frames?per_page=500")).await["frames"].clone();
    let bias = uuid_of(&frames, "bias-0.fits");
    let outside = tempfile::tempdir().unwrap();
    let moved = outside.path().join("bias-0.fits");
    std::fs::copy(server.root.join("frames").join("bias-0.fits"), &moved).unwrap();
    let connection = Connection::open(&server.database).unwrap();
    connection
        .execute(
            "UPDATE psf_guard_calibration_frame SET source_path = ?1 WHERE frame_uuid = ?2",
            rusqlite::params![moved.display().to_string(), bias],
        )
        .unwrap();
    drop(connection);

    let detail = get_json(&server.app, &format!("{BASE}/frames/{bias}")).await;
    assert_eq!(detail["source_exists"], false);
    let picture = call(
        &server.app,
        "GET",
        &format!("{BASE}/frames/{bias}/preview"),
        Value::Null,
    )
    .await;
    assert_eq!(picture.status, StatusCode::NOT_FOUND);
    let missing = get_json(&server.app, &format!("{BASE}/frames?missing=true")).await;
    assert_eq!(missing["total"], 1);
    assert_eq!(missing["frames"][0]["frame_uuid"], bias.as_str());
}
