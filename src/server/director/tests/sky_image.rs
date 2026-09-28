use super::*;
use crate::server::director::sky_image::SkyImageService;
use axum::{extract::Query as AxumQuery, routing::get as axum_get};
use std::collections::HashMap;

const MOCK_JPEG: &[u8] = b"\xFF\xD8\xFFmock-jpeg";

async fn mock_provider() -> String {
    async fn hips2fits(AxumQuery(params): AxumQuery<HashMap<String, String>>) -> Response {
        assert_eq!(params.get("projection").map(String::as_str), Some("TAN"));
        if params.get("hips").map(String::as_str) == Some("CDS/P/Finkbeiner") {
            return StatusCode::NOT_FOUND.into_response();
        }
        (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "image/jpeg")],
            MOCK_JPEG,
        )
            .into_response()
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/hips2fits", axum_get(hips2fits)),
        )
        .await
        .unwrap();
    });
    format!("http://127.0.0.1:{port}/hips2fits")
}

async fn raw(app: &Router, path: &str) -> (StatusCode, Option<String>, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/director/v1{path}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = to_bytes(response.into_body(), 20_000_000).await.unwrap();
    (status, content_type, bytes.to_vec())
}

async fn settle(app: &Router, path: &str) -> (StatusCode, Option<String>, Vec<u8>) {
    for _ in 0..100 {
        let result = raw(app, path).await;
        if result.0 != StatusCode::ACCEPTED {
            return result;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("cutout never settled");
}

#[tokio::test]
async fn cutouts_are_fetched_once_off_the_request_path_and_failures_are_named() {
    let dir = TempDir::new().unwrap();
    let base = mock_provider().await;
    let installed = crate::server::director::sky_image::install_for_test(SkyImageService::new(
        dir.path(),
        &base,
    ));
    let enabled_state = Arc::new(state(&dir, true));
    let app = router(enabled_state.clone());

    let (status, surveys) = call(&app, "GET", "/sky/surveys", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = surveys["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"dss2_color") && ids.contains(&"finkbeiner_halpha"));
    assert_eq!(
        surveys["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == "finkbeiner_halpha")
            .unwrap()["kind"],
        "narrowband"
    );

    let good =
        "/sky/cutout?survey=dss2_color&ra=10.68&dec=41.27&fov=3&width=640&height=480&rotation=15";
    let (first, _, body) = raw(&app, good).await;
    assert_eq!(
        first,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let (status, content_type, bytes) = settle(&app, good).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("image/jpeg"));
    assert_eq!(bytes, MOCK_JPEG);
    let cached: Vec<_> = std::fs::read_dir(dir.path().join("director").join("sky"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(cached.len(), 1, "{cached:?}");
    assert!(cached[0].ends_with(".jpg") && !cached[0].starts_with('.'));
    // A nudge below display precision is the same cached image, served at once.
    let nudged = good.replace("ra=10.68", "ra=10.680001");
    assert_eq!(raw(&app, &nudged).await.0, StatusCode::OK);

    let missing = "/sky/cutout?survey=finkbeiner_halpha&ra=10.68&dec=41.27&fov=3";
    let (status, _, body) = settle(&app, missing).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(
        String::from_utf8_lossy(&body).contains("no coverage"),
        "{}",
        String::from_utf8_lossy(&body)
    );
    // Remembered briefly, so a busy view does not hammer the provider.
    assert_eq!(raw(&app, missing).await.0, StatusCode::BAD_GATEWAY);

    assert_eq!(
        raw(&app, "/sky/cutout?survey=CDS/P/DSS2/color&ra=1&dec=1&fov=1")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        raw(
            &app,
            "/sky/cutout?survey=dss2_color&ra=1&dec=1&fov=1&width=9000"
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        raw(
            &app,
            "/sky/cutout?survey=dss2_color&ra=1&dec=1&fov=1&extra=1"
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    drop(installed);

    let off = router(Arc::new(state(&dir, false)));
    assert_eq!(raw(&off, good).await.0, StatusCode::NOT_FOUND);
    assert_eq!(raw(&off, "/sky/surveys").await.0, StatusCode::NOT_FOUND);
}

/// An offline map under the cache root is a survey like any other: listed
/// with `offline: true`, rendered from its tiles off the request path, and
/// cached like a fetched cutout.
#[tokio::test]
async fn an_offline_sky_map_is_listed_and_rendered_from_its_tiles() {
    let dir = TempDir::new().unwrap();
    let maps = dir.path().join("maps");
    let folder = maps.join("FramingAssistantCache");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("CacheInfo.xml"),
        r#"<ImageCacheInfo><Image Id="a" RA="0.6667" Dec="41.2500" Rotation="0" FoVW="300" FoVH="300" FileName="t.jpg" Source="DSS" Name="t" /></ImageCacheInfo>"#,
    )
    .unwrap();
    let mut tile = image::RgbImage::new(64, 64);
    for p in tile.pixels_mut() {
        p.0 = [30, 200, 60];
    }
    tile.save(folder.join("t.jpg")).unwrap();
    // No provider is ever asked: point the service at a closed port.
    let service = Arc::new(
        SkyImageService::new(dir.path(), "http://127.0.0.1:9/hips2fits").with_sky_maps_root(&maps),
    );
    let listed = service.surveys();
    let offline = listed
        .iter()
        .find(|s| s.id == "nina:FramingAssistantCache")
        .expect("listed");
    assert_eq!(offline.name, "DSS (offline)");
    assert!(offline.offline);
    assert_eq!(
        offline.kind,
        crate::server::director::sky_image::SurveyKind::Broadband
    );
    assert!(listed.iter().any(|s| s.id == "dss2_color" && !s.offline));

    let query = serde_json::from_str(r#"{"survey":"nina:FramingAssistantCache","ra":10.0,"dec":41.25,"fov":2.0,"width":128,"height":96,"rotation":0.0}"#).unwrap();
    let cutout = service.cutout_from_query(&query).unwrap();
    assert_eq!(cutout.hips, None);
    let first = service.clone().respond(cutout.clone()).await;
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let mut bytes = Vec::new();
    for _ in 0..200 {
        let response = service.clone().respond(cutout.clone()).await;
        if response.status() == StatusCode::OK {
            bytes = to_bytes(response.into_body(), 10_000_000)
                .await
                .unwrap()
                .to_vec();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(!bytes.is_empty(), "offline render never settled");
    let img = image::load_from_memory(&bytes).unwrap().to_rgb8();
    assert_eq!((img.width(), img.height()), (128, 96));
    let centre = img.get_pixel(64, 48).0;
    assert!(centre[1] > 150 && centre[0] < 90, "{centre:?}");
    // Unknown layer ids are refused before anything is read.
    let bad = serde_json::from_str(r#"{"survey":"nina:nowhere","ra":10.0,"dec":41.25,"fov":2.0}"#)
        .unwrap();
    assert!(service.cutout_from_query(&bad).is_err());
}
