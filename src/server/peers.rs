//! Managing remote PSF Guard peers, and syncing with one from the UI.
//!
//! `remote_sync` is the receiving half of the protocol and `sync_client` the
//! sending half; this is the surface that lets someone drive the sending half
//! from a browser instead of a terminal.
//!
//! The key never reaches the browser. It is written once through the
//! management-gated CRUD route, stored in the registry, and read back only by
//! this process when it talks to the peer.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

use crate::{
    commands::sync::{sync_remote, RemoteDirection, RemoteSyncOptions},
    db_registry::{DbRegistry, PeerEntry},
    server::{
        api::ApiResponse,
        handlers::{require_database_management_allowed, require_registry_path, AppError},
        state::AppState,
    },
};

/// A peer as the browser sees it: everything but the key.
#[derive(Debug, Serialize)]
pub struct PeerSummary {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub catalog_id: Option<String>,
    pub token_configured: bool,
}

impl From<&PeerEntry> for PeerSummary {
    fn from(entry: &PeerEntry) -> Self {
        Self {
            id: entry.id.clone(),
            name: entry.name.clone(),
            base_url: entry.base_url.clone(),
            catalog_id: entry.catalog_id.clone(),
            token_configured: !entry.token.trim().is_empty(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct UpsertPeerRequest {
    pub name: String,
    pub base_url: String,
    /// Absent on an edit means "keep the stored key".
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub catalog_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PeerCheck {
    pub reachable: bool,
    pub product: Option<String>,
    pub product_version: Option<String>,
    pub protocol_version: Option<u32>,
    pub catalogs: Vec<String>,
    pub capabilities: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RemoteSyncRequest {
    pub peer_id: String,
    /// `pull`, `push_planning`, or `push_grades`.
    pub direction: String,
    #[serde(default = "default_true")]
    pub dry_run: bool,
    #[serde(default = "default_true")]
    pub reviewed_only: bool,
    #[serde(default = "default_true")]
    pub with_image_data: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct RemoteSyncResult {
    pub applied: bool,
    pub peer_product: String,
    pub peer_catalog: String,
    pub summary: BTreeMap<String, i64>,
}

pub async fn list_peers(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<PeerSummary>>>, AppError> {
    Ok(Json(ApiResponse::success(
        load_registry(&state)?
            .peers
            .iter()
            .map(Into::into)
            .collect(),
    )))
}

pub async fn add_peer(
    State(state): State<Arc<AppState>>,
    Json(request): Json<UpsertPeerRequest>,
) -> Result<Json<ApiResponse<PeerSummary>>, AppError> {
    // Storing a credential and a URL this server will then send it to is
    // configuration, so it lives behind the same grant as the rest of the
    // registry.
    require_database_management_allowed(&state)?;
    let registry_path = require_registry_path(&state)?;
    let mut registry = load_registry(&state)?;

    let name = require_text(&request.name, "name")?;
    let base_url = validated_url(&request.base_url)?;
    let token = match request.token.as_deref().map(str::trim) {
        Some(token) if !token.is_empty() => token.to_string(),
        _ => return Err(AppError::BadRequest("a peer needs an API key".into())),
    };
    let id = crate::server::slug::compute_default_slug(&base_url);
    if registry.peers.iter().any(|peer| peer.id == id) {
        return Err(AppError::BadRequest(format!(
            "a peer for {base_url} is already configured"
        )));
    }
    let entry = PeerEntry {
        id,
        name,
        base_url,
        token,
        catalog_id: normalized(request.catalog_id),
    };
    registry.peers.push(entry.clone());
    save(&registry, &registry_path)?;
    Ok(Json(ApiResponse::success(PeerSummary::from(&entry))))
}

pub async fn update_peer(
    State(state): State<Arc<AppState>>,
    Path(peer_id): Path<String>,
    Json(request): Json<UpsertPeerRequest>,
) -> Result<Json<ApiResponse<PeerSummary>>, AppError> {
    require_database_management_allowed(&state)?;
    let registry_path = require_registry_path(&state)?;
    let mut registry = load_registry(&state)?;

    let name = require_text(&request.name, "name")?;
    let base_url = validated_url(&request.base_url)?;
    let catalog_id = normalized(request.catalog_id);
    let peer = registry
        .peers
        .iter_mut()
        .find(|peer| peer.id == peer_id)
        .ok_or(AppError::NotFound)?;
    peer.name = name;
    peer.base_url = base_url;
    peer.catalog_id = catalog_id;
    // An absent key means "leave it alone": the browser never received it, so
    // it cannot echo it back on a rename.
    if let Some(token) = request.token.as_deref().map(str::trim)
        && !token.is_empty()
    {
        peer.token = token.to_string();
    }
    let summary = PeerSummary::from(&*peer);
    save(&registry, &registry_path)?;
    Ok(Json(ApiResponse::success(summary)))
}

pub async fn remove_peer(
    State(state): State<Arc<AppState>>,
    Path(peer_id): Path<String>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    require_database_management_allowed(&state)?;
    let registry_path = require_registry_path(&state)?;
    let mut registry = load_registry(&state)?;
    let before = registry.peers.len();
    registry.peers.retain(|peer| peer.id != peer_id);
    if registry.peers.len() == before {
        return Err(AppError::NotFound);
    }
    save(&registry, &registry_path)?;
    Ok(Json(ApiResponse::success(true)))
}

/// Ask a peer who it is. Reachability is a normal answer, not an error: the
/// UI shows the reason beside the peer rather than a failed request.
pub async fn check_peer(
    State(state): State<Arc<AppState>>,
    Path(peer_id): Path<String>,
) -> Result<Json<ApiResponse<PeerCheck>>, AppError> {
    let peer = find_peer(&state, &peer_id)?;
    let client = match crate::sync_client::SyncClient::new(&peer.base_url, &peer.token) {
        Ok(client) => client,
        Err(error) => return Ok(Json(ApiResponse::success(unreachable(error)))),
    };
    Ok(Json(ApiResponse::success(
        match client.capabilities().await {
            Ok(capabilities) => PeerCheck {
                reachable: true,
                product: Some(capabilities.product),
                product_version: Some(capabilities.product_version),
                protocol_version: Some(capabilities.protocol_version),
                catalogs: capabilities
                    .catalogs
                    .iter()
                    .map(|catalog| catalog.id.clone())
                    .collect(),
                capabilities: capabilities.capabilities,
                error: None,
            },
            Err(error) => unreachable(error),
        },
    )))
}

/// Run one direction against a peer on behalf of the UI.
///
/// Defaults are the cautious ones — dry run, reviewed grades only — so an
/// omitted field can never write more than the caller asked for.
pub async fn sync_with_peer(
    State(state): State<Arc<AppState>>,
    Path(db_id): Path<String>,
    Json(request): Json<RemoteSyncRequest>,
) -> Result<Json<ApiResponse<RemoteSyncResult>>, AppError> {
    let context = state.get_database(&db_id).ok_or(AppError::NotFound)?;
    let peer = find_peer(&state, &request.peer_id)?;
    let direction = match request.direction.as_str() {
        "pull" => RemoteDirection::Pull,
        "push_planning" => RemoteDirection::PushPlanning,
        "push_grades" => RemoteDirection::PushGrades,
        other => {
            return Err(AppError::BadRequest(format!(
                "unknown sync direction '{other}'"
            )))
        }
    };
    // One write at a time, the same lock the local applies take, so a remote
    // pull cannot land in the middle of one.
    let _guard = if request.dry_run {
        None
    } else {
        Some(state.sync_apply_lock.lock().await)
    };
    let outcome = sync_remote(RemoteSyncOptions {
        direction,
        local_path: std::path::PathBuf::from(&context.database_path),
        local_id: context.id.clone(),
        peer_url: peer.base_url.clone(),
        peer_token: peer.token.clone(),
        peer_catalog: peer.catalog_id.clone(),
        reviewed_only: request.reviewed_only,
        dry_run: request.dry_run,
        with_image_data: request.with_image_data,
    })
    .await
    .map_err(|error| AppError::BadRequest(format!("{error:#}")))?;

    if !request.dry_run && direction == RemoteDirection::Pull {
        // A pull just wrote rows this database serves; drop the caches built
        // from the old contents.
        context.clear_directory_tree_cache();
        let _ = context.ensure_cache_available();
    }
    Ok(Json(ApiResponse::success(RemoteSyncResult {
        applied: outcome.applied,
        peer_product: outcome.peer_product,
        peer_catalog: outcome.peer_catalog,
        summary: outcome.summary,
    })))
}

fn unreachable(error: anyhow::Error) -> PeerCheck {
    PeerCheck {
        reachable: false,
        product: None,
        product_version: None,
        protocol_version: None,
        catalogs: vec![],
        capabilities: vec![],
        error: Some(format!("{error:#}")),
    }
}

fn find_peer(state: &AppState, peer_id: &str) -> Result<PeerEntry, AppError> {
    load_registry(state)?
        .peers
        .into_iter()
        .find(|peer| peer.id == peer_id)
        .ok_or(AppError::NotFound)
}

fn load_registry(state: &AppState) -> Result<DbRegistry, AppError> {
    let path = require_registry_path(state)?;
    DbRegistry::load_or_init(&path)
        .map_err(|error| AppError::InternalError(format!("loading registry: {error}")))
}

fn save(registry: &DbRegistry, path: &std::path::Path) -> Result<(), AppError> {
    registry
        .save(path)
        .map_err(|error| AppError::InternalError(format!("saving registry: {error}")))
}

fn require_text(value: &str, field: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::BadRequest(format!("{field} must not be empty")));
    }
    Ok(value.to_string())
}

/// Accept only an absolute HTTP(S) URL. This server will present a stored key
/// to whatever it names, so a typo that turns into a different scheme — or a
/// relative path — must not get that far.
fn validated_url(value: &str) -> Result<String, AppError> {
    let value = value.trim().trim_end_matches('/');
    if !value.starts_with("http://") && !value.starts_with("https://") {
        return Err(AppError::BadRequest(
            "a peer URL must start with http:// or https://".into(),
        ));
    }
    if value.len() <= "https://".len() {
        return Err(AppError::BadRequest("a peer URL needs a host".into()));
    }
    Ok(value.to_string())
}

fn normalized(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_url_must_be_absolute_http() {
        assert!(validated_url("telescope.local").is_err());
        assert!(validated_url("file:///etc/passwd").is_err());
        assert!(validated_url("https://").is_err());
        assert_eq!(
            validated_url("  https://scope.example:3000/ ").unwrap(),
            "https://scope.example:3000"
        );
    }

    #[test]
    fn the_summary_never_carries_the_key() {
        let entry = PeerEntry {
            id: "scope".into(),
            name: "Telescope".into(),
            base_url: "https://scope.example".into(),
            token: "a-secret-key-long-enough-for-this".into(),
            catalog_id: None,
        };
        let summary = PeerSummary::from(&entry);
        assert!(summary.token_configured);
        let json = serde_json::to_string(&summary).unwrap();
        assert!(!json.contains("a-secret-key"), "{json}");
    }
}
