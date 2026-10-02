//! `GET`/`PUT /api/settings/storage`: the cache's disk limit, and what the
//! last pass over the cache volumes found.

use crate::db_registry::{DbRegistry, StorageSettings};
use crate::server::api::ApiResponse;
use crate::server::cache_budget::{self, VolumeReport};
use crate::server::handlers::{
    require_database_management_allowed, require_registry_path, AppError,
};
use crate::server::state::AppState;
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
pub struct StorageSettingsResponse {
    pub max_volume_percent: u8,
    pub default_max_volume_percent: u8,
    pub min_max_volume_percent: u8,
    /// Empty until the first pass, a minute after the server starts, and on
    /// a system that cannot read its volumes.
    pub volumes: Vec<VolumeReport>,
}

fn response() -> StorageSettingsResponse {
    StorageSettingsResponse {
        max_volume_percent: cache_budget::max_volume_percent(),
        default_max_volume_percent: cache_budget::DEFAULT_MAX_VOLUME_PERCENT,
        min_max_volume_percent: cache_budget::MIN_MAX_VOLUME_PERCENT,
        volumes: cache_budget::last_reports(),
    }
}

/// GET /api/settings/storage
pub async fn get_storage_settings() -> Json<ApiResponse<StorageSettingsResponse>> {
    Json(ApiResponse::success(response()))
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateStorageSettingsRequest {
    /// 100 turns culling off.
    pub max_volume_percent: u8,
}

/// PUT /api/settings/storage — deletes previews from the next pass on, so
/// it sits behind database management. The new limit is checked at once.
pub async fn update_storage_settings(
    State(state): State<Arc<AppState>>,
    Json(request): Json<UpdateStorageSettingsRequest>,
) -> Result<Json<ApiResponse<StorageSettingsResponse>>, AppError> {
    require_database_management_allowed(&state)?;
    let percent = request.max_volume_percent;
    if !(cache_budget::MIN_MAX_VOLUME_PERCENT..=100).contains(&percent) {
        return Err(AppError::BadRequest(format!(
            "the limit must be between {}% and 100%",
            cache_budget::MIN_MAX_VOLUME_PERCENT
        )));
    }
    let path = require_registry_path(&state)?;
    let settings =
        (percent != cache_budget::DEFAULT_MAX_VOLUME_PERCENT).then_some(StorageSettings {
            max_volume_percent: Some(percent),
        });
    {
        let _registry_guard = state.registry_write.lock().await;
        let mut registry = DbRegistry::load_or_init(&path)
            .map_err(|error| AppError::InternalError(error.to_string()))?;
        registry.storage = settings;
        registry
            .save(&path)
            .map_err(|error| AppError::InternalError(error.to_string()))?;
        cache_budget::configure(registry.storage.as_ref());
    }
    let state_for_pass = Arc::clone(&state);
    let reports = tokio::task::spawn_blocking(move || cache_budget::pass(&state_for_pass))
        .await
        .map_err(|error| AppError::InternalError(format!("storage check failed: {error}")))?;
    cache_budget::record(reports);
    Ok(Json(ApiResponse::success(response())))
}
