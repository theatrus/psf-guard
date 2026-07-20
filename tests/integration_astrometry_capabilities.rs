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
    assert_eq!(body["data"]["seiza_version"], "0.10.0");
    assert_eq!(body["data"]["seiza_fits_version"], "0.1.6");
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
