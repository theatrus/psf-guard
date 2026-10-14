//! Each signed-in user's view preferences at `/api/me/preferences`.

use axum::{
    body::Body,
    http::{header::COOKIE, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use psf_guard::{
    auth_registry::{AccessRole, AuthRegistry, AuthUserRecord},
    config::ServerAuthConfig,
    server::{auth::ServerAuth, state::AppState},
    user_preferences::{UserPreferencesRegistry, MAX_REQUEST_BYTES},
};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

fn registry() -> AuthRegistry {
    let mut registry = AuthRegistry::default();
    for (name, role) in [
        ("viewer", AccessRole::ReadOnly),
        ("editor", AccessRole::ReadWrite),
    ] {
        registry
            .add(
                AuthUserRecord::new(name, role, &format!("{name}-secret-pass")).unwrap(),
                false,
            )
            .unwrap();
    }
    registry
}

fn app(directory: &tempfile::TempDir, accounts: bool) -> Router {
    let database_registry_path = directory.path().join("config.json");
    let state = Arc::new(AppState::new_for_test(
        Connection::open_in_memory().unwrap(),
    ));
    if accounts {
        let registry = registry();
        registry
            .save(&AuthRegistry::path_for_database_registry(
                &database_registry_path,
            ))
            .unwrap();
        let config = ServerAuthConfig {
            session_hours: Some(1),
            secure_cookie: Some(false),
            allow_read_only_compute: false,
        };
        state.set_server_auth(Some(
            ServerAuth::from_sources(Some(&config), &registry, 3000)
                .unwrap()
                .unwrap(),
        ));
    }
    state.set_registry_path(Some(database_registry_path));
    Router::new().nest("/api", psf_guard::server::api_router(state))
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value)
}

async fn login(app: &Router, username: &str) -> String {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "username": username, "password": format!("{username}-secret-pass") })
                .to_string(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

async fn get(app: &Router, cookie: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::builder().uri("/api/me/preferences");
    if let Some(cookie) = cookie {
        request = request.header(COOKIE, cookie);
    }
    send(app, request.body(Body::empty()).unwrap()).await
}

async fn put(app: &Router, cookie: Option<&str>, body: impl Into<Body>) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(Method::PUT)
        .uri("/api/me/preferences")
        .header("content-type", "application/json");
    if let Some(cookie) = cookie {
        request = request.header(COOKIE, cookie);
    }
    send(app, request.body(body.into()).unwrap()).await
}

fn document(preferences: Value) -> String {
    json!({ "schema_version": 1, "preferences": preferences }).to_string()
}

#[tokio::test]
async fn each_user_reads_and_writes_only_their_own() {
    let directory = tempfile::tempdir().unwrap();
    let app = app(&directory, true);
    let viewer = login(&app, "viewer").await;
    let editor = login(&app, "editor").await;

    // Nothing saved yet.
    let (status, body) = get(&app, Some(&viewer)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["preferences"], json!({}));
    assert!(body["data"].get("updated_unix_seconds").is_none());

    // A viewer may save their own: a view preference changes no catalog.
    let (status, body) = put(
        &app,
        Some(&viewer),
        document(json!({ "imageOrder": "newest", "imageGrouping": "session" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["preferences"],
        json!({ "imageOrder": "newest", "imageGrouping": "session" })
    );
    assert!(body["data"]["updated_unix_seconds"].is_i64());

    // The editor sees none of it, and saves their own.
    let (_, body) = get(&app, Some(&editor)).await;
    assert_eq!(body["data"]["preferences"], json!({}));
    put(
        &app,
        Some(&editor),
        document(json!({ "libraryDensity": "detailed" })),
    )
    .await;

    let (_, body) = get(&app, Some(&viewer)).await;
    assert_eq!(
        body["data"]["preferences"],
        json!({ "imageOrder": "newest", "imageGrouping": "session" })
    );
    let (_, body) = get(&app, Some(&editor)).await;
    assert_eq!(
        body["data"]["preferences"],
        json!({ "libraryDensity": "detailed" })
    );

    // A second session of the same user, as another browser, reads the same.
    let elsewhere = login(&app, "viewer").await;
    let (_, body) = get(&app, Some(&elsewhere)).await;
    assert_eq!(body["data"]["preferences"]["imageOrder"], "newest");

    // A PUT replaces the document; there is no way to name another user.
    put(
        &app,
        Some(&viewer),
        document(json!({ "imageOrder": "oldest" })),
    )
    .await;
    let (_, body) = get(&app, Some(&viewer)).await;
    assert_eq!(
        body["data"]["preferences"],
        json!({ "imageOrder": "oldest" })
    );
    let (status, _) = send(
        &app,
        Request::builder()
            .uri("/api/me/preferences?username=editor")
            .header(COOKIE, &viewer)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stored =
        UserPreferencesRegistry::load(&directory.path().join("user-preferences.json")).unwrap();
    assert_eq!(
        Value::Object(stored.get("editor").unwrap().preferences.clone()),
        json!({ "libraryDensity": "detailed" })
    );
}

#[tokio::test]
async fn checks_what_it_keeps() {
    let directory = tempfile::tempdir().unwrap();
    let app = app(&directory, true);
    let viewer = login(&app, "viewer").await;

    // Unknown keys are dropped and named.
    let (status, body) = put(
        &app,
        Some(&viewer),
        document(json!({ "imageOrder": "newest", "fromTheFuture": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["preferences"],
        json!({ "imageOrder": "newest" })
    );
    assert_eq!(body["data"]["ignored"], json!(["fromTheFuture"]));

    // A known key with a value it cannot hold, another schema, or a body
    // that is not a document are refused, and nothing changes.
    for bad in [
        document(json!({ "imageOrder": "sideways" })),
        document(json!({ "gridThumbnailSize": 5000 })),
        json!({ "schema_version": 2, "preferences": {} }).to_string(),
        json!({ "preferences": {} }).to_string(),
        json!({ "schema_version": 1, "preferences": {}, "username": "editor" }).to_string(),
        "not json".to_string(),
    ] {
        let (status, body) = put(&app, Some(&viewer), bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    let (_, body) = get(&app, Some(&viewer)).await;
    assert_eq!(
        body["data"]["preferences"],
        json!({ "imageOrder": "newest" })
    );

    // A body past the cap is refused before it is read.
    let padding = "x".repeat(MAX_REQUEST_BYTES);
    let (status, body) = put(
        &app,
        Some(&viewer),
        document(json!({ "imageOrder": "newest", "pad": padding })),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
}

#[tokio::test]
async fn needs_a_signed_in_user() {
    let directory = tempfile::tempdir().unwrap();
    let app_with_accounts = app(&directory, true);
    let (status, _) = get(&app_with_accounts, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = put(&app_with_accounts, None, document(json!({}))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A server without accounts serves its own machine; the browser keeps
    // the preferences there.
    let directory = tempfile::tempdir().unwrap();
    let without_accounts = app(&directory, false);
    let (status, body) = get(&without_accounts, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, _) = put(
        &without_accounts,
        None,
        document(json!({ "imageOrder": "newest" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!directory.path().join("user-preferences.json").exists());
}

#[tokio::test]
async fn removing_a_user_drops_their_preferences() {
    let directory = tempfile::tempdir().unwrap();
    let app = app(&directory, true);
    let viewer = login(&app, "viewer").await;
    let editor = login(&app, "editor").await;
    put(
        &app,
        Some(&viewer),
        document(json!({ "imageOrder": "newest" })),
    )
    .await;
    put(
        &app,
        Some(&editor),
        document(json!({ "imageOrder": "oldest" })),
    )
    .await;

    let (status, body) = send(
        &app,
        Request::builder()
            .method(Method::DELETE)
            .uri("/api/auth/users/viewer")
            .header(COOKIE, &editor)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let stored =
        UserPreferencesRegistry::load(&directory.path().join("user-preferences.json")).unwrap();
    assert!(stored.get("viewer").is_none());
    assert!(stored.get("editor").is_some());
}
