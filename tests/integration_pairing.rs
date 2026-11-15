//! One-token pairing: the operator mints a single-use code, the client
//! exchanges it for its own durable credential, and the operator revokes
//! clients one by one.

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::{get, post},
    Router,
};
use http_body_util::BodyExt;
use psf_guard::cli::PregenerationConfig;
use psf_guard::db_registry::RemoteClientKind;
use psf_guard::db_registry::{DbEntry, DbRegistry};
use psf_guard::server::{pairing, peers, remote_sync, state::AppState};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::tempdir;
use tower::ServiceExt;

fn schema(connection: &Connection) {
    connection
        .execute_batch(
            "PRAGMA user_version = 23;
             CREATE TABLE project (Id INTEGER PRIMARY KEY, guid TEXT);
             CREATE TABLE target (Id INTEGER PRIMARY KEY, guid TEXT);
             CREATE TABLE exposuretemplate (Id INTEGER PRIMARY KEY, guid TEXT);
             CREATE TABLE exposureplan (Id INTEGER PRIMARY KEY, guid TEXT);
             CREATE TABLE ruleweight (Id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE acquiredimage (Id INTEGER PRIMARY KEY, gradingStatus INTEGER, guid TEXT);
             CREATE TABLE imagedata (Id INTEGER PRIMARY KEY, imagedata BLOB, acquiredimageid INTEGER);",
        )
        .unwrap();
}

async fn call(
    router: Router,
    method: &str,
    uri: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = bearer {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = match body {
        Some(body) => request
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => request.body(Body::empty()).unwrap(),
    };
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

fn harness() -> (Arc<AppState>, Router, PathBuf, tempfile::TempDir) {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("catalog.sqlite");
    schema(&Connection::open(&db_path).unwrap());
    let image_dir = directory.path().join("images");
    std::fs::create_dir(&image_dir).unwrap();
    let entry = DbEntry {
        ts_sync: None,
        id: "review".into(),
        name: "Review copy".into(),
        db_path: db_path.to_string_lossy().into_owned(),
        image_dirs: vec![image_dir.to_string_lossy().into_owned()],
        reject_archive: None,
        remote_image_upload: None,
        export_dir: None,
        process_dir: None,
        autoimport: None,
        scan_rules: None,
        analyze_new_frames: false,
        pair_calibrated_copies: true,
        scan_calibrated_copies: false,
        processing_overrides: None,
    };
    let registry_path = directory.path().join("registry.json");
    let mut registry = DbRegistry::default();
    registry.databases.push(entry.clone());
    registry.save(&registry_path).unwrap();

    let state = Arc::new(
        AppState::from_databases(
            vec![entry],
            directory
                .path()
                .join("cache")
                .to_string_lossy()
                .into_owned(),
            PregenerationConfig::default(),
        )
        .unwrap(),
    );
    state.set_registry_path(Some(registry_path.clone()));
    state.set_allow_database_management(true);

    let router = Router::new()
        .route(
            "/api/databases/{db_id}/pairing-token",
            post(pairing::issue_pairing_token_route),
        )
        .route("/api/sync/v1/pair", post(pairing::pair_route))
        .route(
            "/api/databases/{db_id}/clients/{client_uuid}",
            axum::routing::delete(pairing::revoke_client_route),
        )
        .route("/api/sync/v1/capabilities", get(remote_sync::capabilities))
        .route("/api/peers", get(peers::list_peers).post(peers::add_peer))
        .with_state(Arc::clone(&state));
    (state, router, registry_path, directory)
}

#[tokio::test]
async fn pairing_exchanges_one_code_for_a_working_token() {
    let (_state, router, registry_path, _directory) = harness();

    let (status, issued) = call(
        router.clone(),
        "POST",
        "/api/databases/review/pairing-token",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{issued:#}");
    let code = issued["data"]["pairing_token"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(code.starts_with("psfpt_"), "{code}");

    let (status, paired) = call(
        router.clone(),
        "POST",
        "/api/sync/v1/pair",
        None,
        Some(json!({
            "protocol_version": 1,
            "pairing_token": code,
            "client_name": "OBSERVATORY-PC"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{paired:#}");
    assert_eq!(paired["data"]["catalog_id"], "review");
    assert_eq!(paired["data"]["catalog_name"], "Review copy");
    assert!(paired["data"]["client_uuid"].as_str().is_some());
    let token = paired["data"]["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("psfrc_"), "{token}");

    // The exchanged token authenticates, and pairing opted the catalog into
    // scheduler sync.
    let (status, capabilities) = call(
        router.clone(),
        "GET",
        "/api/sync/v1/capabilities",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{capabilities:#}");
    let catalogs = capabilities["data"]["catalogs"].as_array().unwrap();
    assert!(
        catalogs
            .iter()
            .any(|catalog| catalog["id"] == "review" && catalog["writable"] == true),
        "{capabilities:#}"
    );

    // The rotated token hash persisted to the registry, so a restart keeps
    // the pairing.
    let registry = DbRegistry::load_or_init(&registry_path).unwrap();
    let stored = registry.find("review").unwrap();
    let config = stored.remote_image_upload.as_ref().unwrap();
    assert!(config.token_is_configured());
    assert!(config.token_matches(&token));
    assert!(config.sync_enabled);
    assert_eq!(config.clients.len(), 1);
    assert_eq!(config.clients[0].name, "OBSERVATORY-PC");

    // Single use: presenting the same code again is refused.
    let (status, refused) = call(
        router.clone(),
        "POST",
        "/api/sync/v1/pair",
        None,
        Some(json!({ "protocol_version": 1, "pairing_token": code })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused:#}");
}

#[tokio::test]
async fn each_pairing_mints_its_own_credential_and_revokes_alone() {
    let (_state, router, _registry_path, _directory) = harness();

    let pair = |router: Router, name: &'static str| async move {
        let (_, issued) = call(
            router.clone(),
            "POST",
            "/api/databases/review/pairing-token",
            None,
            None,
        )
        .await;
        let code = issued["data"]["pairing_token"]
            .as_str()
            .unwrap()
            .to_string();
        let (_, paired) = call(
            router,
            "POST",
            "/api/sync/v1/pair",
            None,
            Some(json!({
                "protocol_version": 1,
                "pairing_token": code,
                "client_name": name
            })),
        )
        .await;
        (
            paired["data"]["token"].as_str().unwrap().to_string(),
            paired["data"]["client_uuid"].as_str().unwrap().to_string(),
        )
    };

    let (first_token, first_uuid) = pair(router.clone(), "Laptop").await;
    let (second_token, _second_uuid) = pair(router.clone(), "Observatory").await;
    assert_ne!(first_token, second_token);

    // Both clients hold live credentials at once.
    for token in [&first_token, &second_token] {
        let (status, _) = call(
            router.clone(),
            "GET",
            "/api/sync/v1/capabilities",
            Some(token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Revoking the laptop signs out only the laptop.
    let (status, _) = call(
        router.clone(),
        "DELETE",
        &format!("/api/databases/review/clients/{first_uuid}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        router.clone(),
        "GET",
        "/api/sync/v1/capabilities",
        Some(&first_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        router,
        "GET",
        "/api/sync/v1/capabilities",
        Some(&second_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn issuing_needs_no_database_management() {
    // A pairing code grants a network client access to a catalog, not a
    // server path: an editor issues it with the gate off.
    let (state, router, _registry_path, _directory) = harness();
    state.set_allow_database_management(false);
    let (status, _) = call(
        router,
        "POST",
        "/api/databases/review/pairing-token",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// Issue a code of `kind` (`""` for the default) and pair with it.
async fn pair_as(router: Router, kind: &str, name: &str) -> (String, String) {
    let (status, issued) = call(
        router.clone(),
        "POST",
        &format!("/api/databases/review/pairing-token{kind}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{issued:#}");
    let (status, paired) = call(
        router,
        "POST",
        "/api/sync/v1/pair",
        None,
        Some(json!({
            "protocol_version": 1,
            "pairing_token": issued["data"]["pairing_token"],
            "client_name": name
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{paired:#}");
    (
        paired["data"]["token"].as_str().unwrap().to_string(),
        paired["data"]["client_uuid"].as_str().unwrap().to_string(),
    )
}

async fn can_sync(router: Router, token: &str) -> bool {
    let (status, _) = call(
        router,
        "GET",
        "/api/sync/v1/capabilities",
        Some(token),
        None,
    )
    .await;
    status == StatusCode::OK
}

#[tokio::test]
async fn a_paired_psf_guard_syncs_under_its_own_grant() {
    let (_state, router, registry_path, _directory) = harness();
    let stored = || {
        DbRegistry::load_or_init(&registry_path)
            .unwrap()
            .find("review")
            .unwrap()
            .remote_image_upload
            .clone()
            .unwrap()
    };

    // Pairing another PSF Guard turns on its grant, not the plugins'.
    let (peer_token, peer_uuid) = pair_as(router.clone(), "?kind=peer", "home-server").await;
    let config = stored();
    assert_eq!(config.clients[0].kind, RemoteClientKind::Peer);
    assert!(!config.sync_enabled);
    assert_eq!(config.peer_sync_enabled, Some(true));
    assert!(can_sync(router.clone(), &peer_token).await);

    // A second server pairs too, and each is removed alone.
    let (second_token, _) = pair_as(router.clone(), "?kind=peer", "laptop").await;
    assert!(can_sync(router.clone(), &second_token).await);
    let (status, _) = call(
        router.clone(),
        "DELETE",
        &format!("/api/databases/review/clients/{peer_uuid}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!can_sync(router.clone(), &peer_token).await);
    assert!(can_sync(router.clone(), &second_token).await);

    // Pairing a plugin turns on the plugins' grant and leaves the peers'.
    let (plugin_token, _) = pair_as(router.clone(), "", "OBSERVATORY-PC").await;
    let config = stored();
    assert!(config.sync_enabled);
    assert_eq!(config.peer_sync_enabled, Some(true));
    assert_eq!(
        config.clients.last().unwrap().kind,
        RemoteClientKind::Plugin
    );
    assert!(can_sync(router.clone(), &plugin_token).await);
}

#[tokio::test]
async fn pairing_a_plugin_does_not_let_peers_sync() {
    let (_state, router, registry_path, _directory) = harness();
    let (plugin_token, _) = pair_as(router.clone(), "", "OBSERVATORY-PC").await;
    assert!(can_sync(router.clone(), &plugin_token).await);
    let registry = DbRegistry::load_or_init(&registry_path).unwrap();
    let config = registry
        .find("review")
        .unwrap()
        .remote_image_upload
        .as_ref()
        .unwrap();
    assert!(config.sync_enabled);
    assert_eq!(config.peer_sync_enabled, Some(false));
}

#[tokio::test]
async fn adding_a_peer_with_a_code_pairs_this_server() {
    // The peer: a server that listens, with a code waiting.
    let (_peer_state, peer_router, peer_registry, _peer_directory) = harness();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let served = peer_router.clone();
    tokio::spawn(async move { axum::serve(listener, served).await.unwrap() });
    let (_, issued) = call(
        peer_router.clone(),
        "POST",
        "/api/databases/review/pairing-token?kind=peer",
        None,
        None,
    )
    .await;
    let code = issued["data"]["pairing_token"]
        .as_str()
        .unwrap()
        .to_string();

    // This server adds the peer with the code and no key.
    let (_state, router, registry_path, _directory) = harness();
    let (status, added) = call(
        router.clone(),
        "POST",
        "/api/peers",
        None,
        Some(json!({ "name": "Telescope", "base_url": base_url, "pairing_code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{added:#}");
    assert_eq!(added["data"]["catalog_id"], "review");
    assert_eq!(added["data"]["token_configured"], true);
    assert!(added["data"].get("token").is_none(), "{added:#}");

    // It stored the credential the peer minted, and the peer lists it.
    let registry = DbRegistry::load_or_init(&registry_path).unwrap();
    let token = registry.peers[0].token.clone();
    assert!(token.starts_with("psfrc_"), "{token}");
    let peer_side = DbRegistry::load_or_init(&peer_registry).unwrap();
    let config = peer_side
        .find("review")
        .unwrap()
        .remote_image_upload
        .as_ref()
        .unwrap();
    assert!(config.token_matches(&token));
    assert_eq!(config.clients.len(), 1);
    assert_eq!(config.clients[0].kind, RemoteClientKind::Peer);
    assert!(!config.clients[0].name.is_empty());
    assert!(can_sync(peer_router, &token).await);

    // A used code is refused, and nothing is stored for it.
    let (status, refused) = call(
        router,
        "POST",
        "/api/peers",
        None,
        Some(json!({ "name": "Again", "base_url": base_url, "pairing_code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused:#}");
    assert_eq!(
        DbRegistry::load_or_init(&registry_path)
            .unwrap()
            .peers
            .len(),
        1
    );
}
