use std::fmt::Write as _;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::Router;
use http_body_util::BodyExt;
use psf_guard::cli::PregenerationConfig;
use psf_guard::db_registry::{DbEntry, RemoteImageUploadConfig};
use psf_guard::server::{remote_upload, state::AppState};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::{tempdir, TempDir};
use tower::ServiceExt;

const TOKEN_A: &str = "test-upload-token-a-1234567890";
const TOKEN_B: &str = "test-upload-token-b-1234567890";

struct Fixture {
    _directory: TempDir,
    state: Arc<AppState>,
    database_a: std::path::PathBuf,
    database_b: std::path::PathBuf,
    images_a: std::path::PathBuf,
    images_b: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempdir().unwrap();
        let database_a = directory.path().join("a.sqlite");
        let database_b = directory.path().join("b.sqlite");
        let images_a = directory.path().join("images-a");
        let images_b = directory.path().join("images-b");
        std::fs::create_dir_all(&images_a).unwrap();
        std::fs::create_dir_all(&images_b).unwrap();
        psf_guard::ts_schema::create_fresh_db(&database_a).unwrap();
        psf_guard::ts_schema::create_fresh_db(&database_b).unwrap();

        let mut config_a = RemoteImageUploadConfig {
            enabled: true,
            image_dir: images_a.to_string_lossy().into_owned(),
            ..Default::default()
        };
        config_a.set_token(TOKEN_A).unwrap();
        let mut config_b = RemoteImageUploadConfig {
            enabled: false,
            image_dir: images_b.to_string_lossy().into_owned(),
            ..Default::default()
        };
        config_b.set_token(TOKEN_B).unwrap();

        let entries = vec![
            DbEntry {
                id: "catalog-a".into(),
                name: "Catalog A".into(),
                db_path: database_a.to_string_lossy().into_owned(),
                image_dirs: vec![images_a.to_string_lossy().into_owned()],
                reject_archive: None,
                remote_image_upload: Some(config_a),
                export_dir: None,
            },
            DbEntry {
                id: "catalog-b".into(),
                name: "Catalog B".into(),
                db_path: database_b.to_string_lossy().into_owned(),
                image_dirs: vec![images_b.to_string_lossy().into_owned()],
                reject_archive: None,
                remote_image_upload: Some(config_b),
                export_dir: None,
            },
        ];
        let state = Arc::new(
            AppState::from_databases(
                entries,
                directory
                    .path()
                    .join("cache")
                    .to_string_lossy()
                    .into_owned(),
                PregenerationConfig::default(),
            )
            .unwrap(),
        );

        Self {
            _directory: directory,
            state,
            database_a,
            database_b,
            images_a,
            images_b,
        }
    }
}

fn build_app(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/api/db/{db_id}/images/upload",
            post(remote_upload::upload_image)
                .layer(DefaultBodyLimit::max(remote_upload::MAX_MULTIPART_BYTES)),
        )
        .with_state(state)
}

fn fits_bytes(object: &str, date_obs: &str) -> Vec<u8> {
    fits_bytes_with_type("LIGHT", object, date_obs)
}

fn fits_bytes_with_type(image_type: &str, object: &str, date_obs: &str) -> Vec<u8> {
    fn card(output: &mut Vec<u8>, text: &str) {
        let mut bytes = text.as_bytes().to_vec();
        assert!(bytes.len() <= 80);
        bytes.resize(80, b' ');
        output.extend_from_slice(&bytes);
    }

    let mut bytes = Vec::new();
    card(&mut bytes, "SIMPLE  =                    T");
    card(&mut bytes, "BITPIX  =                   16");
    card(&mut bytes, "NAXIS   =                    2");
    card(&mut bytes, "NAXIS1  =                   10");
    card(&mut bytes, "NAXIS2  =                   10");
    card(&mut bytes, &format!("IMAGETYP= '{image_type}'"));
    card(&mut bytes, &format!("OBJECT  = '{object}'"));
    card(&mut bytes, "FILTER  = 'Ha      '");
    card(&mut bytes, &format!("DATE-OBS= '{date_obs}'"));
    card(&mut bytes, "EXPTIME =                300.0");
    card(&mut bytes, "GAIN    =                  100");
    card(&mut bytes, "OFFSET  =                   30");
    card(&mut bytes, "XBINNING=                    1");
    card(&mut bytes, "YBINNING=                    1");
    card(&mut bytes, "RA      =            10.680000");
    card(&mut bytes, "DEC     =            41.268700");
    card(&mut bytes, "END");
    bytes.resize(bytes.len().div_ceil(2880) * 2880, b' ');
    bytes.extend_from_slice(&[0u8; 2880]);
    bytes
}

/// The same light frame in a monolithic XISF container, written by the XISF
/// writer rather than hand-assembled, so the sample stays a real file.
fn xisf_bytes(object: &str, date_obs: &str) -> Vec<u8> {
    use seiza_fits::{F32ImageData, HeaderValue, WriteHeaderCard};

    let pixels = vec![0.0f32; 100];
    let mut bytes = Vec::new();
    seiza_xisf::write_f32_image_to(
        &mut bytes,
        10,
        10,
        F32ImageData::Mono(&pixels),
        &[
            WriteHeaderCard::new("IMAGETYP", HeaderValue::String("LIGHT".into())),
            WriteHeaderCard::new("OBJECT", HeaderValue::String(object.into())),
            WriteHeaderCard::new("FILTER", HeaderValue::String("Ha".into())),
            WriteHeaderCard::new("DATE-OBS", HeaderValue::String(date_obs.into())),
            WriteHeaderCard::new("EXPTIME", HeaderValue::Float(300.0)),
            WriteHeaderCard::new("GAIN", HeaderValue::Integer(100)),
            WriteHeaderCard::new("OFFSET", HeaderValue::Integer(30)),
            WriteHeaderCard::new("XBINNING", HeaderValue::Integer(1)),
            WriteHeaderCard::new("YBINNING", HeaderValue::Integer(1)),
            WriteHeaderCard::new("RA", HeaderValue::Float(10.68)),
            WriteHeaderCard::new("DEC", HeaderValue::Float(41.2687)),
        ],
    )
    .unwrap();
    bytes
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        write!(encoded, "{byte:02x}").unwrap();
    }
    encoded
}

fn multipart(filename: &str, image: &[u8]) -> (String, Vec<u8>) {
    let boundary = "psf-guard-test-boundary";
    let mut body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"image\"; filename=\"{filename}\"\r\n\
         Content-Type: application/fits\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(image);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (boundary.into(), body)
}

async fn upload(
    state: Arc<AppState>,
    url_database: &str,
    echoed_database: &str,
    token: &str,
    filename: &str,
    image: &[u8],
    checksum: &str,
) -> (StatusCode, Value) {
    let (boundary, body) = multipart(filename, image);
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/db/{url_database}/images/upload"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("authorization", format!("Bearer {token}"))
        .header("x-psf-guard-database-id", echoed_database)
        .header("x-content-sha256", checksum)
        .body(Body::from(body))
        .unwrap();
    let response = build_app(state).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn image_count(path: &std::path::Path) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM acquiredimage", [], |row| row.get(0))
        .unwrap()
}

fn calibration_count(path: &std::path::Path) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM psf_guard_calibration_frame",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0)
}

#[tokio::test]
async fn upload_is_scoped_to_the_selected_database_and_attaches_followup_frames() {
    let fixture = Fixture::new();
    let first = fits_bytes("M 31", "2026-07-24T05:00:00");
    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        "m31-001.fits",
        &first,
        &sha256(&first),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["database_id"], "catalog-a");
    assert_eq!(body["data"]["frame_kind"], "light");
    assert_eq!(body["data"]["resolution"]["target_name"], "M 31");
    assert_eq!(body["data"]["import"]["targets_created"], 1);
    let target_id = body["data"]["resolution"]["target_id"].as_i64().unwrap();
    assert!(fixture.images_a.join("m31-001.fits").is_file());
    assert!(!fixture.images_b.join("m31-001.fits").exists());
    assert_eq!(image_count(&fixture.database_a), 1);
    assert_eq!(image_count(&fixture.database_b), 0);

    let second = fits_bytes("M 31", "2026-07-24T05:05:00");
    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        "m31-002.fits",
        &second,
        &sha256(&second),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["resolution"]["target_id"], target_id);
    assert_eq!(body["data"]["import"]["attached"], 1);
    assert_eq!(body["data"]["import"]["targets_created"], 0);
    assert_eq!(image_count(&fixture.database_a), 2);
}

#[tokio::test]
async fn calibration_uploads_are_cataloged_without_scheduler_images() {
    let fixture = Fixture::new();
    let cases = [
        ("BIAS", "bias", "bias-001.fits"),
        ("DARK", "dark", "dark-001.fits"),
        ("DARK FLAT", "dark_flat", "dark-flat-001.fits"),
        ("FLAT DARK", "dark_flat", "flat-dark-001.fits"),
        ("FLAT", "flat", "flat-001.fits"),
    ];

    for (index, (image_type, kind, filename)) in cases.iter().enumerate() {
        let image = fits_bytes_with_type(
            image_type,
            "Calibration",
            &format!("2026-07-24T08:{index:02}:00"),
        );
        let (status, body) = upload(
            fixture.state.clone(),
            "catalog-a",
            "catalog-a",
            TOKEN_A,
            filename,
            &image,
            &sha256(&image),
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["frame_kind"], *kind);
        assert!(body["data"].get("resolution").is_none());
        assert_eq!(body["data"]["calibration"]["kind"], *kind);
        assert!(body["data"]["calibration"]["frame_uuid"].is_string());
        assert!(body["data"]["calibration"]["rig_uuid"].is_string());
        assert_eq!(body["data"]["import"]["calibration"][*kind], 1);
        assert!(fixture.images_a.join(filename).is_file());
        assert!(!fixture.images_b.join(filename).exists());
    }

    assert_eq!(image_count(&fixture.database_a), 0);
    assert_eq!(calibration_count(&fixture.database_a), 5);
    assert_eq!(calibration_count(&fixture.database_b), 0);

    let retry = fits_bytes_with_type("FLAT", "Calibration", "2026-07-24T08:04:00");
    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        "flat-001.fits",
        &retry,
        &sha256(&retry),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["already_present"], true);
    assert_eq!(body["data"]["import"]["calibration"]["skipped_existing"], 1);
    assert_eq!(image_count(&fixture.database_a), 0);
    assert_eq!(calibration_count(&fixture.database_a), 5);
}

#[tokio::test]
async fn unsupported_non_light_frame_is_rejected_without_publishing() {
    let fixture = Fixture::new();
    let image = fits_bytes_with_type("SNAPSHOT", "Preview", "2026-07-24T09:00:00");
    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        "snapshot-001.fits",
        &image,
        &sha256(&image),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!fixture.images_a.join("snapshot-001.fits").exists());
    assert_eq!(image_count(&fixture.database_a), 0);
    assert_eq!(calibration_count(&fixture.database_a), 0);
}

#[tokio::test]
async fn identical_retry_is_idempotent() {
    let fixture = Fixture::new();
    let image = fits_bytes("M 42", "2026-07-24T06:00:00");
    let checksum = sha256(&image);
    for expected_present in [false, true] {
        let (status, body) = upload(
            fixture.state.clone(),
            "catalog-a",
            "catalog-a",
            TOKEN_A,
            "m42-001.fits",
            &image,
            &checksum,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["already_present"], expected_present);
    }
    assert_eq!(image_count(&fixture.database_a), 1);

    let changed = fits_bytes("M 42", "2026-07-24T06:01:00");
    let (status, _) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        "m42-001.fits",
        &changed,
        &sha256(&changed),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        sha256(&std::fs::read(fixture.images_a.join("m42-001.fits")).unwrap()),
        checksum
    );
    assert_eq!(image_count(&fixture.database_a), 1);
}

#[tokio::test]
async fn database_echo_token_and_checksum_are_required_before_publish() {
    let fixture = Fixture::new();
    let image = fits_bytes("M 33", "2026-07-24T07:00:00");
    let checksum = sha256(&image);

    let (status, _) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-b",
        TOKEN_A,
        "m33-001.fits",
        &image,
        &checksum,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_B,
        "m33-001.fits",
        &image,
        &checksum,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = upload(
        fixture.state.clone(),
        "catalog-b",
        "catalog-b",
        TOKEN_B,
        "m33-001.fits",
        &image,
        &checksum,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        "m33-001.fits",
        &image,
        &"0".repeat(64),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let catalog = fixture.state.get_database("catalog-a").unwrap();
    let _import_guard = catalog.image_import_mutex.lock().await;
    let (status, _) = upload(
        fixture.state,
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        "m33-001.fits",
        &image,
        &checksum,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(!fixture.images_a.join("m33-001.fits").exists());
    assert_eq!(image_count(&fixture.database_a), 0);
    assert_eq!(image_count(&fixture.database_b), 0);
}

#[tokio::test]
async fn upload_accepts_an_xisf_frame_and_imports_it_like_a_fits_one() {
    let fixture = Fixture::new();
    let image = xisf_bytes("M 81", "2026-07-24T06:00:00");
    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        "m81-001.xisf",
        &image,
        &sha256(&image),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["resolution"]["target_name"], "M 81");
    assert_eq!(body["data"]["import"]["imported"], 1);
    assert_eq!(body["data"]["import"]["unreadable"], 0);
    assert!(fixture.images_a.join("m81-001.xisf").is_file());
    assert_eq!(image_count(&fixture.database_a), 1);

    // The receive directory is one of this database's scanned image roots, so
    // the upload must leave nothing behind that a folder scan would read as a
    // frame.
    let scannable =
        psf_guard::commands::import::collect_fits_files(std::slice::from_ref(&fixture.images_a))
            .expect("scanning the receive directory");
    assert_eq!(
        scannable,
        vec![fixture.images_a.join("m81-001.xisf")],
        "only the published frame should be scannable"
    );
}

#[tokio::test]
async fn upload_still_rejects_a_non_image_extension() {
    let fixture = Fixture::new();
    let image = fits_bytes("M 81", "2026-07-24T06:00:00");
    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        "m81-001.png",
        &image,
        &sha256(&image),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body.to_string().contains(".xisf"),
        "the error should list what is accepted: {body}"
    );
    assert_eq!(image_count(&fixture.database_a), 0);
}
