use axum::{
    body::Body,
    http::{
        header::{CACHE_CONTROL, COOKIE},
        Method, Request, StatusCode,
    },
    middleware,
    routing::{get, post},
    Router,
};
use http_body_util::BodyExt;
use psf_guard::{
    config::{ServerAuthConfig, ServerAuthCredentialConfig},
    server::{
        auth::{self, ServerAuth},
        handlers,
        state::AppState,
    },
};
use rusqlite::Connection;
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

fn auth_config() -> ServerAuthConfig {
    ServerAuthConfig {
        read_only: Some(ServerAuthCredentialConfig {
            username: "viewer".into(),
            password: Some("view-secret".into()),
            password_file: None,
        }),
        read_write: Some(ServerAuthCredentialConfig {
            username: "editor".into(),
            password: Some("edit-secret".into()),
            password_file: None,
        }),
        session_hours: Some(1),
        secure_cookie: false,
    }
}

fn app() -> Router {
    let state = Arc::new(AppState::new_for_test(
        Connection::open_in_memory().unwrap(),
    ));
    state.set_server_auth(Some(ServerAuth::from_config(&auth_config()).unwrap()));
    state.set_allow_database_management(true);

    let api = Router::new()
        .route("/auth/status", get(auth::status))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/info", get(handlers::get_server_info))
        .route(
            "/catalog",
            get(|| async { "catalog" }).put(|| async { "changed" }),
        )
        .route(
            "/sync/v1/capabilities",
            post(|| async { "remote bearer route" }),
        )
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::authorize_api,
        ))
        .with_state(state);

    Router::new().nest("/api", api)
}

async fn json(app: &Router, request: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, headers, value)
}

async fn login(app: &Router, username: &str, password: &str) -> (StatusCode, String, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "username": username, "password": password }).to_string(),
        ))
        .unwrap();
    let (status, headers, json) = json(app, request).await;
    let cookie = headers
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .to_string();
    (status, cookie, json)
}

#[tokio::test]
async fn login_status_and_logout_use_an_http_only_session_cookie() {
    let app = app();
    let unauthenticated = Request::builder()
        .uri("/api/auth/status")
        .body(Body::empty())
        .unwrap();
    let (_, headers, status) = json(&app, unauthenticated).await;
    assert_eq!(headers[CACHE_CONTROL], "no-store");
    assert_eq!(status["data"]["authenticated"], false);

    let (status, cookie, login) = login(&app, "viewer", "view-secret").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(login["data"]["role"], "read_only");
    assert!(cookie.starts_with("psf_guard_session="));

    let authenticated = Request::builder()
        .uri("/api/auth/status")
        .header(COOKIE, &cookie)
        .body(Body::empty())
        .unwrap();
    let (_, _, status) = json(&app, authenticated).await;
    assert_eq!(status["data"]["authenticated"], true);
    assert_eq!(status["data"]["username"], "viewer");

    let logout = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/logout")
        .header(COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let (status, headers, _) = json(&app, logout).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(headers
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("Max-Age=0"));
}

#[tokio::test]
async fn viewer_reads_but_editor_writes() {
    let app = app();
    let (_, viewer_cookie, _) = login(&app, "viewer", "view-secret").await;
    let read = Request::builder()
        .uri("/api/catalog")
        .header(COOKIE, &viewer_cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(read).await.unwrap().status(),
        StatusCode::OK
    );
    let viewer_info = Request::builder()
        .uri("/api/info")
        .header(COOKIE, &viewer_cookie)
        .body(Body::empty())
        .unwrap();
    let (_, headers, viewer_info) = json(&app, viewer_info).await;
    assert_eq!(headers[CACHE_CONTROL], "private");
    assert_eq!(viewer_info["data"]["allow_database_management"], false);

    let write = Request::builder()
        .method(Method::PUT)
        .uri("/api/catalog")
        .header(COOKIE, viewer_cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(write).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    let (_, editor_cookie, _) = login(&app, "editor", "edit-secret").await;
    let editor_info = Request::builder()
        .uri("/api/info")
        .header(COOKIE, &editor_cookie)
        .body(Body::empty())
        .unwrap();
    let (_, _, editor_info) = json(&app, editor_info).await;
    assert_eq!(editor_info["data"]["allow_database_management"], true);

    let write = Request::builder()
        .method(Method::PUT)
        .uri("/api/catalog")
        .header(COOKIE, editor_cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.oneshot(write).await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn protected_api_challenges_but_remote_bearer_routes_stay_separate() {
    let app = app();
    let protected = Request::builder()
        .uri("/api/catalog")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(protected).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    let bad_login = login(&app, "viewer", "wrong").await;
    assert_eq!(bad_login.0, StatusCode::UNAUTHORIZED);

    let remote = Request::builder()
        .method(Method::POST)
        .uri("/api/sync/v1/capabilities")
        .header("authorization", "Bearer remote-key")
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.oneshot(remote).await.unwrap().status(), StatusCode::OK);
}
