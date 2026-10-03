//! The export defaults settings panel: one process-global block in the
//! database registry, shared by every database and both serving modes.
//!
//! GET is open to viewers so the panel and the export dialog can show the
//! current default; PUT lands in `requires_write`. The dialog still offers
//! every layout on each export — this only seeds the choice.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::commands::export::wbpp::WbppOptions;
use crate::commands::export::ExportLayout;
use crate::db_registry::{DbRegistry, ExportSettings};
use crate::server::{
    api::ApiResponse,
    handlers::{require_registry_path, update_registry, AppError},
    state::AppState,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportSettingsResponse {
    /// The layout the export dialog starts from. Never absent: an
    /// unconfigured registry resolves to the standard layout.
    pub default_layout: ExportLayout,
    /// The WBPP settings an export or an in-app run starts from. Never
    /// absent: an unconfigured registry resolves to the defaults.
    pub wbpp: WbppOptions,
}

#[derive(Debug, Deserialize)]
pub struct UpdateExportSettingsRequest {
    pub default_layout: ExportLayout,
    /// Absent keeps the stored WBPP settings.
    #[serde(default)]
    pub wbpp: Option<WbppOptions>,
}

fn current_response(settings: Option<&ExportSettings>) -> ExportSettingsResponse {
    ExportSettingsResponse {
        default_layout: settings
            .and_then(|settings| settings.default_layout)
            .unwrap_or_default(),
        wbpp: settings
            .and_then(|settings| settings.wbpp.clone())
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
    let export = update_registry(&state, |registry| {
        // The defaults are what an absent block already means; storing
        // nothing keeps the registry clean for older builds reading the file.
        let wbpp = request
            .wbpp
            .or_else(|| {
                registry
                    .export
                    .as_ref()
                    .and_then(|export| export.wbpp.clone())
            })
            .filter(|wbpp| *wbpp != WbppOptions::default());
        let default_layout = match request.default_layout {
            ExportLayout::Standard => None,
            layout => Some(layout),
        };
        registry.export = if default_layout.is_none() && wbpp.is_none() {
            None
        } else {
            Some(ExportSettings {
                default_layout,
                wbpp,
            })
        };
        Ok(registry.export.clone())
    })
    .await?;
    Ok(Json(ApiResponse::success(current_response(
        export.as_ref(),
    ))))
}
