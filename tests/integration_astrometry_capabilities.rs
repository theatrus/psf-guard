//! HTTP contract coverage for process-global Seiza capability diagnostics.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use http_body_util::BodyExt;
use psf_guard::astrometry::{AstrometryConfig, AstrometryResourceStatus};
use psf_guard::server::handlers;
use psf_guard::server::state::AppState;
use seiza::objects::{ObjectCatalog, ObjectKind, ObjectMetadata, SkyObject};
use serde_json::Value;
use tempfile::tempdir;
use tower::ServiceExt;

fn locked_version(name: &str) -> String {
    let lock = include_str!("../Cargo.lock")
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    lock["package"]
        .as_array_of_tables()
        .unwrap()
        .iter()
        .find(|package| package["name"].as_str() == Some(name))
        .unwrap()["version"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn cache_versions_follow_the_resolved_seiza_crates() {
    for (name, version) in [
        ("seiza", psf_guard::astrometry::SEIZA_VERSION),
        ("seiza-fits", psf_guard::astrometry::SEIZA_FITS_VERSION),
        (
            "seiza-satellites",
            psf_guard::satellites::SEIZA_SATELLITES_VERSION,
        ),
        (
            "seiza-background",
            env!("PSF_GUARD_SEIZA_BACKGROUND_VERSION"),
        ),
    ] {
        assert_eq!(version, locked_version(name), "{name} cache provenance");
    }
}

/// Stacks and masters key on the release whose output Seiza reproduces, so
/// an upgrade that changes no output keeps them. That release is never newer
/// than the crate itself.
#[test]
fn stack_caches_follow_seiza_stacking_output_not_its_version() {
    let parse = |version: &str| {
        version
            .split('.')
            .map(|part| part.parse::<u64>().unwrap())
            .collect::<Vec<_>>()
    };
    let output = psf_guard::server::stack_preview::SEIZA_STACKING_OUTPUT_VERSION;
    assert_eq!(output, seiza_stacking::OUTPUT_VERSION);
    assert!(parse(output) <= parse(&locked_version("seiza-stacking")));
    assert!(parse(seiza_stretch::OUTPUT_VERSION) <= parse(&locked_version("seiza-stretch")));
}

fn build_app(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/api/astrometry/capabilities",
            get(handlers::get_astrometry_capabilities),
        )
        .route(
            "/api/astrometry/catalogs/validate",
            post(handlers::validate_astrometry_catalogs),
        )
        .route(
            "/api/astrometry/catalogs/install",
            get(handlers::get_astrometry_catalog_install)
                .post(handlers::start_astrometry_catalog_install),
        )
        .with_state(state)
}

async fn request(app: Router, method: &str, uri: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}

fn write_object_catalog(path: &std::path::Path) {
    ObjectCatalog::new(vec![SkyObject {
        kind: ObjectKind::Galaxy,
        ra: 10.6848,
        dec: 41.2691,
        mag: Some(3.44),
        major_arcmin: Some(190.0),
        minor_arcmin: Some(60.0),
        position_angle_deg: Some(35.0),
        name: "NGC 224".to_string(),
        common_name: "Andromeda Galaxy".to_string(),
        metadata: ObjectMetadata {
            id: "openngc:NGC224".to_string(),
            source: "OpenNGC".to_string(),
            ..Default::default()
        },
    }])
    .write_to(path)
    .unwrap();
}

#[tokio::test]
async fn capabilities_and_validation_report_a_partial_data_directory() {
    let directory = tempdir().unwrap();
    let objects_path = directory.path().join("objects.bin");
    write_object_catalog(&objects_path);

    let state = Arc::new(
        AppState::from_databases_with_astrometry(
            vec![],
            directory
                .path()
                .join("cache")
                .to_string_lossy()
                .into_owned(),
            psf_guard::cli::PregenerationConfig::default(),
            Some(AstrometryConfig {
                data_dir: Some(directory.path().to_string_lossy().into_owned()),
                ..Default::default()
            }),
        )
        .unwrap(),
    );

    let (status, body) = request(
        build_app(Arc::clone(&state)),
        "GET",
        "/api/astrometry/capabilities",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["seiza_version"], locked_version("seiza"));
    assert_eq!(
        body["data"]["seiza_fits_version"],
        locked_version("seiza-fits")
    );
    assert_eq!(
        body["data"]["resources"]["objects"]["status"],
        serde_json::to_value(AstrometryResourceStatus::Available).unwrap()
    );
    assert_eq!(body["data"]["resources"]["objects"]["format"], "SEIZAOB4");
    assert_eq!(body["data"]["features"]["object_association"], true);
    assert_eq!(body["data"]["features"]["object_name_search"], false);
    assert_eq!(body["data"]["features"]["stellar_name_search"], false);
    assert_eq!(body["data"]["features"]["hinted_solve"], false);
    assert_eq!(body["data"]["features"]["blind_solve"], false);
    assert_eq!(body["data"]["features"]["transient_annotations"], false);
    assert_eq!(body["data"]["features"]["minor_body_annotations"], false);
    assert_eq!(body["data"]["resources"]["stars"]["status"], "missing");

    let (status, body) = request(
        build_app(state),
        "POST",
        "/api/astrometry/catalogs/validate",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["all_configured_valid"], false);
    let objects = body["data"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|resource| resource["name"] == "objects")
        .unwrap();
    assert_eq!(objects["validated"], true);
    assert_eq!(objects["status"], "available");
    let stars = body["data"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|resource| resource["name"] == "stars")
        .unwrap();
    assert_eq!(stars["validated"], false);
    assert_eq!(stars["status"], "missing");
}

#[tokio::test]
async fn missing_catalog_is_a_successful_diagnostic_response() {
    let directory = tempdir().unwrap();
    let state = Arc::new(
        AppState::from_databases_with_astrometry(
            vec![],
            directory
                .path()
                .join("cache")
                .to_string_lossy()
                .into_owned(),
            psf_guard::cli::PregenerationConfig::default(),
            Some(AstrometryConfig {
                objects: Some(
                    directory
                        .path()
                        .join("missing.bin")
                        .to_string_lossy()
                        .into_owned(),
                ),
                ..Default::default()
            }),
        )
        .unwrap(),
    );

    let (status, body) = request(build_app(state), "GET", "/api/astrometry/capabilities").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["resources"]["objects"]["status"], "missing");
    assert_eq!(body["data"]["features"]["object_association"], false);
}

#[tokio::test]
async fn catalog_install_reports_idle_until_started() {
    let directory = tempdir().unwrap();
    let state = Arc::new(
        AppState::from_databases_with_astrometry(
            vec![],
            directory
                .path()
                .join("cache")
                .to_string_lossy()
                .into_owned(),
            psf_guard::cli::PregenerationConfig::default(),
            Some(AstrometryConfig {
                data_dir: Some(
                    directory
                        .path()
                        .join("catalogs")
                        .to_string_lossy()
                        .into_owned(),
                ),
                ..Default::default()
            }),
        )
        .unwrap(),
    );

    let (status, body) = request(
        build_app(Arc::clone(&state)),
        "GET",
        "/api/astrometry/catalogs/install",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["progress"]["phase"], "idle");

    // Starting an install takes an editor and no database management: it
    // writes only into the configured catalog folder. A real start would
    // download catalogs, so the auth tests check only who may ask.
}
