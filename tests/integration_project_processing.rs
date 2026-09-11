use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::get,
    Router,
};
use http_body_util::BodyExt;
use psf_guard::server::{exposure_groups, state::AppState};
use rusqlite::Connection;
use serde_json::{json, Value};
use tower::ServiceExt;

fn app() -> Router {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE project(Id INTEGER PRIMARY KEY,guid TEXT);
        INSERT INTO project VALUES(1,'first'),(2,'second');",
    )
    .unwrap();
    let state = Arc::new(AppState::new_for_test(conn));
    state.set_allow_database_management(true);
    Router::new()
        .route(
            "/api/db/{db_id}/projects/{project_id}/processing-settings",
            get(exposure_groups::get_project_settings)
                .put(exposure_groups::update_project_settings),
        )
        .with_state(state)
}

async fn request(app: &Router, project: i32, body: Option<Value>) -> (StatusCode, Value) {
    let mut request = Request::builder().uri(format!(
        "/api/db/test/projects/{project}/processing-settings"
    ));
    let body = match body {
        Some(body) => {
            request = request
                .method("PUT")
                .header("content-type", "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
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

#[tokio::test]
async fn project_setting_round_trip_is_isolated_and_reversible() {
    let app = app();
    let (status, initial) = request(&app, 1, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(initial["data"]["split_exposure_groups"], false);
    let (status, saved) = request(&app, 1, Some(json!({"split_exposure_groups":true}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["data"]["split_exposure_groups"], true);
    assert_eq!(
        request(&app, 1, None).await.1["data"]["split_exposure_groups"],
        true
    );
    assert_eq!(
        request(&app, 2, None).await.1["data"]["split_exposure_groups"],
        false
    );
    assert_eq!(
        request(&app, 1, Some(json!({"split_exposure_groups":false})))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, 1, None).await.1["data"]["split_exposure_groups"],
        false
    );
}

#[tokio::test]
async fn project_setting_rejects_missing_projects_and_invalid_payloads() {
    let app = app();
    assert_eq!(request(&app, 999, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        request(&app, 999, Some(json!({"split_exposure_groups":true})))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    for body in [
        json!({}),
        json!({"split_exposure_groups":"true"}),
        json!({"split_exposure_groups":true,"project_id":2}),
    ] {
        assert_eq!(
            request(&app, 1, Some(body)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        request(&app, 1, None).await.1["data"]["split_exposure_groups"],
        false
    );
}
