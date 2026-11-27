//! Connecting PSF Studio: a code approved by a signed-in person, or a link
//! ticket, traded for a Studio token, and what that token may do.

use axum::{
    body::Body,
    http::{header::COOKIE, Method, Request, StatusCode},
    middleware,
    routing::get,
    Router,
};
use http_body_util::BodyExt;
use psf_guard::{
    auth_registry::{AccessRole, AuthRegistry, AuthUserRecord},
    config::ServerAuthConfig,
    server::{
        auth::{self, ServerAuth},
        state::AppState,
    },
};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

fn app(directory: &tempfile::TempDir) -> Router {
    let database_registry_path = directory.path().join("config.json");
    let auth_registry_path = AuthRegistry::path_for_database_registry(&database_registry_path);
    let mut registry = AuthRegistry::default();
    for (name, role) in [
        ("viewer", AccessRole::ReadOnly),
        ("editor", AccessRole::ReadWrite),
    ] {
        registry
            .add(
                AuthUserRecord::new(name, role, &format!("{name}-secret")).unwrap(),
                false,
            )
            .unwrap();
    }
    registry.save(&auth_registry_path).unwrap();
    let config = ServerAuthConfig {
        session_hours: Some(1),
        secure_cookie: Some(false),
        allow_read_only_compute: false,
    };
    let state = Arc::new(AppState::new_for_test(
        Connection::open_in_memory().unwrap(),
    ));
    state.set_server_auth(Some(
        ServerAuth::from_sources(Some(&config), &registry, 3000)
            .unwrap()
            .unwrap(),
    ));
    state.set_registry_path(Some(database_registry_path));
    let real = psf_guard::server::api_router(Arc::clone(&state));
    // Stand-ins: a catalog route, and a database's Studio documents.
    let stand_ins = Router::new()
        .route(
            "/catalog",
            get(|| async { "catalog" }).put(|| async { "changed" }),
        )
        .route(
            "/db/{db}/studio-documents",
            get(|| async { "documents" }).post(|| async { "saved" }),
        )
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::authorize_api,
        ))
        .with_state(Arc::clone(&state));
    Router::new().nest("/api", real.merge(stand_ins))
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: Value,
    cookie: Option<&str>,
    token: Option<&str>,
) -> (StatusCode, Value, Option<String>) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(cookie) = cookie {
        request = request.header(COOKIE, cookie);
    }
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let cookie = response.headers().get("set-cookie").map(|value| {
        value
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    });
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value, cookie)
}

async fn login(app: &Router, username: &str) -> String {
    let (status, _, cookie) = send(
        app,
        Method::POST,
        "/api/auth/login",
        json!({"username": username, "password": format!("{username}-secret")}),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    cookie.unwrap()
}

/// Connect, allow as `username`, and collect the token.
async fn connected(app: &Router, username: &str) -> (String, Value) {
    let (status, started, _) = send(
        app,
        Method::POST,
        "/api/auth/studio/connect",
        json!({"client_name": "Observatory Mac"}),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let started = &started["data"];
    let connect_id = started["connect_id"].as_str().unwrap().to_string();
    let code = started["user_code"].as_str().unwrap().to_string();
    assert_eq!(
        started["approve_path"],
        format!("/#/studio-connect?code={code}")
    );
    let trade = json!({"connect_id": connect_id});
    let (_, waiting, _) = send(
        app,
        Method::POST,
        "/api/auth/studio/token",
        trade.clone(),
        None,
        None,
    )
    .await;
    assert_eq!(waiting["data"]["status"], "waiting");

    let cookie = login(app, username).await;
    let (status, shown, _) = send(
        app,
        Method::GET,
        &format!("/api/auth/studio/approve?code={}", code.to_lowercase()),
        Value::Null,
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["data"]["client_name"], "Observatory Mac");
    let (status, _, _) = send(
        app,
        Method::POST,
        "/api/auth/studio/approve",
        json!({"code": code, "allow": true}),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, approved, _) = send(
        app,
        Method::POST,
        "/api/auth/studio/token",
        trade.clone(),
        None,
        None,
    )
    .await;
    let approved = approved["data"].clone();
    assert_eq!(approved["status"], "approved", "{approved}");
    // Answered once.
    let (_, again, _) = send(
        app,
        Method::POST,
        "/api/auth/studio/token",
        trade,
        None,
        None,
    )
    .await;
    assert_eq!(again["data"]["status"], "expired");
    (approved["token"].as_str().unwrap().to_string(), approved)
}

#[tokio::test]
async fn an_editor_connects_studio_which_reads_and_saves_documents_only() {
    let directory = tempfile::tempdir().unwrap();
    let app = app(&directory);
    let (token, approved) = connected(&app, "editor").await;
    assert_eq!(approved["username"], "editor");
    assert_eq!(approved["role"], "read_write");
    assert_eq!(approved["saves_documents"], true);
    assert!(approved["expires_at"].is_i64());

    let call = |method, uri: &'static str| {
        let app = app.clone();
        let token = token.clone();
        async move {
            send(&app, method, uri, json!({}), None, Some(&token))
                .await
                .0
        }
    };
    assert_eq!(call(Method::GET, "/api/catalog").await, StatusCode::OK);
    assert_eq!(
        call(Method::GET, "/api/db/main/studio-documents").await,
        StatusCode::OK
    );
    assert_eq!(
        call(Method::POST, "/api/db/main/studio-documents").await,
        StatusCode::OK
    );
    // Nothing else is written with it, although its user is an editor.
    assert_eq!(
        call(Method::PUT, "/api/catalog").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(call(Method::POST, "/api/mcp").await, StatusCode::FORBIDDEN);
    assert_eq!(
        call(Method::GET, "/api/auth/tokens").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(Method::POST, "/api/auth/studio/ticket").await,
        StatusCode::FORBIDDEN
    );

    // The token list shows it as a Studio connection.
    let cookie = login(&app, "editor").await;
    let (_, tokens, _) = send(
        &app,
        Method::GET,
        "/api/auth/tokens",
        Value::Null,
        Some(&cookie),
        None,
    )
    .await;
    let studio = tokens["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|token| token["scope"] == "studio")
        .expect("a Studio token");
    assert_eq!(studio["label"], "PSF Studio on Observatory Mac");
}

#[tokio::test]
async fn a_viewer_connects_studio_which_cannot_save_documents() {
    let directory = tempfile::tempdir().unwrap();
    let app = app(&directory);
    let (token, approved) = connected(&app, "viewer").await;
    assert_eq!(approved["saves_documents"], false);
    let (status, _, _) = send(
        &app,
        Method::POST,
        "/api/db/main/studio-documents",
        json!({}),
        None,
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = send(
        &app,
        Method::GET,
        "/api/db/main/studio-documents",
        Value::Null,
        None,
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_denied_code_gives_no_token_and_a_wrong_code_is_not_found() {
    let directory = tempfile::tempdir().unwrap();
    let app = app(&directory);
    let (_, started, _) = send(
        &app,
        Method::POST,
        "/api/auth/studio/connect",
        json!({}),
        None,
        None,
    )
    .await;
    let connect_id = started["data"]["connect_id"].as_str().unwrap().to_string();
    let code = started["data"]["user_code"].as_str().unwrap().to_string();
    let cookie = login(&app, "editor").await;
    let (status, _, _) = send(
        &app,
        Method::POST,
        "/api/auth/studio/approve",
        json!({"code": "AAAA-AAAA", "allow": true}),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Approval needs a signed-in person.
    let (status, _, _) = send(
        &app,
        Method::POST,
        "/api/auth/studio/approve",
        json!({"code": code, "allow": true}),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    send(
        &app,
        Method::POST,
        "/api/auth/studio/approve",
        json!({"code": code, "allow": false}),
        Some(&cookie),
        None,
    )
    .await;
    let (_, denied, _) = send(
        &app,
        Method::POST,
        "/api/auth/studio/token",
        json!({"connect_id": connect_id}),
        None,
        None,
    )
    .await;
    assert_eq!(denied["data"]["status"], "denied");
    assert!(denied["data"].get("token").is_none());
}

#[tokio::test]
async fn a_link_ticket_trades_once_for_a_token() {
    let directory = tempfile::tempdir().unwrap();
    let app = app(&directory);
    let cookie = login(&app, "editor").await;
    let (status, issued, _) = send(
        &app,
        Method::POST,
        "/api/auth/studio/ticket",
        Value::Null,
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ticket = issued["data"]["ticket"].as_str().unwrap().to_string();
    let trade = json!({"ticket": ticket, "client_name": "Laptop"});
    let (_, approved, _) = send(
        &app,
        Method::POST,
        "/api/auth/studio/token",
        trade.clone(),
        None,
        None,
    )
    .await;
    assert_eq!(approved["data"]["status"], "approved");
    assert_eq!(approved["data"]["username"], "editor");
    let (_, again, _) = send(
        &app,
        Method::POST,
        "/api/auth/studio/token",
        trade,
        None,
        None,
    )
    .await;
    assert_eq!(again["data"]["status"], "expired");
}
