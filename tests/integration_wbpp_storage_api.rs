//! WBPP storage preferences and unrestricted percentage limits over HTTP.
use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::get,
    Router,
};
use http_body_util::BodyExt;
use psf_guard::{
    db_registry::DbRegistry,
    server::{state::AppState, storage_settings, wbpp_run},
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

fn server(management: bool) -> (tempfile::TempDir, Arc<AppState>, Router) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.json");
    DbRegistry::default().save(&path).unwrap();
    let state = Arc::new(AppState::new_for_test(
        rusqlite::Connection::open_in_memory().unwrap(),
    ));
    state.databases.write().unwrap().clear();
    state.set_registry_path(Some(path));
    state.set_allow_database_management(management);
    let router = Router::new()
        .route(
            "/pixinsight",
            get(wbpp_run::get_pixinsight_settings).put(wbpp_run::update_pixinsight_settings),
        )
        .route(
            "/storage",
            get(storage_settings::get_storage_settings)
                .put(storage_settings::update_storage_settings),
        )
        .with_state(state.clone());
    (dir, state, router)
}
async fn put(router: &Router, path: &str, body: Value) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn wbpp_policy_is_validated_persisted_and_preserved_by_older_clients() {
    let (_dir, state, router) = server(true);
    let policy = json!({"max_volume_percent":20,"intermediate_days":14,"master_days":0});
    let (status, response) = put(
        &router,
        "/pixinsight",
        json!({"runs_dir":"/tmp/wbpp-api-test","storage":policy}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["data"]["storage"], policy);
    let (status, response) = put(
        &router,
        "/pixinsight",
        json!({"binary":null,"runs_dir":null}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["data"]["storage"], policy);
    let stored =
        DbRegistry::load_or_init(state.registry_path.read().unwrap().as_ref().unwrap()).unwrap();
    assert_eq!(stored.pixinsight.unwrap().storage.master_days, 0);
    for invalid in [json!({"max_volume_percent":101}), json!({"master_days":1})] {
        let (status, _) = put(&router, "/pixinsight", json!({"storage":invalid})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    state.shutdown.begin();
}
#[tokio::test]
async fn percentage_limits_accept_zero_and_values_below_fifty_but_reject_out_of_range() {
    let (_dir, state, router) = server(true);
    for limit in [0, 1, 20, 49, 50, 100] {
        let (status,response)=put(&router,"/storage",json!({"max_volume_percent":limit,"stack_max_volume_percent":limit,"calibration_max_volume_percent":limit})).await;
        assert_eq!(status, StatusCode::OK, "{response}");
        assert_eq!(response["data"]["max_volume_percent"], limit);
        assert_eq!(response["data"]["min_max_volume_percent"], 0);
        let (status, response) = put(
            &router,
            "/pixinsight",
            json!({"storage":{"max_volume_percent":limit}}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{response}");
        assert_eq!(response["data"]["storage"]["max_volume_percent"], limit);
    }
    for limit in [-1, 101] {
        assert_eq!(
            put(&router, "/storage", json!({"max_volume_percent":limit}))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    state.shutdown.begin();
}
#[tokio::test]
async fn naming_wbpp_storage_requires_database_management() {
    let (_dir, state, router) = server(false);
    assert_eq!(
        put(
            &router,
            "/pixinsight",
            json!({"runs_dir":"/tmp/not-authorized"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    state.shutdown.begin();
}
