//! Personal API tokens and the MCP endpoint they unlock.

use axum::{
    body::Body,
    http::{header::COOKIE, HeaderMap, Method, Request, StatusCode},
    middleware,
    routing::{delete, get, post},
    Router,
};
use http_body_util::BodyExt;
use psf_guard::{
    auth_registry::{AccessRole, AuthRegistry, AuthUserRecord},
    config::ServerAuthConfig,
    server::{
        auth::{self, ServerAuth},
        mcp,
        state::AppState,
        user_admin,
    },
};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

fn registry() -> AuthRegistry {
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

/// The API with sessions, tokens, a plain read/write route, and MCP, wired
/// the way the server wires them.
fn app(directory: &tempfile::TempDir) -> (Router, Arc<AppState>) {
    let database_registry_path = directory.path().join("config.json");
    let auth_registry_path = AuthRegistry::path_for_database_registry(&database_registry_path);
    let registry = registry();
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

    let api = Router::new()
        .route("/auth/login", post(auth::login))
        .route(
            "/auth/tokens",
            get(user_admin::list_tokens).post(user_admin::create_token),
        )
        .route("/auth/tokens/{id}", delete(user_admin::revoke_token))
        .route(
            "/catalog",
            get(|| async { "catalog" }).put(|| async { "changed" }),
        )
        .nest_service("/mcp", mcp::service(Arc::clone(&state)))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::authorize_api,
        ))
        .with_state(Arc::clone(&state));
    (Router::new().nest("/api", api), state)
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let value = if bytes.is_empty() {
        Value::Null
    } else if headers
        .get("content-type")
        .is_some_and(|value| value.as_bytes().starts_with(b"text/event-stream"))
    {
        // Take the last JSON `data:` line of the stream.
        text.lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .filter_map(|data| serde_json::from_str(data.trim()).ok())
            .next_back()
            .unwrap_or(Value::Null)
    } else {
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    };
    (status, headers, value)
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
    let (status, headers, _) = send(app, request).await;
    assert_eq!(status, StatusCode::OK);
    headers["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

async fn mint(app: &Router, cookie: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/tokens")
        .header(COOKIE, cookie)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, _, value) = send(app, request).await;
    (status, value)
}

fn with_bearer(method: Method, uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn tokens_carry_their_users_role_and_can_be_narrowed_and_revoked() {
    let directory = tempfile::tempdir().unwrap();
    let (app, _) = app(&directory);
    let editor = login(&app, "editor", "editor-secret").await;

    let (status, minted) = mint(&app, &editor, json!({ "label": "full" })).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    let full = minted["data"]["token"].as_str().unwrap().to_string();
    assert!(full.starts_with("psfg_"));
    assert_eq!(minted["data"]["summary"]["role"], "read_write");

    let (_, minted) = mint(
        &app,
        &editor,
        json!({ "label": "narrow", "read_only": true, "expires_in_days": 7 }),
    )
    .await;
    let narrow = minted["data"]["token"].as_str().unwrap().to_string();
    let narrow_id = minted["data"]["summary"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(minted["data"]["summary"]["role"], "read_only");
    assert!(minted["data"]["summary"]["expires_at"].is_i64());
    assert_eq!(minted["data"]["tokens"].as_array().unwrap().len(), 2);

    let (status, _, _) = send(&app, with_bearer(Method::PUT, "/api/catalog", &full)).await;
    assert_eq!(status, StatusCode::OK, "an editor's token writes");
    let (status, _, _) = send(&app, with_bearer(Method::GET, "/api/catalog", &narrow)).await;
    assert_eq!(status, StatusCode::OK, "a read-only token reads");
    let (status, _, body) = send(&app, with_bearer(Method::PUT, "/api/catalog", &narrow)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, _, body) = send(
        &app,
        with_bearer(Method::GET, "/api/catalog", "psfg_not_a_real_token"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    // A token is judged before any cookie the same request carries.
    let mixed = Request::builder()
        .uri("/api/catalog")
        .header(COOKIE, &editor)
        .header("authorization", "Bearer psfg_stale")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, mixed).await.0, StatusCode::UNAUTHORIZED);

    // Tokens cannot mint tokens.
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/tokens")
        .header("authorization", format!("Bearer {full}"))
        .header("content-type", "application/json")
        .body(Body::from(json!({ "label": "chain" }).to_string()))
        .unwrap();
    let (status, _, body) = send(&app, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // The secret is not on disk; the hash is.
    let saved = std::fs::read_to_string(directory.path().join("auth.json")).unwrap();
    assert!(!saved.contains(&full));
    assert!(saved.contains("\"token_hash\""));

    let request = Request::builder()
        .method(Method::DELETE)
        .uri(format!("/api/auth/tokens/{narrow_id}"))
        .header(COOKIE, &editor)
        .body(Body::empty())
        .unwrap();
    let (status, _, remaining) = send(&app, request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(remaining["data"].as_array().unwrap().len(), 1);
    let (status, _, _) = send(&app, with_bearer(Method::GET, "/api/catalog", &narrow)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "revoked at once");
}

#[tokio::test]
async fn a_viewer_manages_only_their_own_tokens_and_never_gains_write() {
    let directory = tempfile::tempdir().unwrap();
    let (app, _) = app(&directory);
    let viewer = login(&app, "viewer", "viewer-secret").await;
    let editor = login(&app, "editor", "editor-secret").await;

    let (status, minted) = mint(&app, &viewer, json!({ "label": "mine" })).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    assert_eq!(minted["data"]["summary"]["role"], "read_only");
    let mine = minted["data"]["token"].as_str().unwrap().to_string();
    let (status, _, _) = send(&app, with_bearer(Method::PUT, "/api/catalog", &mine)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = mint(
        &app,
        &viewer,
        json!({ "label": "for editor", "username": "editor" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (_, minted) = mint(&app, &editor, json!({ "label": "editors" })).await;
    let editors_id = minted["data"]["summary"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let request = Request::builder()
        .uri("/api/auth/tokens")
        .header(COOKIE, &viewer)
        .body(Body::empty())
        .unwrap();
    let (_, _, listed) = send(&app, request).await;
    let listed = listed["data"].as_array().unwrap().clone();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["username"], "viewer");

    let request = Request::builder()
        .method(Method::DELETE)
        .uri(format!("/api/auth/tokens/{editors_id}"))
        .header(COOKIE, &viewer)
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(&app, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let request = Request::builder()
        .uri("/api/auth/tokens")
        .header(COOKIE, &editor)
        .body(Body::empty())
        .unwrap();
    let (_, _, listed) = send(&app, request).await;
    assert_eq!(
        listed["data"].as_array().unwrap().len(),
        2,
        "an editor sees all"
    );
}

struct McpClient<'a> {
    app: &'a Router,
    token: String,
    session: Option<String>,
    next_id: i64,
}

impl McpClient<'_> {
    async fn call(&mut self, method: &str, params: Value) -> (StatusCode, Value) {
        let id = self.next_id;
        self.next_id += 1;
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/api/mcp")
            .header("host", "localhost")
            .header("authorization", format!("Bearer {}", self.token))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(session) = &self.session {
            builder = builder.header("mcp-session-id", session);
        }
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let request = builder.body(Body::from(body.to_string())).unwrap();
        let (status, headers, value) = send(self.app, request).await;
        if let Some(session) = headers.get("mcp-session-id") {
            self.session = Some(session.to_str().unwrap().to_string());
        }
        (status, value)
    }

    async fn notify(&mut self, method: &str) {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/api/mcp")
            .header("host", "localhost")
            .header("authorization", format!("Bearer {}", self.token))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(session) = &self.session {
            builder = builder.header("mcp-session-id", session);
        }
        let body = json!({ "jsonrpc": "2.0", "method": method });
        let request = builder.body(Body::from(body.to_string())).unwrap();
        let (status, _, _) = send(self.app, request).await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }

    async fn initialize(&mut self) -> Value {
        let (status, value) = self
            .call(
                "initialize",
                json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "0" }
                }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{value}");
        self.notify("notifications/initialized").await;
        value
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let (status, value) = self
            .call(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{value}");
        value["result"].clone()
    }
}

fn tool_text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn mcp_endpoint_serves_tools_behind_the_token_and_checks_write_per_tool() {
    let directory = tempfile::tempdir().unwrap();
    let (app, state) = app(&directory);
    let db_id = state.all_databases()[0].id.clone();

    let anonymous = Request::builder()
        .method(Method::POST)
        .uri("/api/mcp")
            .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } } }).to_string()))
        .unwrap();
    assert_eq!(send(&app, anonymous).await.0, StatusCode::UNAUTHORIZED);

    let editor = login(&app, "editor", "editor-secret").await;
    let (_, minted) = mint(
        &app,
        &editor,
        json!({ "label": "agent", "read_only": true }),
    )
    .await;
    let read_only = minted["data"]["token"].as_str().unwrap().to_string();
    let (_, minted) = mint(&app, &editor, json!({ "label": "agent rw" })).await;
    let read_write = minted["data"]["token"].as_str().unwrap().to_string();

    let mut client = McpClient {
        app: &app,
        token: read_only,
        session: None,
        next_id: 1,
    };
    let initialized = client.initialize().await;
    assert_eq!(initialized["result"]["serverInfo"]["name"], "psf-guard");
    assert!(initialized["result"]["instructions"]
        .as_str()
        .unwrap()
        .contains("list_databases"));

    let (status, listed) = client.call("tools/list", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let names = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    for expected in [
        "list_databases",
        "list_images",
        "analyze_sequence",
        "get_jobs",
        "grade_images",
        "start_import",
        "start_wbpp_run",
    ] {
        assert!(names.contains(&expected.to_string()), "{names:?}");
    }
    let import_tool = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "start_import")
        .unwrap();
    assert_eq!(
        import_tool["inputSchema"]["properties"]["database"]["type"],
        "string"
    );

    let databases = client.tool("list_databases", json!({})).await;
    assert_ne!(databases["isError"], true, "{databases}");
    assert!(tool_text(&databases).contains(&db_id));

    let jobs = client.tool("get_jobs", json!({ "database": db_id })).await;
    assert_ne!(jobs["isError"], true, "{jobs}");
    assert!(tool_text(&jobs).contains("quality_backfill"));

    let missing = client.tool("get_jobs", json!({ "database": "nope" })).await;
    assert_eq!(missing["isError"], true);
    assert!(tool_text(&missing).contains(&db_id), "names the known ids");

    let denied = client
        .tool(
            "grade_images",
            json!({ "database": db_id, "updates": [{ "image_id": 1, "status": "rejected" }] }),
        )
        .await;
    assert_eq!(denied["isError"], true, "{denied}");
    assert!(tool_text(&denied).contains("read-only"), "{denied}");

    let mut writer = McpClient {
        app: &app,
        token: read_write,
        session: None,
        next_id: 1,
    };
    writer.initialize().await;
    let attempted = writer
        .tool(
            "grade_images",
            json!({ "database": db_id, "updates": [{ "image_id": 1, "status": "rejected" }] }),
        )
        .await;
    assert!(
        !tool_text(&attempted).contains("read-only"),
        "the write path is reached: {attempted}"
    );
}
