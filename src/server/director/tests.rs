use super::*;
use crate::{
    auth_registry::{AccessRole, AuthRegistry, AuthTokenRecord, AuthUserRecord},
    cli::PregenerationConfig,
    server::auth,
};
use axum::{
    body::{to_bytes, Body},
    http::Request,
    middleware,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

fn state(dir: &TempDir, enable: bool) -> AppState {
    let mut state = AppState::from_databases(
        vec![],
        dir.path().join("cache").to_string_lossy().into(),
        PregenerationConfig::default(),
    )
    .unwrap();
    state.set_allow_database_management(true);
    if enable {
        state.director = Service::configured(Some(&dir.path().join("meta.sqlite")), true).unwrap();
    }
    state
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .nest("/api/director/v1", routes())
        .layer(middleware::from_fn(super::super::json_no_store))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::authorize_api,
        ))
        .with_state(state)
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    body: Value,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("/api/director/v1{path}"))
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 100_000).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({"text":String::from_utf8_lossy(&bytes)})),
    )
}

#[test]
fn startup_is_explicit_and_never_adopts_a_foreign_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    assert!(Service::configured(None, true).unwrap().is_none());
    assert!(Service::configured(Some(&path), false).is_err());
    assert!(!path.exists());
    std::fs::write(&path, b"foreign data").unwrap();
    assert!(Service::configured(Some(&path), true).is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"foreign data");
}

#[test]
fn cli_requires_explicit_management_and_preserves_the_meta_path() {
    use clap::Parser;
    assert!(crate::cli::Cli::try_parse_from([
        "psf-guard",
        "server",
        "--director-meta",
        "meta.sqlite"
    ])
    .is_err());
    let parsed = crate::cli::Cli::try_parse_from([
        "psf-guard",
        "server",
        "--allow-database-management",
        "--director-meta",
        "meta.sqlite",
    ])
    .unwrap();
    let crate::cli::Commands::Server { director_meta, .. } = parsed.command else {
        panic!("not a server command")
    };
    assert_eq!(
        director_meta.unwrap(),
        std::path::PathBuf::from("meta.sqlite")
    );
}

#[tokio::test]
async fn projects_survive_restart_and_renames_require_expected_revision() {
    let dir = TempDir::new().unwrap();
    let app_state = Arc::new(state(&dir, true));
    let original_instance = app_state.director.as_ref().unwrap().instance_id;
    let app = router(app_state.clone());
    let id = Uuid::new_v4();
    let create = json!({"id":id,"name":"M31"});
    assert_eq!(
        call(&app, "POST", "/projects", create.clone(), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "POST", "/projects", create, None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/projects",
            json!({"id":id,"name":"Changed"}),
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let path = format!("/projects/{id}");
    let rename = json!({"expected_revision":1,"name":"Andromeda"});
    let (_, changed) = call(&app, "PATCH", &path, rename.clone(), None).await;
    assert_eq!(changed["data"]["revision"], 2);
    assert_eq!(
        call(&app, "PATCH", &path, rename, None).await.0,
        StatusCode::CONFLICT
    );
    let (_, page) = call(&app, "GET", "/projects?limit=1", Value::Null, None).await;
    assert_eq!(page["data"]["items"][0]["id"], id.to_string());
    drop(app);
    drop(app_state);
    let reopened = Arc::new(state(&dir, true));
    assert_eq!(
        reopened.director.as_ref().unwrap().instance_id,
        original_instance
    );
    let (_, record) = call(&router(reopened), "GET", &path, Value::Null, None).await;
    assert_eq!(record["data"]["name"], "Andromeda");
}

#[tokio::test]
async fn disabled_and_management_gates_apply_without_exposing_paths() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(state(&dir, false));
    let app = router(state.clone());
    let (_, status) = call(&app, "GET", "/status", Value::Null, None).await;
    assert_eq!(status["data"]["enabled"], false);
    assert_eq!(status["data"]["acquisition_available"], false);
    assert_eq!(status["data"]["instance_id"], Value::Null);
    assert_eq!(
        call(&app, "GET", "/projects", Value::Null, None).await.0,
        StatusCode::NOT_FOUND
    );
    state.set_allow_database_management(false);
    assert_eq!(
        call(&app, "GET", "/projects", Value::Null, None).await.0,
        StatusCode::FORBIDDEN
    );
    assert!(!dir.path().join("meta.sqlite").exists());
}

#[tokio::test]
async fn requests_are_strict_bounded_and_do_not_claim_acquisition_authority() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(state(&dir, true));
    let app = router(state.clone());
    for path in [
        "/projects?limit=0",
        "/projects?limit=257",
        "/projects?after=bad",
        "/projects?unknown=true",
    ] {
        assert_eq!(
            call(&app, "GET", path, Value::Null, None).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let id = Uuid::new_v4();
    assert_eq!(
        call(
            &app,
            "POST",
            "/projects",
            json!({"id":id,"name":"M31","acquire":true}),
            None
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/projects",
            json!({"id":id,"name":"x".repeat(5000)}),
            None
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/director/v1/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = to_bytes(response.into_body(), 10_000).await.unwrap();
    let status: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status["data"]["enabled"], true);
    assert_eq!(status["data"]["acquisition_available"], false);
    assert!(!String::from_utf8_lossy(&bytes).contains(&dir.path().to_string_lossy().to_string()));
}

#[tokio::test]
async fn normal_api_auth_rejects_sync_keys_and_read_only_mutations() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(state(&dir, true));
    let mut registry = AuthRegistry::default();
    registry
        .add(
            AuthUserRecord::new("editor", AccessRole::ReadWrite, "test-password-not-real").unwrap(),
            false,
        )
        .unwrap();
    let (reader, reader_record) = AuthTokenRecord::mint("editor", "reader", true, None).unwrap();
    let (writer, writer_record) = AuthTokenRecord::mint("editor", "writer", false, None).unwrap();
    registry.tokens = vec![reader_record, writer_record];
    state.set_server_auth(auth::ServerAuth::from_sources(None, &registry, 3000).unwrap());
    let app = router(state.clone());
    let create = json!({"id":Uuid::new_v4(),"name":"M31"});
    for token in [None, Some("not-a-user-api-token")] {
        assert_eq!(
            call(&app, "GET", "/status", Value::Null, token).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&app, "POST", "/projects", create.clone(), token)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        call(&app, "POST", "/projects", create.clone(), Some(&reader))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "GET", "/projects", Value::Null, Some(&reader))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "POST", "/projects", create, Some(&writer))
            .await
            .0,
        StatusCode::OK
    );
    state.set_server_auth(None);
    state.set_anonymous_access_trusted(false);
    assert_eq!(
        call(&app, "GET", "/status", Value::Null, None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn canceled_requests_do_not_release_admission_before_the_write_finishes() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(state(&dir, true));
    let service = state.director.clone().unwrap();
    let (started, start) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let id = Uuid::new_v4();
    let task = tokio::spawn(service.clone().run(move |store| {
        started.send(()).unwrap();
        wait.recv().unwrap();
        store.create_project(id, "M31")
    }));
    start.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let response = router(state)
        .oneshot(
            Request::builder()
                .uri("/api/director/v1/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[RETRY_AFTER], "1");
    release.send(()).unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match service.clone().run(move |store| store.project(id)).await {
            Ok(record) => {
                assert_eq!(record.unwrap().name, "M31");
                break;
            }
            Err(Error::Busy) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await
            }
            other => panic!("unexpected metadata result: {other:?}"),
        }
    }
}

#[tokio::test]
async fn sqlite_contention_is_retryable_and_storage_diagnostics_stay_private() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(state(&dir, true));
    let app = router(state);
    let external = rusqlite::Connection::open(dir.path().join("meta.sqlite")).unwrap();
    external.execute_batch("BEGIN IMMEDIATE").unwrap();
    let id = Uuid::new_v4();
    let create = json!({"id":id,"name":"M31"});
    assert_eq!(
        call(&app, "POST", "/projects", create.clone(), None)
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    external.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        call(&app, "POST", "/projects", create, None).await.0,
        StatusCode::OK
    );
    external.execute_batch("CREATE TRIGGER fail_insert BEFORE INSERT ON global_project BEGIN SELECT RAISE(ABORT,'private-root-path-marker'); END;").unwrap();
    let (status, error) = call(
        &app,
        "POST",
        "/projects",
        json!({"id":Uuid::new_v4(),"name":"M42"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!error.to_string().contains("private-root-path-marker"));
    assert!(error["error"].as_str().unwrap().contains("see server logs"));
}
