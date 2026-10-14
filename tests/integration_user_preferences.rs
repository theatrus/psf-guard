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
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
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

async fn patch(app: &Router, cookie: Option<&str>, body: impl Into<Body>) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(Method::PATCH)
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

/// An endless request body that counts what the server reads of it.
struct CountedBody {
    read: Arc<AtomicUsize>,
}

impl tokio::io::AsyncRead for CountedBody {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let room = buffer.remaining().min(1024);
        buffer.put_slice(&vec![b' '; room]);
        self.read.fetch_add(room, Ordering::SeqCst);
        std::task::Poll::Ready(Ok(()))
    }
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
async fn keeps_the_scoring_defaults() {
    let directory = tempfile::tempdir().unwrap();
    let app = app(&directory, true);
    let viewer = login(&app, "viewer").await;
    let editor = login(&app, "editor").await;

    let scoring = json!({
        "imageOrder": "newest",
        "scoringPenaltySatellite": 0,
        "scoringPenaltyPointing": 1,
        "scoringPenaltyTemporal": 1.5,
        "scoringHfrRejectAbove": 2.55,
        "scoringStarCountRejectBelow": null,
    });
    let (status, body) = put(&app, Some(&viewer), document(scoring.clone())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["preferences"], scoring);
    let (_, body) = get(&app, Some(&viewer)).await;
    assert_eq!(body["data"]["preferences"], scoring);
    // They are the viewer's alone.
    let (_, body) = get(&app, Some(&editor)).await;
    assert_eq!(body["data"]["preferences"], json!({}));

    // A penalty or limit out of range is refused, and the saved ones stay.
    for bad in [
        json!({ "scoringPenaltySatellite": 3 }),
        json!({ "scoringPenaltySatellite": 0.35 }),
        json!({ "scoringHfrRejectAbove": 0 }),
        json!({ "scoringStarCountRejectBelow": "500" }),
        json!({ "scoringStarCountRejectBelow": 12.5 }),
    ] {
        let (status, body) = put(&app, Some(&viewer), document(bad.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    let (_, body) = get(&app, Some(&viewer)).await;
    assert_eq!(body["data"]["preferences"], scoring);
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

#[tokio::test]
async fn a_patch_merges_the_keys_it_names_into_the_callers_document() {
    let directory = tempfile::tempdir().unwrap();
    let app = app(&directory, true);
    let viewer = login(&app, "viewer").await;
    let editor = login(&app, "editor").await;

    // A viewer may patch their own, as they may PUT it.
    let (status, body) = patch(
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

    // Another browser of the same person changes one other key: the first
    // two stay.
    let elsewhere = login(&app, "viewer").await;
    let (_, body) = patch(
        &app,
        Some(&elsewhere),
        document(json!({ "gridThumbnailSize": 450 })),
    )
    .await;
    assert_eq!(
        body["data"]["preferences"],
        json!({ "imageOrder": "newest", "imageGrouping": "session", "gridThumbnailSize": 450 })
    );

    // A key set to null goes back to the default; unknown keys are named.
    let (status, body) = patch(
        &app,
        Some(&viewer),
        document(json!({ "imageGrouping": null, "fromTheFuture": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["preferences"],
        json!({ "imageOrder": "newest", "gridThumbnailSize": 450 })
    );
    assert_eq!(body["data"]["ignored"], json!(["fromTheFuture"]));

    // The editor's document is their own.
    let (_, body) = get(&app, Some(&editor)).await;
    assert_eq!(body["data"]["preferences"], json!({}));
    patch(
        &app,
        Some(&editor),
        document(json!({ "libraryDensity": "detailed" })),
    )
    .await;
    let (_, body) = get(&app, Some(&viewer)).await;
    assert_eq!(
        body["data"]["preferences"],
        json!({ "imageOrder": "newest", "gridThumbnailSize": 450 })
    );

    // An old tab's PUT still replaces the whole document.
    put(
        &app,
        Some(&viewer),
        document(json!({ "showAllChip": false })),
    )
    .await;
    let (_, body) = get(&app, Some(&viewer)).await;
    assert_eq!(body["data"]["preferences"], json!({ "showAllChip": false }));
    let (_, body) = get(&app, Some(&editor)).await;
    assert_eq!(
        body["data"]["preferences"],
        json!({ "libraryDensity": "detailed" })
    );
}

#[tokio::test]
async fn a_patch_is_checked_like_a_put() {
    let directory = tempfile::tempdir().unwrap();
    let app = app(&directory, true);
    let viewer = login(&app, "viewer").await;
    patch(
        &app,
        Some(&viewer),
        document(json!({ "imageOrder": "newest" })),
    )
    .await;

    for bad in [
        document(json!({ "imageOrder": "sideways" })),
        document(json!({ "gridThumbnailSize": 5000 })),
        document(json!({ "showAllChip": "no" })),
        json!({ "schema_version": 2, "preferences": {} }).to_string(),
        json!({ "preferences": {} }).to_string(),
        json!({ "schema_version": 1, "preferences": {}, "username": "editor" }).to_string(),
        "not json".to_string(),
    ] {
        let (status, body) = patch(&app, Some(&viewer), bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    // A PUT may not reset a key with null; only a PATCH names a reset.
    let (status, _) = put(&app, Some(&viewer), document(json!({ "imageOrder": null }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, body) = get(&app, Some(&viewer)).await;
    assert_eq!(
        body["data"]["preferences"],
        json!({ "imageOrder": "newest" })
    );

    // Signed out, or with no accounts, there is nothing to patch.
    let (status, _) = patch(&app, None, document(json!({}))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let without = tempfile::tempdir().unwrap();
    let (status, _) = patch(
        &app_without_accounts(&without),
        None,
        document(json!({ "imageOrder": "newest" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!without.path().join("user-preferences.json").exists());
}

fn app_without_accounts(directory: &tempfile::TempDir) -> Router {
    app(directory, false)
}

#[tokio::test]
async fn an_oversized_body_is_refused_at_the_limit_not_after_two_megabytes() {
    let directory = tempfile::tempdir().unwrap();
    let app = app(&directory, true);
    let viewer = login(&app, "viewer").await;

    for method in [Method::PATCH, Method::PUT] {
        // An endless body with no length: the server stops reading at the
        // route's limit, far short of axum's 2 MiB default.
        let read = Arc::new(AtomicUsize::new(0));
        let stream = tokio_util::io::ReaderStream::new(CountedBody {
            read: Arc::clone(&read),
        });
        let (status, body) = send(
            &app,
            Request::builder()
                .method(method.clone())
                .uri("/api/me/preferences")
                .header("content-type", "application/json")
                .header(COOKIE, &viewer)
                .body(Body::from_stream(stream))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{method}: {body}");
        assert!(
            body["error"].as_str().unwrap().contains("limited to"),
            "{body}"
        );
        let consumed = read.load(Ordering::SeqCst);
        assert!(
            consumed <= MAX_REQUEST_BYTES + 64 * 1024,
            "{method} read {consumed} bytes"
        );
    }

    // One that says its length up front is refused the same way.
    let padding = "x".repeat(MAX_REQUEST_BYTES);
    let (status, _) = patch(
        &app,
        Some(&viewer),
        document(json!({ "imageOrder": "newest", "pad": padding })),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (_, body) = get(&app, Some(&viewer)).await;
    assert_eq!(body["data"]["preferences"], json!({}));
}

#[tokio::test]
async fn concurrent_patches_lose_no_key() {
    let directory = tempfile::tempdir().unwrap();
    let app = app(&directory, true);
    // Two users, and two sessions of one of them, as two browsers.
    let sessions = [
        login(&app, "viewer").await,
        login(&app, "viewer").await,
        login(&app, "editor").await,
    ];
    let changes = [
        ("showNightChip", json!(false)),
        ("showAllChip", json!(false)),
        ("showCalibrationChip", json!(false)),
        ("advanceOnGrade", json!(false)),
        ("libraryDensity", json!("detailed")),
        ("projectPickerGrouping", json!("database")),
        ("imageOrder", json!("newest")),
        ("imageGrouping", json!("date")),
        ("gridThumbnailSize", json!(450)),
        ("sequenceThumbnailSize", json!(200)),
    ];
    let mut tasks = Vec::new();
    for (index, (key, value)) in changes.iter().enumerate() {
        // The viewer's keys alternate between their two browsers; the
        // editor sends each key alongside.
        let viewer_session = sessions[index % 2].clone();
        let editor_session = sessions[2].clone();
        let app = app.clone();
        let body = document(json!({ *key: value.clone() }));
        tasks.push(tokio::spawn(async move {
            let (viewer, editor) = tokio::join!(
                patch(&app, Some(&viewer_session), body.clone()),
                patch(&app, Some(&editor_session), body),
            );
            assert_eq!(viewer.0, StatusCode::OK, "{}", viewer.1);
            assert_eq!(editor.0, StatusCode::OK, "{}", editor.1);
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let expected = Value::Object(
        changes
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect(),
    );
    for session in &sessions {
        let (_, body) = get(&app, Some(session)).await;
        assert_eq!(body["data"]["preferences"], expected);
    }
}

#[tokio::test]
async fn a_removed_users_entry_is_pruned_and_not_written_back() {
    let directory = tempfile::tempdir().unwrap();
    let app = app(&directory, true);
    let viewer = login(&app, "viewer").await;
    let editor = login(&app, "editor").await;
    let preferences_path = directory.path().join("user-preferences.json");
    // An entry for an account that is gone, as a CLI removal that could not
    // write this file leaves behind.
    let mut stored = UserPreferencesRegistry::default();
    stored
        .set(
            "ghost",
            json!({ "imageOrder": "newest" })
                .as_object()
                .unwrap()
                .clone(),
            1,
        )
        .unwrap();
    stored.save(&preferences_path).unwrap();

    // The next save by anyone drops it.
    patch(
        &app,
        Some(&viewer),
        document(json!({ "imageOrder": "newest" })),
    )
    .await;
    let loaded = UserPreferencesRegistry::load(&preferences_path).unwrap();
    assert!(loaded.get("ghost").is_none());
    assert!(loaded.get("viewer").is_some());

    // Removing the viewer ends their sessions: a tab still open cannot
    // write their entry back.
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
    let (status, _) = patch(
        &app,
        Some(&viewer),
        document(json!({ "imageOrder": "oldest" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let loaded = UserPreferencesRegistry::load(&preferences_path).unwrap();
    assert!(loaded.get("viewer").is_none());
}

#[tokio::test]
async fn the_server_drops_removed_users_preferences_when_it_starts() {
    let directory = tempfile::tempdir().unwrap();
    let database_registry_path = directory.path().join("config.json");
    let preferences_path = directory.path().join("user-preferences.json");
    let mut stored = UserPreferencesRegistry::default();
    for name in ["viewer", "ghost"] {
        stored
            .set(
                name,
                json!({ "imageOrder": "newest" })
                    .as_object()
                    .unwrap()
                    .clone(),
                1,
            )
            .unwrap();
    }
    stored.save(&preferences_path).unwrap();
    let auth = ServerAuth::from_sources(
        Some(&ServerAuthConfig {
            session_hours: Some(1),
            secure_cookie: Some(false),
            allow_read_only_compute: false,
        }),
        &registry(),
        3000,
    )
    .unwrap()
    .unwrap();
    psf_guard::server::user_preferences::forget_removed_users(&auth, &database_registry_path);
    let loaded = UserPreferencesRegistry::load(&preferences_path).unwrap();
    assert!(loaded.get("ghost").is_none());
    assert!(loaded.get("viewer").is_some());
}
