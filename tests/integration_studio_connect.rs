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
    // A stand-in catalog route that is only a read and a write.
    let stand_ins = Router::new()
        .route(
            "/catalog",
            get(|| async { "catalog" }).put(|| async { "changed" }),
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

fn document() -> Value {
    serde_json::from_str(&psf_guard::studio::Document::new("Iris").to_json()).unwrap()
}

fn new_document() -> Value {
    json!({ "name": "Iris", "project_id": 11, "target_id": 17, "document": document() })
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
        call(Method::GET, "/api/db/test/studio-documents").await,
        StatusCode::OK
    );
    let (status, created, _) = send(
        &app,
        Method::POST,
        "/api/db/test/studio-documents",
        new_document(),
        None,
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
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

    // Signing out revokes the token it presents.
    let (status, _, _) = send(
        &app,
        Method::POST,
        "/api/auth/studio/disconnect",
        Value::Null,
        None,
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        call(Method::GET, "/api/catalog").await,
        StatusCode::UNAUTHORIZED
    );
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
        "/api/db/test/studio-documents",
        new_document(),
        None,
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = send(
        &app,
        Method::GET,
        "/api/db/test/studio-documents",
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

/// Sends raw bytes, as Studio uploads a file.
async fn upload(app: &Router, uri: &str, bytes: &[u8], token: &str) -> StatusCode {
    let request = Request::builder()
        .method(Method::PUT)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(bytes.to_vec()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap().status()
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test]
async fn studio_saves_revisions_files_and_exports_and_a_stale_save_conflicts() {
    let directory = tempfile::tempdir().unwrap();
    let app = app(&directory);
    let (token, _) = connected(&app, "editor").await;
    let base = "/api/db/test/studio-documents";
    let call = |method: Method, uri: String, body: Value| {
        let app = app.clone();
        let token = token.clone();
        async move {
            let (status, value, _) = send(&app, method, &uri, body, None, Some(&token)).await;
            (status, value)
        }
    };

    // The edited file goes up first; a wrong hash is refused.
    let edited = b"pixels an editor saved";
    let sha = sha256(edited);
    assert_eq!(
        upload(
            &app,
            &format!("{base}/files/{}", "0".repeat(64)),
            edited,
            &token
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        upload(&app, &format!("{base}/files/{sha}"), edited, &token).await,
        StatusCode::OK
    );
    let mut new = new_document();
    new["files"] = json!([{ "role": "edited", "sha256": sha, "name": "Iris.edit.tif" }]);
    let (status, created) = call(Method::POST, base.to_string(), new).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = created["data"]["id"].as_str().unwrap().to_string();

    // Two computers start from revision 1; the second save conflicts.
    let save = json!({ "base_revision": 1, "document": document() });
    let (status, saved) = call(Method::PUT, format!("{base}/{id}"), save.clone()).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["data"]["revisions"][1]["revision"], 2);
    assert_eq!(saved["data"]["revisions"][1]["saved_by"], "editor");
    let (status, conflict) = call(Method::PUT, format!("{base}/{id}"), save).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["data"]["revisions"][1]["revision"], 2);

    // A revision reads back as the document; the file downloads.
    let (status, revision) =
        call(Method::GET, format!("{base}/{id}/revisions/2"), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revision["name"], "Iris");
    let request = Request::builder()
        .uri(format!("{base}/files/{sha}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], edited);

    // An export of revision 2.
    let picture = b"an exported picture";
    let export_sha = sha256(picture);
    upload(&app, &format!("{base}/files/{export_sha}"), picture, &token).await;
    let (status, exported) = call(
        Method::POST,
        format!("{base}/{id}/exports"),
        json!({ "sha256": export_sha, "revision": 2, "name": "Iris.tif", "format": "tif" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{exported}");
    assert_eq!(exported["data"]["exports"][0]["size"], picture.len());

    let (_, listed) = call(Method::GET, format!("{base}?project=11"), Value::Null).await;
    assert_eq!(listed["data"][0]["revision"], 2);
    assert_eq!(listed["data"][0]["exports"], 1);

    // Delete removes it, and the files only it used.
    let (status, _) = call(Method::DELETE, format!("{base}/{id}"), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(Method::GET, format!("{base}/{id}"), Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(Method::GET, format!("{base}/files/{sha}"), Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
