//! One-token pairing for remote clients, modeled on chatstronomy's hub
//! pairing: the operator mints a short-lived, single-use pairing code in
//! Settings; the client presents it once and receives the durable bearer
//! token in exchange. Nobody transcribes a long-lived secret by hand, and
//! the code is worthless after one use or an hour. Every pairing mints its
//! own credential, so the operator revokes one client without signing out
//! the rest.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::server::{
    api::ApiResponse,
    handlers::{require_database_management_allowed, require_registry_path, AppError},
    remote_audit::{AuditAction, AuditOutcome, AuditRecord},
    state::AppState,
};

/// Pairing codes die after an hour, matching chatstronomy's TTL: long
/// enough to walk to the telescope machine, short enough that a leaked
/// code from a screenshot goes stale the same evening.
const PAIRING_TTL_SECONDS: i64 = 3600;
const PAIRING_TOKEN_PREFIX: &str = "psfpt_";
const CLIENT_TOKEN_PREFIX: &str = "psfrc_";

/// A fresh high-entropy secret: 64 hex chars from two UUIDv4s.
fn random_secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push_str(&format!("{byte:02x}"));
    }
    encoded
}

fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

struct PendingPairing {
    db_id: String,
    expires_at: i64,
}

/// In-memory store of outstanding pairing codes, keyed by token hash.
///
/// Deliberately not persisted: a pairing code is an ephemeral handshake, and
/// a server restart mid-pairing simply means reissuing the code. Only the
/// hash lives here — the plaintext exists once, in the issue response.
#[derive(Default)]
pub struct PairingStore {
    pending: Mutex<HashMap<String, PendingPairing>>,
}

impl PairingStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a code for one catalog. Any earlier unconsumed code for the same
    /// catalog is revoked — one outstanding code per catalog, so a stale
    /// sticky note cannot pair after the operator issued a fresh one.
    pub fn issue(&self, db_id: &str) -> Result<(String, i64), AppError> {
        let token = format!("{PAIRING_TOKEN_PREFIX}{}", random_secret());
        let expires_at = unix_seconds() + PAIRING_TTL_SECONDS;
        let mut pending = self.lock()?;
        let now = unix_seconds();
        pending.retain(|_, entry| entry.db_id != db_id && entry.expires_at > now);
        pending.insert(
            hash_token(&token),
            PendingPairing {
                db_id: db_id.to_string(),
                expires_at,
            },
        );
        Ok((token, expires_at))
    }

    /// Consume a code, returning the catalog it pairs. Single-use: a second
    /// presentation of the same code finds nothing.
    pub fn consume(&self, token: &str) -> Result<Option<String>, AppError> {
        let mut pending = self.lock()?;
        let Some(entry) = pending.remove(&hash_token(token)) else {
            return Ok(None);
        };
        if entry.expires_at <= unix_seconds() {
            return Ok(None);
        }
        Ok(Some(entry.db_id))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, PendingPairing>>, AppError> {
        self.pending
            .lock()
            .map_err(|error| AppError::InternalError(format!("pairing lock poisoned: {error}")))
    }
}

#[derive(Debug, Serialize)]
pub struct PairingCode {
    pub pairing_token: String,
    pub expires_at: i64,
}

/// `POST /api/databases/{db_id}/pairing-token` — operator-side, management
/// gated exactly like editing the database the code will grant.
pub async fn issue_pairing_token_route(
    State(state): State<Arc<AppState>>,
    Path(db_id): Path<String>,
) -> Result<Json<ApiResponse<PairingCode>>, AppError> {
    require_database_management_allowed(&state)?;
    require_registry_path(&state)?;
    if !state.databases.read().unwrap().contains_key(&db_id) {
        return Err(AppError::NotFound);
    }
    let (pairing_token, expires_at) = state.pairing.issue(&db_id)?;
    Ok(Json(ApiResponse::success(PairingCode {
        pairing_token,
        expires_at,
    })))
}

#[derive(Debug, Deserialize)]
pub struct PairRequest {
    pub protocol_version: u32,
    pub pairing_token: String,
    /// Operator-facing label for this install ("OBSERVATORY-PC · Profile
    /// Astro"), shown in the paired-clients list.
    #[serde(default)]
    pub client_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PairResult {
    pub catalog_id: String,
    pub catalog_name: String,
    /// This client's credential id — the handle the operator revokes.
    pub client_uuid: String,
    /// The durable bearer token. This response is the only place the
    /// plaintext ever exists — the registry stores a salted hash.
    pub token: String,
    pub product: &'static str,
    pub product_version: &'static str,
}

/// `POST /api/sync/v1/pair` — client-side exchange. The pairing code is the
/// entire authorization: it was minted behind the management gate and dies
/// on first use. Each pairing mints its own credential — other paired
/// clients keep working, and the operator revokes them one by one.
pub async fn pair_route(
    State(state): State<Arc<AppState>>,
    Json(request): Json<PairRequest>,
) -> Result<Json<ApiResponse<PairResult>>, AppError> {
    use crate::db_registry::{DbRegistry, RemoteImageUploadConfig};
    use crate::server::database_context::DatabaseContext;

    let audit = |db_id: &str, outcome, detail: Option<&str>| {
        state.remote_audit.record(
            db_id,
            AuditAction::Pair,
            outcome,
            AuditRecord {
                detail,
                ..Default::default()
            },
        );
    };
    if request.protocol_version != 1 {
        return Err(AppError::BadRequest(format!(
            "unsupported pairing protocol {}",
            request.protocol_version
        )));
    }
    let Some(db_id) = state.pairing.consume(&request.pairing_token)? else {
        audit("-", AuditOutcome::Refused, Some("unknown or expired code"));
        return Err(AppError::Forbidden(
            "pairing code is unknown, expired, or already used".into(),
        ));
    };

    let registry_path = require_registry_path(&state)?;
    let _registry_guard = state.registry_write.lock().await;
    let mut registry = DbRegistry::load_or_init(&registry_path)
        .map_err(|error| AppError::InternalError(format!("loading registry: {error}")))?;
    let Some(entry) = registry
        .databases
        .iter_mut()
        .find(|entry| entry.id == db_id)
    else {
        audit(&db_id, AuditOutcome::Failed, Some("catalog vanished"));
        return Err(AppError::NotFound);
    };

    let token = format!("{CLIENT_TOKEN_PREFIX}{}", random_secret());
    let client_name = request
        .client_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Paired client");
    let config = entry
        .remote_image_upload
        .get_or_insert_with(RemoteImageUploadConfig::default);
    let client_uuid = config
        .add_client(client_name, &token)
        .map_err(|error| AppError::InternalError(format!("storing client token: {error}")))?;
    // Pairing exists to connect a scheduler-sync client; upload stays
    // whatever the operator configured (it needs a receive directory too).
    config.sync_enabled = true;
    let entry = entry.clone();

    let context = Arc::new(
        DatabaseContext::new(
            entry.id.clone(),
            entry.name.clone(),
            entry.db_path.clone(),
            entry.image_dirs.clone(),
            entry.remote_image_upload.clone(),
            entry.export_dir.clone(),
            state.cache_dir_root.clone(),
        )
        .map_err(|error| AppError::InternalError(format!("reopening database: {error}")))?,
    );
    registry
        .save(&registry_path)
        .map_err(|error| AppError::InternalError(format!("persisting registry: {error}")))?;
    state
        .databases
        .write()
        .unwrap()
        .insert(entry.id.clone(), context);

    audit(&db_id, AuditOutcome::Ok, Some(client_name));
    tracing::info!("remote client \"{client_name}\" paired with catalog {db_id}");
    Ok(Json(ApiResponse::success(PairResult {
        catalog_id: entry.id,
        catalog_name: entry.name,
        client_uuid,
        token,
        product: "PSF Guard",
        product_version: env!("CARGO_PKG_VERSION"),
    })))
}

/// `DELETE /api/databases/{db_id}/clients/{client_uuid}` — revoke one
/// paired client. Management gated: same trust as issuing a code. Other
/// clients and the legacy manual key keep working.
pub async fn revoke_client_route(
    State(state): State<Arc<AppState>>,
    Path((db_id, client_uuid)): Path<(String, String)>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    use crate::db_registry::DbRegistry;
    use crate::server::database_context::DatabaseContext;

    require_database_management_allowed(&state)?;
    let registry_path = require_registry_path(&state)?;
    let _registry_guard = state.registry_write.lock().await;
    let mut registry = DbRegistry::load_or_init(&registry_path)
        .map_err(|error| AppError::InternalError(format!("loading registry: {error}")))?;
    let Some(entry) = registry
        .databases
        .iter_mut()
        .find(|entry| entry.id == db_id)
    else {
        return Err(AppError::NotFound);
    };
    let removed = entry
        .remote_image_upload
        .as_mut()
        .is_some_and(|config| config.revoke_client(&client_uuid));
    if !removed {
        return Err(AppError::NotFound);
    }
    let entry = entry.clone();
    let context = Arc::new(
        DatabaseContext::new(
            entry.id.clone(),
            entry.name.clone(),
            entry.db_path.clone(),
            entry.image_dirs.clone(),
            entry.remote_image_upload.clone(),
            entry.export_dir.clone(),
            state.cache_dir_root.clone(),
        )
        .map_err(|error| AppError::InternalError(format!("reopening database: {error}")))?,
    );
    registry
        .save(&registry_path)
        .map_err(|error| AppError::InternalError(format!("persisting registry: {error}")))?;
    state.databases.write().unwrap().insert(entry.id, context);
    tracing::info!("revoked paired client {client_uuid} on catalog {db_id}");
    Ok(Json(ApiResponse::success(true)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_codes_are_single_use() {
        let store = PairingStore::new();
        let (token, _) = store.issue("catalog").unwrap();
        assert!(token.starts_with(PAIRING_TOKEN_PREFIX));
        assert_eq!(store.consume(&token).unwrap().as_deref(), Some("catalog"));
        assert_eq!(store.consume(&token).unwrap(), None);
    }

    #[test]
    fn reissuing_revokes_the_previous_code_for_that_catalog() {
        let store = PairingStore::new();
        let (first, _) = store.issue("catalog").unwrap();
        let (second, _) = store.issue("catalog").unwrap();
        assert_eq!(store.consume(&first).unwrap(), None);
        assert_eq!(store.consume(&second).unwrap().as_deref(), Some("catalog"));
    }

    #[test]
    fn codes_for_other_catalogs_survive_a_reissue() {
        let store = PairingStore::new();
        let (other, _) = store.issue("other").unwrap();
        let (_, _) = store.issue("catalog").unwrap();
        assert_eq!(store.consume(&other).unwrap().as_deref(), Some("other"));
    }

    #[test]
    fn unknown_codes_consume_to_nothing() {
        let store = PairingStore::new();
        assert_eq!(store.consume("psfpt_nope").unwrap(), None);
    }
}
