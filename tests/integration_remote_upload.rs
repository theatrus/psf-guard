use std::fmt::Write as _;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::Router;
use http_body_util::BodyExt;
use psf_guard::cli::PregenerationConfig;
use psf_guard::db_registry::{DbEntry, RemoteImageUploadConfig, RemoteImageUploadPlacement};
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
        Self::with_placement(RemoteImageUploadPlacement::Flat)
    }

    fn with_placement(placement: RemoteImageUploadPlacement) -> Self {
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
            placement,
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

    fn reconfigured_state(
        &self,
        placement: RemoteImageUploadPlacement,
        receive_dir: &std::path::Path,
        image_dirs: &[&std::path::Path],
    ) -> Arc<AppState> {
        let mut config = RemoteImageUploadConfig {
            enabled: true,
            image_dir: receive_dir.to_string_lossy().into_owned(),
            placement,
            ..Default::default()
        };
        config.set_token(TOKEN_A).unwrap();
        Arc::new(
            AppState::from_databases(
                vec![DbEntry {
                    id: "catalog-a".into(),
                    name: "Catalog A".into(),
                    db_path: self.database_a.to_string_lossy().into_owned(),
                    image_dirs: image_dirs
                        .iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect(),
                    reject_archive: None,
                    remote_image_upload: Some(config),
                    export_dir: None,
                }],
                self._directory
                    .path()
                    .join("cache-reconfigured")
                    .to_string_lossy()
                    .into_owned(),
                PregenerationConfig::default(),
            )
            .unwrap(),
        )
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
    fits_bytes_with_identity(image_type, object, Some("Ha"), Some(date_obs))
}

fn fits_bytes_with_identity(
    image_type: &str,
    object: &str,
    filter: Option<&str>,
    date_obs: Option<&str>,
) -> Vec<u8> {
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
    if let Some(filter) = filter {
        card(&mut bytes, &format!("FILTER  = '{filter}'"));
    }
    if let Some(date_obs) = date_obs {
        card(&mut bytes, &format!("DATE-OBS= '{date_obs}'"));
    }
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

fn insert_synced_light(path: &std::path::Path, filename: &str) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute(
            "INSERT INTO project (Id, profileId, name, isMosaic, flatsHandling, guid)
             VALUES (1, 'profile', 'M 31', 0, 0, 'project-guid')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO target (Id, name, active, ra, dec, epochcode, projectId, guid)
             VALUES (1, 'M 31', 1, 0.71, 41.27, 0, 1, 'target-guid')",
            [],
        )
        .unwrap();
    let metadata = serde_json::json!({
        "FileName": format!(r"C:\remote-capture\{filename}"),
    })
    .to_string();
    connection
        .execute(
            "INSERT INTO acquiredimage
                (Id, projectId, targetId, acquireddate, filtername, gradingStatus,
                 metadata, profileId, guid)
             VALUES (1, 1, 1, 1784869200, 'Ha', 1, ?1, 'profile', 'image-guid')",
            [&metadata],
        )
        .unwrap();
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
async fn target_tree_uploads_use_only_sanitized_header_and_catalog_segments() {
    let fixture = Fixture::with_placement(RemoteImageUploadPlacement::TargetTree);
    let image = fits_bytes("../M 31/Panel:West", "2026-07-24T05:00:00");
    let filename = "m31-panel-001.fits";

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
    assert!(fixture
        .images_a
        .join("_M 31_Panel_West")
        .join("LIGHT")
        .join("Ha")
        .join(filename)
        .is_file());
    assert!(!fixture.images_a.join(filename).exists());
    assert!(!fixture._directory.path().join("M 31").exists());
}

#[tokio::test]
async fn target_tree_new_capture_does_not_reuse_an_unregistered_flat_basename() {
    let fixture = Fixture::with_placement(RemoteImageUploadPlacement::TargetTree);
    let filename = "capture.fits";
    let old_flat = fits_bytes("Old Target", "2026-07-24T04:00:00");
    std::fs::write(fixture.images_a.join(filename), &old_flat).unwrap();
    let image = fits_bytes("M 42", "2026-07-24T05:00:00");

    let (status, body) = upload(
        fixture.state,
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &sha256(&image),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        std::fs::read(fixture.images_a.join(filename)).unwrap(),
        old_flat
    );
    assert!(fixture
        .images_a
        .join("M 42")
        .join("LIGHT")
        .join("Ha")
        .join(filename)
        .is_file());
}

#[tokio::test]
async fn target_tree_sync_first_capture_ignores_an_unrelated_legacy_flat_basename() {
    let fixture = Fixture::with_placement(RemoteImageUploadPlacement::TargetTree);
    let filename = "synced-capture.fits";
    insert_synced_light(&fixture.database_a, filename);
    let old_flat = fits_bytes("Old Target", "2026-07-24T04:00:00");
    std::fs::write(fixture.images_a.join(filename), &old_flat).unwrap();
    let image = fits_bytes("M 31", "2026-07-24T05:00:00");

    let (status, body) = upload(
        fixture.state,
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &sha256(&image),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["resolution"]["image_id"], 1);
    assert_eq!(
        std::fs::read(fixture.images_a.join(filename)).unwrap(),
        old_flat
    );
    assert!(fixture
        .images_a
        .join("M 31")
        .join("LIGHT")
        .join("Ha")
        .join(filename)
        .is_file());
}

#[tokio::test]
async fn target_tree_matches_the_imported_name_for_a_light_without_an_object() {
    let fixture = Fixture::with_placement(RemoteImageUploadPlacement::TargetTree);
    let image = fits_bytes("", "2026-07-24T05:00:00");
    let filename = "unknown-001.fits";

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
    assert!(fixture
        .images_a
        .join("Unknown Target")
        .join("LIGHT")
        .join("Ha")
        .join(filename)
        .is_file());
}

#[tokio::test]
async fn mapped_light_retry_restores_a_missing_file_without_capture_identity_headers() {
    let fixture = Fixture::with_placement(RemoteImageUploadPlacement::TargetTree);
    let image = fits_bytes_with_identity("LIGHT", "M 31", None, None);
    let filename = "m31-no-identity.fits";
    let checksum = sha256(&image);

    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &checksum,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let destination = fixture
        .images_a
        .join("M 31")
        .join("LIGHT")
        .join("NONE")
        .join(filename);
    assert!(destination.is_file());
    std::fs::remove_file(&destination).unwrap();

    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &checksum,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["already_present"], false);
    assert!(destination.is_file());
    assert_eq!(image_count(&fixture.database_a), 1);
}

#[tokio::test]
async fn upload_attaches_bytes_to_a_light_created_by_scheduler_sync() {
    let fixture = Fixture::new();
    let filename = "m31-synced.fits";
    insert_synced_light(&fixture.database_a, filename);
    let connection = rusqlite::Connection::open(&fixture.database_a).unwrap();
    let metadata: String = connection
        .query_row(
            "SELECT metadata FROM acquiredimage WHERE Id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
    metadata["DetectedStars"] = 99.into();
    metadata["HFR"] = 9.9.into();
    metadata["PsfGuardQualitySource"] = "file:old-source".into();
    metadata["PsfGuardQualityFields"] = serde_json::json!(["HFR"]);
    connection
        .execute(
            "UPDATE acquiredimage SET metadata = ?1 WHERE Id = 1",
            [metadata.to_string()],
        )
        .unwrap();
    drop(connection);
    let image = fits_bytes("M 31", "2026-07-24T05:00:00");
    let checksum = sha256(&image);
    let context = fixture.state.get_database("catalog-a").unwrap();
    let previews = context.cache_dir_path.join("previews");
    let annotated = context.cache_dir_path.join("annotated");
    std::fs::create_dir_all(&previews).unwrap();
    std::fs::create_dir_all(&annotated).unwrap();
    let stale_preview = previews.join("1_old-source.png");
    let stale_annotated = annotated.join("annotated_v4_1_old-source.png");
    let unrelated_preview = previews.join("2_other-image.png");
    std::fs::write(&stale_preview, b"stale").unwrap();
    std::fs::write(&stale_annotated, b"stale").unwrap();
    std::fs::write(&unrelated_preview, b"keep").unwrap();

    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &checksum,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["already_present"], false);
    assert_eq!(body["data"]["resolution"]["image_id"], 1);
    assert_eq!(body["data"]["resolution"]["project_name"], "M 31");
    assert_eq!(body["data"]["import"]["imported"], 0);
    assert_eq!(body["data"]["import"]["skipped_existing"], 1);
    assert_eq!(image_count(&fixture.database_a), 1);
    let metadata: String = rusqlite::Connection::open(&fixture.database_a)
        .unwrap()
        .query_row(
            "SELECT metadata FROM acquiredimage WHERE Id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
    assert_eq!(metadata["DetectedStars"], 99);
    assert!(metadata.get("HFR").is_none());
    assert!(metadata.get("PsfGuardQualitySource").is_none());
    assert!(metadata.get("PsfGuardQualityFields").is_none());
    // Mapping-aware artifact keys make these unreachable without scanning
    // whole cache directories on every upload.
    assert!(stale_preview.exists());
    assert!(stale_annotated.exists());
    assert!(unrelated_preview.exists());
    assert_eq!(
        sha256(&std::fs::read(fixture.images_a.join(filename)).unwrap()),
        checksum
    );

    let current_preview = previews.join("1_current-source.png");
    let current_annotated = annotated.join("annotated_v4_1_current-source.png");
    std::fs::write(&current_preview, b"current").unwrap();
    std::fs::write(&current_annotated, b"current").unwrap();

    let (status, body) = upload(
        fixture.state,
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &checksum,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["already_present"], true);
    assert_eq!(image_count(&fixture.database_a), 1);
    assert!(current_preview.exists());
    assert!(current_annotated.exists());
}

#[tokio::test]
async fn synced_light_retry_keeps_its_registered_file_after_target_and_root_changes() {
    let fixture = Fixture::with_placement(RemoteImageUploadPlacement::TargetTree);
    let filename = "m31-synced-retry.fits";
    insert_synced_light(&fixture.database_a, filename);
    let image = fits_bytes("M 31", "2026-07-24T05:00:00");
    let checksum = sha256(&image);

    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &checksum,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let original = fixture
        .images_a
        .join("M 31")
        .join("LIGHT")
        .join("Ha")
        .join(filename);
    assert!(original.is_file());

    let connection = rusqlite::Connection::open(&fixture.database_a).unwrap();
    connection
        .execute("UPDATE target SET name = 'Renamed target' WHERE Id = 1", [])
        .unwrap();
    connection
        .execute(
            "UPDATE acquiredimage
             SET metadata = '{\"FileName\":\"C:\\\\changed\\\\renamed-by-scheduler.fits\"}'
             WHERE Id = 1",
            [],
        )
        .unwrap();
    drop(connection);
    std::fs::remove_dir_all(fixture.images_a.join("M 31")).unwrap();
    let second_root = fixture._directory.path().join("images-c");
    std::fs::create_dir_all(&second_root).unwrap();
    let reconfigured = fixture.reconfigured_state(
        RemoteImageUploadPlacement::Flat,
        &second_root,
        &[&fixture.images_a, &second_root],
    );

    let (status, body) = upload(
        reconfigured,
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &checksum,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["already_present"], false);
    assert!(original.is_file());
    assert!(!second_root.join(filename).exists());
}

#[tokio::test]
async fn synced_light_rejects_a_same_named_frame_with_different_capture_headers() {
    let fixture = Fixture::with_placement(RemoteImageUploadPlacement::TargetTree);
    let filename = "m31-synced-mismatch.fits";
    insert_synced_light(&fixture.database_a, filename);
    let different_capture = fits_bytes("M 31", "2026-07-24T05:10:00");

    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &different_capture,
        &sha256(&different_capture),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("capture time or filter"));
    assert!(!fixture
        .images_a
        .join("M 31")
        .join("LIGHT")
        .join("Ha")
        .join(filename)
        .exists());
    assert_eq!(image_count(&fixture.database_a), 1);
}

#[tokio::test]
async fn sync_first_light_without_capture_headers_requires_the_resolved_target() {
    let fixture = Fixture::with_placement(RemoteImageUploadPlacement::TargetTree);
    let filename = "m31-synced-target-only.fits";
    insert_synced_light(&fixture.database_a, filename);
    let image = fits_bytes_with_identity("LIGHT", "M 31", None, None);

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
    assert_eq!(body["data"]["resolution"]["image_id"], 1);

    let fixture = Fixture::with_placement(RemoteImageUploadPlacement::TargetTree);
    let filename = "m31-synced-explicit-filter-mismatch.fits";
    insert_synced_light(&fixture.database_a, filename);
    let wrong_filter = fits_bytes_with_identity("LIGHT", "M 31", Some("OIII"), None);
    let (status, body) = upload(
        fixture.state,
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &wrong_filter,
        &sha256(&wrong_filter),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("capture time or filter"));
}

#[tokio::test]
async fn sync_first_light_rejects_conflicting_object_even_when_time_and_filter_match() {
    let fixture = Fixture::with_placement(RemoteImageUploadPlacement::TargetTree);
    let filename = "mismatched-target.fits";
    insert_synced_light(&fixture.database_a, filename);
    rusqlite::Connection::open(&fixture.database_a)
        .unwrap()
        .execute(
            "INSERT INTO target (Id, name, active, ra, dec, epochcode, projectId, guid)
             VALUES (2, 'M 42', 1, 5.59, -5.45, 0, 1, 'm42-target-guid')",
            [],
        )
        .unwrap();
    let wrong_target = fits_bytes("M 42", "2026-07-24T05:00:00");

    let (status, body) = upload(
        fixture.state,
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &wrong_target,
        &sha256(&wrong_target),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("resolved target"));
    assert_eq!(image_count(&fixture.database_a), 1);
    assert!(!fixture.images_a.join("M 31").exists());
    assert!(!fixture.images_a.join("M 42").exists());
}

#[tokio::test]
async fn synced_light_upload_supports_a_legacy_schema_without_image_guids() {
    let fixture = Fixture::new();
    let filename = "legacy-synced.fits";
    insert_synced_light(&fixture.database_a, filename);
    rusqlite::Connection::open(&fixture.database_a)
        .unwrap()
        .execute("ALTER TABLE acquiredimage DROP COLUMN guid", [])
        .unwrap();
    let image = fits_bytes("M 31", "2026-07-24T05:00:00");
    let checksum = sha256(&image);

    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &checksum,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let connection = rusqlite::Connection::open(&fixture.database_a).unwrap();
    let metadata: String = connection
        .query_row(
            "SELECT metadata FROM acquiredimage WHERE Id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
    metadata["DetectedStars"] = 321.into();
    metadata["HFR"] = 2.4.into();
    connection
        .execute(
            "UPDATE acquiredimage SET metadata = ?1 WHERE Id = 1",
            [metadata.to_string()],
        )
        .unwrap();
    drop(connection);
    let context = fixture.state.get_database("catalog-a").unwrap();
    let preview = context.cache_dir_path.join("previews/1_legacy-quality.png");
    let annotated = context
        .cache_dir_path
        .join("annotated/annotated_v4_1_legacy-quality.png");
    std::fs::create_dir_all(preview.parent().unwrap()).unwrap();
    std::fs::create_dir_all(annotated.parent().unwrap()).unwrap();
    std::fs::write(&preview, b"current").unwrap();
    std::fs::write(&annotated, b"current").unwrap();

    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &checksum,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["already_present"], true);
    assert!(preview.exists());
    assert!(annotated.exists());
    let mapping_count: i64 = rusqlite::Connection::open(&fixture.database_a)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM psf_guard_remote_image_file",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(mapping_count, 1);
    let metadata: String = rusqlite::Connection::open(&fixture.database_a)
        .unwrap()
        .query_row(
            "SELECT metadata FROM acquiredimage WHERE Id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
    assert_eq!(metadata["DetectedStars"], 321);
    assert_eq!(metadata["HFR"], 2.4);
}

#[tokio::test]
async fn null_guid_mapping_is_removed_when_a_legacy_image_id_is_reused() {
    let fixture = Fixture::new();
    let filename = "legacy-reused-id.fits";
    insert_synced_light(&fixture.database_a, filename);
    let connection = rusqlite::Connection::open(&fixture.database_a).unwrap();
    connection
        .execute("ALTER TABLE acquiredimage DROP COLUMN guid", [])
        .unwrap();
    drop(connection);
    let image = fits_bytes("M 31", "2026-07-24T05:00:00");
    let checksum = sha256(&image);

    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &checksum,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let connection = rusqlite::Connection::open(&fixture.database_a).unwrap();
    connection
        .execute("DELETE FROM acquiredimage WHERE Id = 1", [])
        .unwrap();
    let metadata = serde_json::json!({
        "FileName": format!(r"C:\new-capture\{filename}"),
    })
    .to_string();
    connection
        .execute(
            "INSERT INTO acquiredimage
                (Id, projectId, targetId, acquireddate, filtername, gradingStatus,
                 metadata, profileId)
             VALUES (1, 1, 1, 1784872800, 'OIII', 1, ?1, 'profile')",
            [&metadata],
        )
        .unwrap();
    drop(connection);

    let (status, body) = upload(
        fixture.state,
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &checksum,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let mapping_count: i64 = rusqlite::Connection::open(&fixture.database_a)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM psf_guard_remote_image_file",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(mapping_count, 0);
}

#[tokio::test]
async fn pre_mapping_flat_upload_is_found_after_the_receive_root_changes() {
    let fixture = Fixture::new();
    let filename = "pre-mapping-flat.fits";
    insert_synced_light(&fixture.database_a, filename);
    let image = fits_bytes("M 31", "2026-07-24T05:00:00");
    let checksum = sha256(&image);
    std::fs::write(fixture.images_a.join(filename), &image).unwrap();

    let second_root = fixture._directory.path().join("images-c");
    std::fs::create_dir_all(&second_root).unwrap();
    let reconfigured = fixture.reconfigured_state(
        RemoteImageUploadPlacement::Flat,
        &second_root,
        &[&fixture.images_a, &second_root],
    );
    let (status, body) = upload(
        reconfigured,
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &checksum,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["already_present"], true);
    assert!(fixture.images_a.join(filename).is_file());
    assert!(!second_root.join(filename).exists());
}

#[tokio::test]
async fn pre_mapping_flat_upload_rejects_any_differing_registered_sibling() {
    let fixture = Fixture::new();
    let filename = "pre-mapping-conflict.fits";
    insert_synced_light(&fixture.database_a, filename);
    let image = fits_bytes("M 31", "2026-07-24T05:00:00");
    let different = fits_bytes("M 31", "2026-07-24T05:10:00");
    std::fs::write(fixture.images_a.join(filename), &image).unwrap();

    let second_root = fixture._directory.path().join("images-c");
    std::fs::create_dir_all(&second_root).unwrap();
    std::fs::write(second_root.join(filename), different).unwrap();
    let reconfigured = fixture.reconfigured_state(
        RemoteImageUploadPlacement::Flat,
        &second_root,
        &[&fixture.images_a, &second_root],
    );

    let (status, body) = upload(
        reconfigured,
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &sha256(&image),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("different content"));
    assert_eq!(image_count(&fixture.database_a), 1);
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
async fn target_tree_places_calibration_frames_in_predictable_type_folders() {
    let fixture = Fixture::with_placement(RemoteImageUploadPlacement::TargetTree);
    let cases = [
        ("BIAS", "bias-placed.fits", "BIAS/bias-placed.fits"),
        ("DARK", "dark-placed.fits", "DARK/dark-placed.fits"),
        (
            "DARK FLAT",
            "dark-flat-placed.fits",
            "DARKFLAT/dark-flat-placed.fits",
        ),
        (
            "FLAT",
            "flat-placed.fits",
            "Calibration/FLAT/Ha/flat-placed.fits",
        ),
    ];

    for (index, (image_type, filename, relative)) in cases.iter().enumerate() {
        let image = fits_bytes_with_type(
            image_type,
            "Calibration",
            &format!("2026-07-24T09:{index:02}:00"),
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
        assert!(
            fixture.images_a.join(relative).is_file(),
            "missing {relative}"
        );
        assert!(!fixture.images_a.join(filename).exists());
    }
    assert_eq!(image_count(&fixture.database_a), 0);
    assert_eq!(calibration_count(&fixture.database_a), 4);
}

#[tokio::test]
async fn target_tree_flats_can_reuse_a_basename_across_targets_and_filters() {
    let fixture = Fixture::with_placement(RemoteImageUploadPlacement::TargetTree);
    let filename = "flat-001.fits";
    let first =
        fits_bytes_with_identity("FLAT", "Panel A", Some("Ha"), Some("2026-07-24T09:00:00"));
    let second =
        fits_bytes_with_identity("FLAT", "Panel B", Some("OIII"), Some("2026-07-24T09:01:00"));

    for image in [&first, &second, &first, &second] {
        let (status, body) = upload(
            fixture.state.clone(),
            "catalog-a",
            "catalog-a",
            TOKEN_A,
            filename,
            image,
            &sha256(image),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    assert!(fixture
        .images_a
        .join("Panel A/FLAT/Ha")
        .join(filename)
        .is_file());
    assert!(fixture
        .images_a
        .join("Panel B/FLAT/OIII")
        .join(filename)
        .is_file());
    assert_eq!(calibration_count(&fixture.database_a), 2);
}

#[tokio::test]
async fn calibration_retry_restores_its_missing_file_after_layout_and_root_changes() {
    let fixture = Fixture::with_placement(RemoteImageUploadPlacement::TargetTree);
    let filename = "flat-layout-retry.fits";
    let image = fits_bytes_with_type("FLAT", "Calibration", "2026-07-24T09:30:00");
    let checksum = sha256(&image);
    let target_path = fixture
        .images_a
        .join("Calibration")
        .join("FLAT")
        .join("Ha")
        .join(filename);

    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &checksum,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(target_path.is_file());
    let original_uuid: String = rusqlite::Connection::open(&fixture.database_a)
        .unwrap()
        .query_row(
            "SELECT frame_uuid FROM psf_guard_calibration_frame",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let (mapped_uuid, mapped_path, mapped_sha256): (String, String, String) =
        rusqlite::Connection::open(&fixture.database_a)
            .unwrap()
            .query_row(
                "SELECT frame_uuid, source_path, source_sha256
                 FROM psf_guard_remote_calibration_file",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
    assert_eq!(mapped_uuid, original_uuid);
    assert_eq!(std::path::Path::new(&mapped_path), target_path);
    assert_eq!(mapped_sha256, checksum);
    std::fs::remove_file(&target_path).unwrap();

    let second_root = fixture._directory.path().join("images-c");
    std::fs::create_dir_all(&second_root).unwrap();
    let flat_state = fixture.reconfigured_state(
        RemoteImageUploadPlacement::Flat,
        &second_root,
        &[&fixture.images_a, &second_root],
    );
    let (status, body) = upload(
        flat_state,
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &image,
        &checksum,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["already_present"], false);
    assert!(target_path.is_file());
    assert!(!second_root.join(filename).exists());
    assert_eq!(calibration_count(&fixture.database_a), 1);
    let restored_uuid: String = rusqlite::Connection::open(&fixture.database_a)
        .unwrap()
        .query_row(
            "SELECT frame_uuid FROM psf_guard_calibration_frame",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(restored_uuid, original_uuid);

    std::fs::remove_dir_all(fixture.images_a.join("Calibration")).unwrap();
    let changed = fits_bytes_with_type("FLAT", "Calibration", "2026-07-24T09:31:00");
    let (status, body) = upload(
        fixture.reconfigured_state(
            RemoteImageUploadPlacement::Flat,
            &second_root,
            &[&fixture.images_a, &second_root],
        ),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        filename,
        &changed,
        &sha256(&changed),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(!target_path.exists());
    assert!(!second_root.join(filename).exists());
    assert_eq!(calibration_count(&fixture.database_a), 1);
    let retained_uuid: String = rusqlite::Connection::open(&fixture.database_a)
        .unwrap()
        .query_row(
            "SELECT frame_uuid FROM psf_guard_calibration_frame",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained_uuid, original_uuid);
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

#[cfg(unix)]
#[tokio::test]
async fn final_filename_symlink_is_rejected() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let image = fits_bytes("M 42", "2026-07-24T06:00:00");
    let outside = fixture._directory.path().join("outside.fits");
    std::fs::write(&outside, &image).unwrap();
    symlink(&outside, fixture.images_a.join("linked.fits")).unwrap();

    let (status, body) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        "linked.fits",
        &image,
        &sha256(&image),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(image_count(&fixture.database_a), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn target_tree_parent_symlink_is_rejected() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::with_placement(RemoteImageUploadPlacement::TargetTree);
    let outside = fixture._directory.path().join("outside-target");
    std::fs::create_dir_all(&outside).unwrap();
    symlink(&outside, fixture.images_a.join("M 31")).unwrap();
    let image = fits_bytes("M 31", "2026-07-24T06:00:00");

    let (status, body) = upload(
        fixture.state,
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        "linked-parent.fits",
        &image,
        &sha256(&image),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(!outside.join("LIGHT").exists());
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

    let (status, _) = upload(
        fixture.state.clone(),
        "catalog-a",
        "catalog-a",
        TOKEN_A,
        "CON.fits",
        &image,
        &sha256(&image),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(image_count(&fixture.database_a), 0);
}
