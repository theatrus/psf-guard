//! The export defaults settings panel: one process-global block in the
//! database registry, shared by every database and both serving modes.
//!
//! GET is open to viewers so the panel and the export dialog can show the
//! current default; PUT lands in `requires_write`. The dialog still offers
//! every layout on each export — this only seeds the choice.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::commands::export::ExportLayout;
use crate::db_registry::{DbRegistry, ExportSettings};
use crate::server::{
    api::ApiResponse,
    handlers::{require_registry_path, AppError},
    state::AppState,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportSettingsResponse {
    /// The layout the export dialog starts from. Never absent: an
    /// unconfigured registry resolves to the standard layout.
    pub default_layout: ExportLayout,
}

#[derive(Debug, Deserialize)]
pub struct UpdateExportSettingsRequest {
    pub default_layout: ExportLayout,
}

fn current_response(settings: Option<&ExportSettings>) -> ExportSettingsResponse {
    ExportSettingsResponse {
        default_layout: settings
            .and_then(|settings| settings.default_layout)
            .unwrap_or_default(),
    }
}

/// GET /api/settings/export
pub async fn get_export_settings(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<ExportSettingsResponse>>, AppError> {
    // A server without a persistent registry still has a current value: the
    // default. The panel shows it read-only rather than erroring.
    let settings = match require_registry_path(&state) {
        Ok(path) => {
            DbRegistry::load_or_init(&path)
                .map_err(|error| AppError::InternalError(error.to_string()))?
                .export
        }
        Err(_) => None,
    };
    Ok(Json(ApiResponse::success(current_response(
        settings.as_ref(),
    ))))
}

/// PUT /api/settings/export
pub async fn update_export_settings(
    State(state): State<Arc<AppState>>,
    Json(request): Json<UpdateExportSettingsRequest>,
) -> Result<Json<ApiResponse<ExportSettingsResponse>>, AppError> {
    let path = require_registry_path(&state)?;
    let _registry_guard = state.registry_write.lock().await;
    let mut registry = DbRegistry::load_or_init(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    // Standard is what an absent block already means; storing nothing keeps
    // the registry clean for older builds reading the same file.
    registry.export = match request.default_layout {
        ExportLayout::Standard => None,
        layout => Some(ExportSettings {
            default_layout: Some(layout),
        }),
    };
    registry
        .save(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    Ok(Json(ApiResponse::success(current_response(
        registry.export.as_ref(),
    ))))
}
