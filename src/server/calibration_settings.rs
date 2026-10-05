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

use crate::calibration::ExternalMasterPolicy;
use crate::db_registry::{CalibrationSettings, DbRegistry};
use crate::server::{
    api::ApiResponse,
    handlers::{require_registry_path, update_registry, AppError},
    state::AppState,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct CalibrationSettingsResponse {
    /// The configured override, absent when the library default applies.
    pub rotation_tolerance_deg: Option<f64>,
    /// What applies when no override is set, so the panel can label the
    /// placeholder honestly instead of hard-coding a number that drifts.
    pub default_rotation_tolerance_deg: f64,
    /// How masters built by other software are used. Always populated: the
    /// default is `prefer`.
    pub external_masters: ExternalMasterPolicy,
    pub flat_star_masking: bool,
    /// Days; absent when the default applies.
    pub dark_reach_days: Option<f64>,
    pub default_dark_reach_days: f64,
    /// Frames; absent when the default applies.
    pub complete_dark_frames: Option<usize>,
    pub default_complete_dark_frames: usize,
}

#[derive(Debug, Deserialize)]
pub struct UpdateCalibrationSettingsRequest {
    /// Degrees; `null` clears the override back to the library default.
    pub rotation_tolerance_deg: Option<f64>,
    /// Omitted keeps the default, `prefer`.
    #[serde(default)]
    pub external_masters: Option<ExternalMasterPolicy>,
    /// Omitted preserves the existing setting, including requests from older clients.
    #[serde(default)]
    pub flat_star_masking: Option<bool>,
    /// Days. Omitted preserves the saved value; `null` restores the default.
    #[serde(default, deserialize_with = "present")]
    pub dark_reach_days: Option<Option<f64>>,
    /// Frames. Omitted preserves the saved value; `null` restores the default.
    #[serde(default, deserialize_with = "present")]
    pub complete_dark_frames: Option<Option<usize>>,
}

/// A field that was sent, `null` included, as opposed to one left out.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// The longest dark reach the panel accepts: ten years.
const MAX_DARK_REACH_DAYS: f64 = 3650.0;
/// More darks than one master holds would never be complete.
const MAX_COMPLETE_DARK_FRAMES: usize = 64;

fn current_response(settings: Option<&CalibrationSettings>) -> CalibrationSettingsResponse {
    CalibrationSettingsResponse {
        rotation_tolerance_deg: settings.and_then(|settings| settings.rotation_tolerance_deg),
        default_rotation_tolerance_deg: seiza_calibration::MatchTolerances::default().rotation_deg,
        external_masters: settings
            .and_then(|settings| settings.external_masters)
            .unwrap_or_default(),
        flat_star_masking: settings
            .and_then(|settings| settings.flat_star_masking)
            .unwrap_or(false),
        dark_reach_days: settings.and_then(|settings| settings.dark_reach_days),
        default_dark_reach_days: crate::calibration::DEFAULT_DARK_REACH_DAYS,
        complete_dark_frames: settings.and_then(|settings| settings.complete_dark_frames),
        default_complete_dark_frames: crate::calibration::DEFAULT_COMPLETE_DARK_FRAMES,
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
    if let Some(Some(days)) = request.dark_reach_days
        && (!days.is_finite() || !(1.0..=MAX_DARK_REACH_DAYS).contains(&days))
    {
        return Err(AppError::BadRequest(
            "dark reach must be between 1 and 3650 days".into(),
        ));
    }
    if let Some(Some(frames)) = request.complete_dark_frames
        && !(2..=MAX_COMPLETE_DARK_FRAMES).contains(&frames)
    {
        return Err(AppError::BadRequest(
            "a complete night of darks must be between 2 and 64 frames".into(),
        ));
    }
    // The default policy is not written down, so a registry that only ever
    // held defaults stays clean and older builds see nothing new.
    let external_masters = request
        .external_masters
        .filter(|policy| *policy != ExternalMasterPolicy::default());
    let calibration = update_registry(&state, |registry| {
        let flat_star_masking =
            requested_flat_star_masking(&request, registry.calibration.as_ref());
        let dark_reach_days = match request.dark_reach_days {
            Some(sent) => sent,
            None => registry
                .calibration
                .as_ref()
                .and_then(|settings| settings.dark_reach_days),
        };
        let complete_dark_frames = match request.complete_dark_frames {
            Some(sent) => sent,
            None => registry
                .calibration
                .as_ref()
                .and_then(|settings| settings.complete_dark_frames),
        };
        registry.calibration = (request.rotation_tolerance_deg.is_some()
            || external_masters.is_some()
            || flat_star_masking
            || dark_reach_days.is_some()
            || complete_dark_frames.is_some())
        .then_some(CalibrationSettings {
            rotation_tolerance_deg: request.rotation_tolerance_deg,
            external_masters,
            flat_star_masking: flat_star_masking.then_some(true),
            dark_reach_days,
            complete_dark_frames,
        });
        Ok(registry.calibration.clone())
    })
    .await?;
    crate::calibration::configure(calibration.as_ref());
    Ok(Json(ApiResponse::success(current_response(
        calibration.as_ref(),
    ))))
}

fn requested_flat_star_masking(
    request: &UpdateCalibrationSettingsRequest,
    current: Option<&CalibrationSettings>,
) -> bool {
    request
        .flat_star_masking
        .or_else(|| current.and_then(|settings| settings.flat_star_masking))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_reach_is_left_out_kept_or_cleared() {
        let left_out: UpdateCalibrationSettingsRequest =
            serde_json::from_str(r#"{"rotation_tolerance_deg":null}"#).unwrap();
        assert_eq!(left_out.dark_reach_days, None);
        let cleared: UpdateCalibrationSettingsRequest =
            serde_json::from_str(r#"{"rotation_tolerance_deg":null,"dark_reach_days":null}"#)
                .unwrap();
        assert_eq!(cleared.dark_reach_days, Some(None));
        let set: UpdateCalibrationSettingsRequest =
            serde_json::from_str(r#"{"rotation_tolerance_deg":null,"dark_reach_days":90}"#)
                .unwrap();
        assert_eq!(set.dark_reach_days, Some(Some(90.0)));
        let response = current_response(None);
        assert_eq!(response.dark_reach_days, None);
        assert_eq!(response.default_dark_reach_days, 183.0);
        assert_eq!(response.default_complete_dark_frames, 10);
    }

    #[test]
    fn flat_star_masking_defaults_off_and_omission_preserves_saved_choice() {
        assert!(!current_response(None).flat_star_masking);
        let settings = CalibrationSettings {
            flat_star_masking: Some(true),
            ..Default::default()
        };
        assert!(current_response(Some(&settings)).flat_star_masking);
        let mut request: UpdateCalibrationSettingsRequest =
            serde_json::from_str(r#"{"rotation_tolerance_deg":null}"#).unwrap();
        assert!(!requested_flat_star_masking(&request, None));
        assert!(requested_flat_star_masking(&request, Some(&settings)));
        request.flat_star_masking = Some(false);
        assert!(!requested_flat_star_masking(&request, Some(&settings)));
    }
}
