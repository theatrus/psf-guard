//! End-to-end test that exercises the runtime CRUD endpoints on
//! `/api/databases` from B4. Spins up an in-process axum app with the same
//! route shape the production server uses, points it at a temp directory for
//! the registry, then walks add → use → remove.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::{get, put};
use axum::Router;
use http_body_util::BodyExt;
use psf_guard::server::handlers;
use psf_guard::server::state::AppState;
use serde_json::Value;
use tempfile::tempdir;
use tower::ServiceExt;

fn build_app(state: Arc<AppState>) -> Router {
    let db_routes: Router<Arc<AppState>> = Router::new()
        .route("/projects", get(handlers::list_projects))
        .route("/projects/overview", get(handlers::get_projects_overview));

    Router::new()
        .route("/api/info", get(handlers::get_server_info))
        .route(
            "/api/databases",
            get(handlers::list_databases).post(handlers::add_database_route),
        )
        .route(
            "/api/databases/{db_id}",
            put(handlers::update_database_route).delete(handlers::remove_database_route),
        )
        .nest("/api/db/{db_id}", db_routes)
        .with_state(state)
}

fn create_sqlite(path: &std::path::Path) {
    use rusqlite::Connection;
    let conn = Connection::open(path).unwrap();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS project (Id INTEGER PRIMARY KEY, name TEXT, profileId TEXT)",
        [],
    )
    .unwrap();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS target (Id INTEGER PRIMARY KEY, name TEXT, projectId INTEGER, active INTEGER, ra REAL, dec REAL)",
        [],
    )
    .unwrap();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS acquiredimage (Id INTEGER PRIMARY KEY, projectId INTEGER, targetId INTEGER, gradingStatus INTEGER, metadata TEXT, acquireddate INTEGER, filtername TEXT)",
        [],
    )
    .unwrap();
}

async fn json_request(
    app: Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let req = builder
        .body(match body {
            Some(v) => Body::from(serde_json::to_vec(&v).unwrap()),
            None => Body::empty(),
        })
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

#[tokio::test]
async fn server_info_exposes_the_configured_site_banner() {
    let dir = tempdir().unwrap();
    let state = Arc::new(
        AppState::from_databases(
            vec![],
            dir.path().join("cache").to_string_lossy().into_owned(),
            psf_guard::cli::PregenerationConfig::default(),
        )
        .unwrap(),
    );
    state.set_site_banner(Some(psf_guard::config::SiteBannerConfig {
        title: "Demo site".into(),
        message: "Sample data; changes may be reset.".into(),
        link_text: Some("Learn more".into()),
        link_url: Some("https://psf-guard.com/".into()),
    }));

    let (status, body) = json_request(build_app(state), "GET", "/api/info", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["banner"]["title"], "Demo site");
    assert_eq!(
        body["data"]["banner"]["message"],
        "Sample data; changes may be reset."
    );
    assert_eq!(body["data"]["banner"]["link_url"], "https://psf-guard.com/");
}

#[tokio::test]
async fn crud_lifecycle_adds_uses_and_removes_a_database() {
    let dir = tempdir().unwrap();
    let registry_path = dir.path().join("config.json");
    let cache_dir = dir.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let db_path = dir.path().join("scratch.sqlite");
    create_sqlite(&db_path);
    let image_dir = dir.path().join("imgs");
    std::fs::create_dir_all(&image_dir).unwrap();

    // Empty state to start; registry will be created on the first POST.
    let state = Arc::new(
        AppState::from_databases(
            vec![],
            cache_dir.to_string_lossy().into_owned(),
            psf_guard::cli::PregenerationConfig::default(),
        )
        .unwrap(),
    );
    state.set_registry_path(Some(registry_path.clone()));
    state.set_allow_database_management(true);

    // 1) Empty listing.
    let (status, body) =
        json_request(build_app(state.clone()), "GET", "/api/databases", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 0);

    // 2) Add a database via POST.
    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases",
        Some(serde_json::json!({
            "name": "Scratch Rig",
            "db_path": db_path.to_string_lossy(),
            "image_dirs": [image_dir.to_string_lossy()],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let added = &body["data"];
    let slug = added["id"].as_str().unwrap().to_string();
    assert_eq!(added["name"], "Scratch Rig");
    assert_eq!(added["image_directories"].as_array().unwrap().len(), 1);

    // 3) Listing now returns the new entry.
    let (_, body) = json_request(build_app(state.clone()), "GET", "/api/databases", None).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["id"], slug);

    // 4) Hit a per-DB endpoint for the new DB — should be reachable.
    let (status, _) = json_request(
        build_app(state.clone()),
        "GET",
        &format!("/api/db/{}/projects", slug),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 5) Wrong-slug per-DB request still returns 404.
    let (status, _) = json_request(
        build_app(state.clone()),
        "GET",
        "/api/db/no-such-slug/projects",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 6) Rename via PUT.
    let (status, body) = json_request(
        build_app(state.clone()),
        "PUT",
        &format!("/api/databases/{}", slug),
        Some(serde_json::json!({
            "name": "Renamed Rig",
            "slug": "renamed-rig",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["id"], "renamed-rig");
    assert_eq!(body["data"]["name"], "Renamed Rig");

    // Old slug no longer resolves.
    let (status, _) = json_request(
        build_app(state.clone()),
        "GET",
        &format!("/api/db/{}/projects", slug),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 7) Registry on disk reflects the rename.
    let on_disk = std::fs::read_to_string(&registry_path).unwrap();
    let parsed: Value = serde_json::from_str(&on_disk).unwrap();
    let dbs = parsed["databases"].as_array().unwrap();
    assert_eq!(dbs.len(), 1);
    assert_eq!(dbs[0]["id"], "renamed-rig");

    // 8) Remove it.
    let (status, body) = json_request(
        build_app(state.clone()),
        "DELETE",
        "/api/databases/renamed-rig",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["removed"], true);

    // 9) Listing is empty again.
    let (_, body) = json_request(build_app(state.clone()), "GET", "/api/databases", None).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn add_database_with_invalid_slug_is_rejected() {
    let dir = tempdir().unwrap();
    let registry_path = dir.path().join("config.json");
    let cache_dir = dir.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let db_path = dir.path().join("scratch.sqlite");
    create_sqlite(&db_path);
    let image_dir = dir.path().join("imgs");
    std::fs::create_dir_all(&image_dir).unwrap();

    let state = Arc::new(
        AppState::from_databases(
            vec![],
            cache_dir.to_string_lossy().into_owned(),
            psf_guard::cli::PregenerationConfig::default(),
        )
        .unwrap(),
    );
    state.set_registry_path(Some(registry_path));
    state.set_allow_database_management(true);

    let (status, _) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases",
        Some(serde_json::json!({
            "name": "Bad Slug",
            "db_path": db_path.to_string_lossy(),
            "image_dirs": [image_dir.to_string_lossy()],
            "slug": "Has Uppercase",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Nothing got registered.
    let (_, body) = json_request(build_app(state.clone()), "GET", "/api/databases", None).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn cache_dir_is_per_slug_and_follows_rename() {
    let dir = tempdir().unwrap();
    let registry_path = dir.path().join("config.json");
    let cache_root = dir.path().join("cache");
    std::fs::create_dir_all(&cache_root).unwrap();
    let db_path = dir.path().join("scratch.sqlite");
    create_sqlite(&db_path);
    let image_dir = dir.path().join("imgs");
    std::fs::create_dir_all(&image_dir).unwrap();

    let state = Arc::new(
        AppState::from_databases(
            vec![],
            cache_root.to_string_lossy().into_owned(),
            psf_guard::cli::PregenerationConfig::default(),
        )
        .unwrap(),
    );
    state.set_registry_path(Some(registry_path));
    state.set_allow_database_management(true);

    let (_, body) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases",
        Some(serde_json::json!({
            "name": "First",
            "db_path": db_path.to_string_lossy(),
            "image_dirs": [image_dir.to_string_lossy()],
            "slug": "first-rig",
        })),
    )
    .await;
    assert_eq!(body["data"]["id"], "first-rig");

    // The per-DB cache subdir should exist.
    assert!(cache_root.join("first-rig").is_dir());

    // Drop a sentinel file into the cache dir; after rename, it should still
    // be there under the new slug (preview cache survives slug rename).
    let sentinel = cache_root.join("first-rig").join("sentinel");
    std::fs::write(&sentinel, b"hello").unwrap();

    let (status, _) = json_request(
        build_app(state.clone()),
        "PUT",
        "/api/databases/first-rig",
        Some(serde_json::json!({ "slug": "renamed-rig" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    assert!(
        !cache_root.join("first-rig").exists(),
        "old cache dir should have been moved"
    );
    let new_sentinel = cache_root.join("renamed-rig").join("sentinel");
    assert!(
        new_sentinel.exists(),
        "sentinel should have been carried to renamed-rig cache dir"
    );
    assert_eq!(std::fs::read(new_sentinel).unwrap(), b"hello");
}

#[tokio::test]
async fn crud_returns_403_when_management_flag_is_off() {
    // Registry IS configured (server could persist), but the operator did
    // not opt into `--allow-database-management`. CRUD must be forbidden so
    // a reachable client cannot rewrite the user's DB list.
    let dir = tempdir().unwrap();
    let registry_path = dir.path().join("config.json");
    let cache_dir = dir.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let db_path = dir.path().join("scratch.sqlite");
    create_sqlite(&db_path);
    let image_dir = dir.path().join("imgs");
    std::fs::create_dir_all(&image_dir).unwrap();

    let state = Arc::new(
        AppState::from_databases(
            vec![],
            cache_dir.to_string_lossy().into_owned(),
            psf_guard::cli::PregenerationConfig::default(),
        )
        .unwrap(),
    );
    state.set_registry_path(Some(registry_path));
    // Note: set_allow_database_management NOT called → defaults to false.

    let (status, body) = json_request(
        build_app(state.clone()),
        "POST",
        "/api/databases",
        Some(serde_json::json!({
            "name": "Try",
            "db_path": db_path.to_string_lossy(),
            "image_dirs": [image_dir.to_string_lossy()],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("allow-database-management"));

    // PUT and DELETE are also gated.
    let (status, _) = json_request(
        build_app(state.clone()),
        "PUT",
        "/api/databases/anything",
        Some(serde_json::json!({"name": "x"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = json_request(
        build_app(state.clone()),
        "DELETE",
        "/api/databases/anything",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Read-only listing still works.
    let (status, _) = json_request(build_app(state), "GET", "/api/databases", None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn crud_is_disabled_when_no_registry_path_configured() {
    let dir = tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let db_path = dir.path().join("scratch.sqlite");
    create_sqlite(&db_path);
    let image_dir = dir.path().join("imgs");
    std::fs::create_dir_all(&image_dir).unwrap();

    let state = Arc::new(
        AppState::from_databases(
            vec![],
            cache_dir.to_string_lossy().into_owned(),
            psf_guard::cli::PregenerationConfig::default(),
        )
        .unwrap(),
    );
    // Note: state.set_registry_path is NOT called. Also management flag is off.
    // Both gates need to clear; the order doesn't matter — either failing path
    // is acceptable.
    state.set_allow_database_management(true);

    let (status, _) = json_request(
        build_app(state),
        "POST",
        "/api/databases",
        Some(serde_json::json!({
            "name": "Try",
            "db_path": db_path.to_string_lossy(),
            "image_dirs": [image_dir.to_string_lossy()],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
