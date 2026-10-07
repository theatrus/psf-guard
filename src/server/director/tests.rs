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
mod adoption;
mod collaboration;
mod configuration;
mod equipment_report;
mod pairing;
mod preferences;
mod site_profile;
mod sky_image;
mod workload;

fn state(dir: &TempDir, enable: bool) -> AppState {
    let mut state = AppState::from_databases(
        vec![],
        dir.path().join("cache"),
        PregenerationConfig::default(),
    )
    .unwrap();
    state.set_allow_database_management(true);
    if enable {
        state.director = Service::configured(Some(&dir.path().join("meta.sqlite"))).unwrap();
    }
    state
}

#[tokio::test]
async fn discovery_is_scoped_read_only_and_does_not_block_metadata() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(state(&dir, true));
    let path = dir.path().join("catalog.sqlite");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE project(Id INTEGER PRIMARY KEY,name TEXT,guid TEXT,profileId TEXT)",
    )
    .unwrap();
    let guid = Uuid::new_v4();
    conn.execute(
        "INSERT INTO project VALUES(1,'M31',?1,'profile-a')",
        [guid.to_string()],
    )
    .unwrap();
    let catalog = super::super::database_context::DatabaseContext::new(
        "rig-catalog".into(),
        "Rig catalog".into(),
        path.to_string_lossy().into(),
        vec![dir.path().to_string_lossy().into()],
        None,
        None,
        None,
        dir.path().join("cache"),
    )
    .unwrap();
    state
        .databases
        .write()
        .unwrap()
        .insert(catalog.id.clone(), Arc::new(catalog));
    let app = router(state.clone());
    let endpoint = "/catalogs/rig-catalog/discovery";
    let (status, first) = call(&app, "GET", endpoint, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["data"]["catalog_slug"], "rig-catalog");
    assert_eq!(
        first["data"]["evidence"]["projects"][0]["source_project_guid"],
        guid.to_string()
    );
    assert!(!first.to_string().contains(path.to_str().unwrap()));
    let (_, again) = call(&app, "GET", endpoint, Value::Null, None).await;
    assert_eq!(
        again["data"]["snapshot_digest"],
        first["data"]["snapshot_digest"]
    );
    conn.execute_batch("UPDATE project SET name='Andromeda'")
        .unwrap();
    let (_, changed) = call(&app, "GET", endpoint, Value::Null, None).await;
    assert_ne!(
        changed["data"]["snapshot_digest"],
        first["data"]["snapshot_digest"]
    );
    let service = state.director.as_ref().unwrap();
    let permit = service
        .discovery_admission
        .clone()
        .acquire_owned()
        .await
        .unwrap();
    assert_eq!(
        call(&app, "GET", endpoint, Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        call(&app, "GET", "/projects", Value::Null, None).await.0,
        StatusCode::OK
    );
    drop(permit);
    assert_eq!(
        call(
            &app,
            "GET",
            "/catalogs/missing/discovery",
            Value::Null,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // Discovery only reads, so it stays open without database management.
    state.set_allow_database_management(false);
    assert_eq!(
        call(&app, "GET", endpoint, Value::Null, None).await.0,
        StatusCode::OK
    );
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
fn metadata_cannot_claim_existing_or_future_registry_files() {
    let dir = TempDir::new().unwrap();
    let registry = dir.path().join("registry.json");
    let paths = [
        registry.clone(),
        AuthRegistry::path_for_database_registry(&registry),
        crate::processing_setups::ProcessingSetupsRegistry::path_for_database_registry(&registry),
        collaboration_auth::credential_path(&registry),
        collaboration_auth::credential_path(&registry).with_extension("lock"),
    ];
    for path in paths {
        assert!(validate_registry_separation(Some(&path), Some(&registry)).is_err());
        let alias = dir.path().join(".").join(path.file_name().unwrap());
        assert!(validate_registry_separation(Some(&alias), Some(&registry)).is_err());
        assert!(!path.exists());
        std::fs::write(&path, b"{}").unwrap();
        assert!(validate_registry_separation(Some(&path), Some(&registry)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
    }
    #[cfg(windows)]
    assert!(
        validate_registry_separation(Some(&dir.path().join("REGISTRY.JSON")), Some(&registry))
            .is_err()
    );
    assert!(
        validate_registry_separation(Some(&dir.path().join("meta.sqlite")), Some(&registry))
            .is_ok()
    );
    assert!(validate_registry_separation(None, Some(&registry)).is_ok());
}

#[test]
fn the_store_defaults_beside_the_registry_only_when_management_is_on() {
    use std::path::{Path, PathBuf};
    assert_eq!(
        default_meta_path(Path::new("/etc/psf-guard/config.json")),
        PathBuf::from("/etc/psf-guard/director-meta.sqlite")
    );
    assert_eq!(
        default_meta_path(Path::new("/tmp/psf-guard-test.json")),
        PathBuf::from("/tmp/psf-guard-test.director-meta.sqlite")
    );
    let registry = Path::new("/tmp/registry.json");
    let explicit = Path::new("/var/lib/meta.sqlite");
    assert_eq!(
        resolve_meta_path(Some(explicit), Some(registry)).as_deref(),
        Some(explicit)
    );
    // Planning runs without database management, so the store is placed
    // beside the registry either way.
    assert_eq!(
        resolve_meta_path(None, Some(registry)),
        Some(default_meta_path(registry))
    );
    assert_eq!(resolve_meta_path(None, None), None);
    for path in [
        default_meta_path(registry),
        default_meta_path(Path::new("/tmp/config.json")),
    ] {
        assert!(validate_registry_separation(Some(&path), Some(registry)).is_ok());
    }
}

#[test]
fn startup_is_explicit_and_never_adopts_a_foreign_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    assert!(Service::configured(None).unwrap().is_none());
    assert!(!path.exists());
    std::fs::write(&path, b"foreign data").unwrap();
    assert!(Service::configured(Some(&path)).is_err());
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
    assert!(status["data"].get("acquisition_available").is_none());
    assert_eq!(status["data"]["instance_id"], Value::Null);
    assert_eq!(status["data"]["database_management"], true);
    assert_eq!(
        call(&app, "GET", "/projects", Value::Null, None).await.0,
        StatusCode::NOT_FOUND
    );
    // Without a store nothing is served, whatever the management flag.
    state.set_allow_database_management(false);
    assert_eq!(
        call(&app, "GET", "/projects", Value::Null, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert!(!dir.path().join("meta.sqlite").exists());

    // With a store, reads stay open without management and writes into rig
    // databases are refused; the status says which server this is.
    let state = Arc::new(self::state(&dir, true));
    let app = router(state.clone());
    state.set_allow_database_management(false);
    let (_, status) = call(&app, "GET", "/status", Value::Null, None).await;
    assert_eq!(status["data"]["enabled"], true);
    assert_eq!(status["data"]["database_management"], false);
    assert_eq!(
        call(&app, "GET", "/projects", Value::Null, None).await.0,
        StatusCode::OK
    );
    let (_, created) = call(
        &app,
        "POST",
        "/projects",
        json!({"id": "11111111-1111-4111-8111-111111111111", "name": "Read-only plan"}),
        None,
    )
    .await;
    assert_eq!(created["data"]["name"], "Read-only plan");
    assert_eq!(
        call(
            &app,
            "POST",
            "/projects/11111111-1111-4111-8111-111111111111/activation/apply",
            json!({"preview_digest": "0".repeat(64)}),
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/projects/11111111-1111-4111-8111-111111111111/activation/push",
            json!({}),
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
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
    assert!(status["data"].get("acquisition_available").is_none());
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
            call(
                &app,
                "GET",
                "/catalogs/missing/discovery",
                Value::Null,
                token
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
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
        call(
            &app,
            "GET",
            "/catalogs/missing/discovery",
            Value::Null,
            Some(&reader)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
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
    // The stuck write holds the writer: another write waits its turn and
    // then answers busy, while a read is served from the pool at once.
    let app = router(state.clone());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/director/v1/projects")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "id": Uuid::new_v4(), "name": "M33" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[RETRY_AFTER], "1");
    // A workspace opens with a burst of reads; every one of them is served
    // while the writer is held, none waits and none answers busy.
    let started_at = tokio::time::Instant::now();
    let reads: Vec<_> = (0..8)
        .map(|_| {
            tokio::spawn(
                app.clone().oneshot(
                    Request::builder()
                        .uri("/api/director/v1/projects")
                        .body(Body::empty())
                        .unwrap(),
                ),
            )
        })
        .collect();
    for read in reads {
        assert_eq!(read.await.unwrap().unwrap().status(), StatusCode::OK);
    }
    assert!(
        started_at.elapsed() < ADMISSION_WAIT,
        "reads must not wait for the writer"
    );
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

#[tokio::test]
async fn the_template_library_is_shared_over_http_with_revisions() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(state(&dir, true));
    let app = router(state);
    let id = Uuid::new_v4();
    let template = |revision: u64, exposure: f64| json!({ "id": id, "revision": revision, "name": "Ha 300", "filter_name": "Ha", "gain": 100, "offset": 30, "bin": 1, "readout_mode": null, "default_exposure_seconds": exposure, "updated_at_ms": 0 });
    let (status, empty) = call(&app, "GET", "/templates", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["data"], json!([]));
    let (status, saved) = call(
        &app,
        "PUT",
        &format!("/templates/{id}"),
        template(0, 300.0),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["data"]["revision"], 1);
    assert_eq!(saved["data"]["bandpass"]["id"], "h_alpha");
    // The path and the body must agree; a stale revision conflicts.
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/templates/{}", Uuid::new_v4()),
            template(1, 300.0),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/templates/{id}"),
            template(0, 600.0),
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (status, changed) = call(
        &app,
        "PUT",
        &format!("/templates/{id}"),
        template(1, 600.0),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["data"]["revision"], 2);
    let (_, listed) = call(&app, "GET", "/templates", Value::Null, None).await;
    assert_eq!(listed["data"][0]["default_exposure_seconds"], 600.0);
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/templates/{id}?revision=1"),
            Value::Null,
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/templates/{id}?revision=2"),
            Value::Null,
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/templates/{id}?revision=2"),
            Value::Null,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (_, listed) = call(&app, "GET", "/templates", Value::Null, None).await;
    assert_eq!(listed["data"], json!([]));
}
