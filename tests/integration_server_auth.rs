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
    auth_registry::{AccessRole, AuthRegistry, AuthUserRecord},
    config::ServerAuthConfig,
    server::{
        auth::{self, ServerAuth},
        handlers,
        state::AppState,
        user_admin,
    },
};
use rusqlite::Connection;
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

fn auth_config() -> ServerAuthConfig {
    ServerAuthConfig {
        session_hours: Some(1),
        secure_cookie: false,
        allow_read_only_compute: false,
    }
}

fn auth_registry() -> AuthRegistry {
    let mut registry = AuthRegistry::default();
    registry
        .add(
            AuthUserRecord::new("viewer", AccessRole::ReadOnly, "viewer-secret").unwrap(),
            false,
        )
        .unwrap();
    registry
        .add(
            AuthUserRecord::new("editor", AccessRole::ReadWrite, "editor-secret").unwrap(),
            false,
        )
        .unwrap();
    registry
}

fn app() -> Router {
    app_with_auth(auth_config())
}

fn app_with_auth(config: ServerAuthConfig) -> Router {
    let state = Arc::new(AppState::new_for_test(
        Connection::open_in_memory().unwrap(),
    ));
    state.set_server_auth(Some(
        ServerAuth::from_sources(Some(&config), &auth_registry())
            .unwrap()
            .unwrap(),
    ));
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
        .route(
            "/images/{image_id}/astrometry",
            post(|| async { "derived result" }),
        )
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::authorize_api,
        ))
        .with_state(state);

    Router::new().nest("/api", api)
}

fn user_management_app(directory: &tempfile::TempDir) -> Router {
    let database_registry_path = directory.path().join("config.json");
    let auth_registry_path = AuthRegistry::path_for_database_registry(&database_registry_path);
    let registry = auth_registry();
    registry.save(&auth_registry_path).unwrap();
    let config = ServerAuthConfig {
        session_hours: Some(1),
        secure_cookie: false,
        allow_read_only_compute: false,
    };
    let auth = ServerAuth::from_sources(Some(&config), &registry)
        .unwrap()
        .unwrap();
    let state = Arc::new(AppState::new_for_test(
        Connection::open_in_memory().unwrap(),
    ));
    state.set_server_auth(Some(auth));
    state.set_registry_path(Some(database_registry_path));

    let api = Router::new()
        .route("/auth/status", get(auth::status))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route(
            "/auth/users",
            get(user_admin::list_users).post(user_admin::create_user),
        )
        .route(
            "/auth/users/{username}",
            axum::routing::put(user_admin::update_user).delete(user_admin::remove_user),
        )
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::authorize_api,
        ))
        .with_state(state);
    Router::new().nest("/api", api)
}

#[tokio::test]
async fn viewer_compute_is_separate_from_read_access() {
    let app = app();
    let (_, viewer_cookie, _) = login(&app, "viewer", "viewer-secret").await;
    let compute = Request::builder()
        .method(Method::POST)
        .uri("/api/images/12/astrometry")
        .header(COOKIE, &viewer_cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(compute).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    let mut config = auth_config();
    config.allow_read_only_compute = true;
    let trusted_app = app_with_auth(config);
    let (_, trusted_viewer_cookie, _) = login(&trusted_app, "viewer", "viewer-secret").await;
    let compute = Request::builder()
        .method(Method::POST)
        .uri("/api/images/12/astrometry")
        .header(COOKIE, trusted_viewer_cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        trusted_app.oneshot(compute).await.unwrap().status(),
        StatusCode::OK
    );
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

    let (status, cookie, login) = login(&app, "viewer", "viewer-secret").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(login["data"]["role"], "read_only");
    assert_eq!(login["data"]["can_compute"], false);
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
    let (_, viewer_cookie, _) = login(&app, "viewer", "viewer-secret").await;
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

    let (_, editor_cookie, _) = login(&app, "editor", "editor-secret").await;
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

#[tokio::test]
async fn editor_manages_all_hashed_users_live() {
    let directory = tempfile::tempdir().unwrap();
    let app = user_management_app(&directory);
    let (_, viewer_cookie, _) = login(&app, "viewer", "viewer-secret").await;
    let viewer_list = Request::builder()
        .uri("/api/auth/users")
        .header(COOKIE, viewer_cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(viewer_list).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    let (_, editor_cookie, _) = login(&app, "editor", "editor-secret").await;
    let list = Request::builder()
        .uri("/api/auth/users")
        .header(COOKIE, &editor_cookie)
        .body(Body::empty())
        .unwrap();
    let (_, _, list) = json(&app, list).await;
    assert_eq!(list["data"][0]["username"], "editor");
    assert_eq!(list["data"][1]["username"], "viewer");

    let create = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/users")
        .header(COOKIE, &editor_cookie)
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "username": "guest",
                "role": "read_only",
                "password": "guest-password"
            })
            .to_string(),
        ))
        .unwrap();
    assert_eq!(json(&app, create).await.0, StatusCode::OK);
    let (_, guest_cookie, _) = login(&app, "guest", "guest-password").await;
    assert!(!guest_cookie.is_empty());

    let update = Request::builder()
        .method(Method::PUT)
        .uri("/api/auth/users/guest")
        .header(COOKIE, &editor_cookie)
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "role": "read_write",
                "password": "new-guest-password"
            })
            .to_string(),
        ))
        .unwrap();
    assert_eq!(json(&app, update).await.0, StatusCode::OK);
    let old_session = Request::builder()
        .uri("/api/auth/status")
        .header(COOKIE, guest_cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        json(&app, old_session).await.2["data"]["authenticated"],
        false
    );
    assert_eq!(
        login(&app, "guest", "guest-password").await.0,
        StatusCode::UNAUTHORIZED
    );
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        login(&app, "guest", "new-guest-password").await.0,
        StatusCode::OK
    );

    let remove = Request::builder()
        .method(Method::DELETE)
        .uri("/api/auth/users/guest")
        .header(COOKIE, &editor_cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(json(&app, remove).await.0, StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        login(&app, "guest", "new-guest-password").await.0,
        StatusCode::UNAUTHORIZED
    );

    let remove_self = Request::builder()
        .method(Method::DELETE)
        .uri("/api/auth/users/editor")
        .header(COOKIE, editor_cookie)
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = json(&app, remove_self).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("this session"));
}
