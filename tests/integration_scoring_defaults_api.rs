//! The server's scoring defaults over HTTP: viewers read them, editors
//! change them, each value is checked as the Scoring control checks it, the
//! registry keeps them, and a change refreshes every database's stacks when
//! automatic previews are on.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{header::COOKIE, Method, Request, StatusCode},
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
        database_context::DatabaseContext,
        scoring_defaults,
        stack_preview::automatic::{self, AutomationPolicy},
        state::AppState,
    },
};
use rusqlite::Connection;
use serde_json::{json, Value};
use tower::ServiceExt;

/// The defaults and the automation switch are process-wide; the tests take
/// turns.
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Server {
    dir: tempfile::TempDir,
    state: Arc<AppState>,
    app: Router,
}

/// A server with a viewer and an editor, a registry file, and one catalog.
fn server() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let database = root.join("rig.sqlite");
    let connection = Connection::open(&database).unwrap();
    psf_guard::ts_schema::apply_schema(&connection).unwrap();
    drop(connection);

    let mut users = AuthRegistry::default();
    users
        .add(
            AuthUserRecord::new("viewer", AccessRole::ReadOnly, "viewer-secret").unwrap(),
            false,
        )
        .unwrap();
    users
        .add(
            AuthUserRecord::new("editor", AccessRole::ReadWrite, "editor-secret").unwrap(),
            false,
        )
        .unwrap();
    let config = ServerAuthConfig {
        session_hours: Some(1),
        secure_cookie: Some(false),
        allow_read_only_compute: false,
    };
    let state = Arc::new(AppState::new_for_test(
        Connection::open_in_memory().unwrap(),
    ));
    state.set_server_auth(Some(
        ServerAuth::from_sources(Some(&config), &users, 3000)
            .unwrap()
            .unwrap(),
    ));
    state.set_registry_path(Some(root.join("config.json")));
    state.set_allow_database_management(false);
    let context = DatabaseContext::new(
        "rig".into(),
        "Observatory".into(),
        database.display().to_string(),
        vec![root.display().to_string()],
        None,
        None,
        None,
        root.join("cache"),
    )
    .unwrap();
    state
        .databases
        .write()
        .unwrap()
        .insert("rig".into(), Arc::new(context));

    let api = Router::new()
        .route("/auth/login", post(auth::login))
        .route(
            "/settings/scoring",
            get(scoring_defaults::get_scoring_defaults)
                .put(scoring_defaults::update_scoring_defaults),
        )
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::authorize_api,
        ))
        .with_state(Arc::clone(&state));
    Server {
        dir,
        state,
        app: Router::new().nest("/api", api),
    }
}

async fn send(
    app: &Router,
    method: Method,
    cookie: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri("/api/settings/scoring")
        .header(COOKIE, cookie);
    let request = match body {
        Some(body) => request
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value)
}

async fn login(app: &Router, username: &str, password: &str) -> String {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "username": username, "password": password }).to_string(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn scoring(satellite: f64, hfr_reject_above: Option<f64>) -> Value {
    json!({
        "penalty_satellite": satellite,
        "penalty_pointing": 1.0,
        "penalty_temporal": 1.0,
        "penalty_hfr_rise": 1.0,
        "penalty_star_loss": 1.0,
        "hfr_reject_above": hfr_reject_above,
        "star_count_reject_below": null,
    })
}

/// Put the built-in defaults and automation off back in force.
fn reset() {
    scoring_defaults::configure(None);
    automatic::configure(AutomationPolicy::default());
}

#[tokio::test]
async fn viewers_read_the_defaults_and_only_editors_save_them() {
    let _turn = TURN.lock().await;
    reset();
    let server = server();
    let viewer = login(&server.app, "viewer", "viewer-secret").await;
    let editor = login(&server.app, "editor", "editor-secret").await;

    // Nothing stored yet: the built-in values, and a page may copy its own.
    let (status, body) = send(&server.app, Method::GET, &viewer, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["stored"], false);
    assert_eq!(body["data"]["scoring"], scoring(1.0, None));
    assert_eq!(body["data"]["builtin"], scoring(1.0, None));

    let (status, _) = send(
        &server.app,
        Method::PUT,
        &viewer,
        Some(scoring(0.5, Some(3.0))),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = send(
        &server.app,
        Method::PUT,
        &editor,
        Some(scoring(0.5, Some(3.0))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["stored"], true);
    assert_eq!(body["data"]["scoring"], scoring(0.5, Some(3.0)));

    // Every view reads them now, the registry keeps them, and automatic
    // builds score with them.
    let (_, body) = send(&server.app, Method::GET, &viewer, None).await;
    assert_eq!(body["data"]["stored"], true);
    assert_eq!(body["data"]["scoring"], scoring(0.5, Some(3.0)));
    let registry: Value =
        serde_json::from_slice(&std::fs::read(server.dir.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(registry["scoring"], scoring(0.5, Some(3.0)));
    assert_eq!(scoring_defaults::current().penalty_satellite, 0.5);

    // Saved as the built-in values, they stay stored: a page does not copy
    // a person's older values over them.
    let (_, body) = send(&server.app, Method::PUT, &editor, Some(scoring(1.0, None))).await;
    assert_eq!(body["data"]["stored"], true);
    reset();
}

#[tokio::test]
async fn values_the_scoring_control_cannot_set_are_refused() {
    let _turn = TURN.lock().await;
    reset();
    let server = server();
    let editor = login(&server.app, "editor", "editor-secret").await;
    for refused in [
        scoring(0.25, None),
        scoring(2.5, None),
        scoring(1.0, Some(0.0)),
        scoring(1.0, Some(101.0)),
    ] {
        let (status, body) = send(&server.app, Method::PUT, &editor, Some(refused.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused} gave {body}");
    }
    let (_, body) = send(&server.app, Method::GET, &editor, None).await;
    assert_eq!(body["data"]["stored"], false, "nothing refused was kept");
    reset();
}

#[tokio::test]
async fn a_change_refreshes_every_database_when_automatic_previews_are_on() {
    let _turn = TURN.lock().await;
    reset();
    let server = server();
    let editor = login(&server.app, "editor", "editor-secret").await;
    automatic::configure(AutomationPolicy {
        enabled: true,
        ..AutomationPolicy::default()
    });

    send(&server.app, Method::PUT, &editor, Some(scoring(0.5, None))).await;
    // Every database the server has open, each for all its stacked
    // projects. The test state opens one of its own beside "rig".
    let scheduled = server.state.auto_stacks.scheduled();
    let mut databases = scheduled
        .iter()
        .map(|refresh| refresh.database_id.as_str())
        .collect::<Vec<_>>();
    databases.sort();
    assert_eq!(databases, ["rig", "test"], "{scheduled:?}");
    for refresh in &scheduled {
        assert_eq!(refresh.project_id, None, "every stacked project");
        assert_eq!(
            serde_json::to_value(refresh.reason).unwrap(),
            json!("scoring")
        );
    }

    // Saving the same values again changes nothing to rebuild.
    server.state.auto_stacks.clear();
    send(&server.app, Method::PUT, &editor, Some(scoring(0.5, None))).await;
    assert_eq!(server.state.auto_stacks.pending_count(), 0);

    // With automatic previews off, a change waits for a person's build.
    automatic::configure(AutomationPolicy::default());
    send(&server.app, Method::PUT, &editor, Some(scoring(0.0, None))).await;
    assert_eq!(server.state.auto_stacks.pending_count(), 0);
    reset();
}
