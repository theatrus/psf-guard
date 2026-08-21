//! The calibration matching settings panel: one process-global block in the
//! database registry, shared by every database and both serving modes.
//!
//! GET is open to viewers so the panel can show the current values; PUT lands
//! in `requires_write`. A change applies immediately — the next master
//! selection or stack uses it — and persists through the registry, so browser
//! and desktop modes read the same value after a restart.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::db_registry::{CalibrationSettings, DbRegistry};
use crate::server::{
    api::ApiResponse,
    handlers::{require_registry_path, AppError},
    state::AppState,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct CalibrationSettingsResponse {
    /// The configured override, absent when the library default applies.
    pub rotation_tolerance_deg: Option<f64>,
    /// What applies when no override is set, so the panel can label the
    /// placeholder honestly instead of hard-coding a number that drifts.
    pub default_rotation_tolerance_deg: f64,
}

#[derive(Debug, Deserialize)]
pub struct UpdateCalibrationSettingsRequest {
    /// Degrees; `null` clears the override back to the library default.
    pub rotation_tolerance_deg: Option<f64>,
}

fn current_response(settings: Option<&CalibrationSettings>) -> CalibrationSettingsResponse {
    CalibrationSettingsResponse {
        rotation_tolerance_deg: settings.and_then(|settings| settings.rotation_tolerance_deg),
        default_rotation_tolerance_deg: seiza_calibration::MatchTolerances::default().rotation_deg,
    }
}

/// GET /api/settings/calibration
pub async fn get_calibration_settings(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<CalibrationSettingsResponse>>, AppError> {
    // A server without a persistent registry still has a current value: the
    // default. The panel shows it read-only rather than erroring.
    let settings = match require_registry_path(&state) {
        Ok(path) => {
            DbRegistry::load_or_init(&path)
                .map_err(|error| AppError::InternalError(error.to_string()))?
                .calibration
        }
        Err(_) => None,
    };
    Ok(Json(ApiResponse::success(current_response(
        settings.as_ref(),
    ))))
}

/// PUT /api/settings/calibration
pub async fn update_calibration_settings(
    State(state): State<Arc<AppState>>,
    Json(request): Json<UpdateCalibrationSettingsRequest>,
) -> Result<Json<ApiResponse<CalibrationSettingsResponse>>, AppError> {
    if let Some(degrees) = request.rotation_tolerance_deg {
        // 0 means "exact angle only", which is a legitimate ask; 180 is the
        // whole half-turn, past which the wrap makes larger values lies.
        if !degrees.is_finite() || !(0.0..=180.0).contains(&degrees) {
            return Err(AppError::BadRequest(
                "rotation tolerance must be between 0 and 180 degrees".into(),
            ));
        }
    }
    let path = require_registry_path(&state)?;
    let _registry_guard = state.registry_write.lock().await;
    let mut registry = DbRegistry::load_or_init(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    registry.calibration = request
        .rotation_tolerance_deg
        .map(|degrees| CalibrationSettings {
            rotation_tolerance_deg: Some(degrees),
        });
    registry
        .save(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    crate::calibration::configure_rotation_tolerance(request.rotation_tolerance_deg);
    Ok(Json(ApiResponse::success(current_response(
        registry.calibration.as_ref(),
    ))))
}
