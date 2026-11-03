//! Settings › Review › Star detection: how the quality scan detects stars
//! on each rig's frames. One process-global block in the database registry
//! ([`crate::db_registry::DbRegistry::star_detection`]), shared by every
//! database and both serving modes: a default row, and each rig's own.
//!
//! GET is open to viewers so the panel can show the current values; PUT and
//! DELETE land in `requires_write`. A change applies as soon as it is saved:
//! frames whose settings now resolve differently read as outdated, and the
//! next quality scan or backfill measures them again (see
//! [`crate::server::spatial_scan::detection_is_current`]).

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::db_registry::DbRegistry;
use crate::server::{
    api::ApiResponse,
    calibration_settings::{catalog_rigs, rig_detail, rig_key, MAX_RIG_NAME},
    handlers::{require_registry_path, update_registry, AppError},
    state::AppState,
};
use crate::star_detection::{RigStarDetection, RigStarDetectionEntry, StarDetectionSettings};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StarDetectionSettingsResponse {
    /// What a rig without a row of its own uses.
    pub default: RigStarDetection,
    /// N.I.N.A.'s own defaults, which Reset goes back to.
    pub builtin: RigStarDetection,
    /// Every rig an open catalog records, and every rig with a row of its
    /// own, by name.
    pub rigs: Vec<StarDetectionRigView>,
}

/// One rig, labelled as Settings › Processing › Calibration › Rigs labels it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StarDetectionRigView {
    pub rig_uuid: String,
    pub name: String,
    /// Its frame size and binning, which tell two rigs of one name apart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The databases that record it, by name.
    pub databases: Vec<String>,
    /// Its own row; absent when it uses the default row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<RigStarDetection>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateStarDetectionRequest {
    /// The new default row. Every field is required, so a typo cannot pass
    /// for a default.
    pub default: StrictRigStarDetection,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateRigStarDetectionRequest {
    /// The rig's name as the panel showed it, kept to name the rig when no
    /// open catalog holds it.
    #[serde(default)]
    pub name: Option<String>,
    pub settings: StrictRigStarDetection,
}

/// [`RigStarDetection`] as a request sends it: every field present.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrictRigStarDetection {
    pub detector: crate::star_detection::DetectorChoice,
    pub sensitivity: crate::star_detection::Sensitivity,
    pub noise_reduction: crate::star_detection::NoiseReduction,
    pub hocus_focus_version: crate::star_detection::HocusFocusVersion,
}

impl From<StrictRigStarDetection> for RigStarDetection {
    fn from(sent: StrictRigStarDetection) -> Self {
        Self {
            detector: sent.detector,
            sensitivity: sent.sensitivity,
            noise_reduction: sent.noise_reduction,
            hocus_focus_version: sent.hocus_focus_version,
        }
    }
}

/// The most rigs that may have a row of their own. Far more than any
/// observatory has, and a bound on what one editor can make the registry
/// hold.
const MAX_RIGS: usize = 256;

/// One save at a time, from its registry write to the value in force, so
/// two saves at once cannot leave the server using one and the registry
/// holding the other.
static SAVING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Keep the block only when it differs from the defaults, so a registry
/// that only ever held defaults stays clean.
fn tidy(settings: StarDetectionSettings) -> Option<StarDetectionSettings> {
    (settings != StarDetectionSettings::default()).then_some(settings)
}

/// The registry's block as it stands, or the defaults.
fn load_settings(state: &AppState) -> Result<StarDetectionSettings, AppError> {
    // A server without a persistent registry still has values in force:
    // the panel shows them read-only rather than erroring.
    match require_registry_path(state) {
        Ok(path) => Ok(DbRegistry::load_or_init(&path)
            .map_err(|error| AppError::InternalError(error.to_string()))?
            .star_detection
            .unwrap_or_default()),
        Err(_) => Ok((*crate::star_detection::current()).clone()),
    }
}

/// Every rig the open catalogs record, merged by UUID, with every rig that
/// has a row of its own; rigs with rows first, then by name.
fn merge_rig_views(
    listed: Vec<(String, String, Vec<crate::calibration::RigListing>)>,
    settings: &StarDetectionSettings,
) -> Vec<StarDetectionRigView> {
    let mut views: BTreeMap<String, StarDetectionRigView> = BTreeMap::new();
    for (_, database, rigs) in listed {
        for rig in rigs {
            let view = views
                .entry(rig.rig_uuid.clone())
                .or_insert_with(|| StarDetectionRigView {
                    rig_uuid: rig.rig_uuid.clone(),
                    name: rig.name.clone(),
                    detail: rig_detail(&rig),
                    databases: Vec::new(),
                    settings: None,
                });
            if !view.databases.contains(&database) {
                view.databases.push(database.clone());
            }
        }
    }
    for (uuid, entry) in &settings.rigs {
        let view = views
            .entry(uuid.clone())
            .or_insert_with(|| StarDetectionRigView {
                rig_uuid: uuid.clone(),
                name: entry.name.clone().unwrap_or_else(|| "Unnamed rig".into()),
                detail: None,
                databases: Vec::new(),
                settings: None,
            });
        view.settings = Some(entry.settings);
    }
    let mut views: Vec<StarDetectionRigView> = views.into_values().collect();
    views.sort_by(|left, right| {
        left.settings
            .is_none()
            .cmp(&right.settings.is_none())
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.rig_uuid.cmp(&right.rig_uuid))
    });
    views
}

async fn respond(
    state: &Arc<AppState>,
    settings: StarDetectionSettings,
) -> Json<ApiResponse<StarDetectionSettingsResponse>> {
    let rigs = merge_rig_views(catalog_rigs(state).await, &settings);
    Json(ApiResponse::success(StarDetectionSettingsResponse {
        default: settings.default,
        builtin: RigStarDetection::default(),
        rigs,
    }))
}

/// Change the block, save the registry, and only once it is saved put the
/// settings in force. A save that fails changes nothing in force.
async fn save_and_apply(
    state: &Arc<AppState>,
    edit: impl FnOnce(&mut StarDetectionSettings) -> Result<(), AppError>,
) -> Result<StarDetectionSettings, AppError> {
    let _saving = SAVING.lock().await;
    let settings = update_registry(state, |registry| {
        let mut settings = registry.star_detection.clone().unwrap_or_default();
        edit(&mut settings)?;
        registry.star_detection = tidy(settings.clone());
        Ok(settings)
    })
    .await?;
    crate::star_detection::configure(Some(&settings));
    Ok(settings)
}

/// GET /api/settings/star-detection
pub async fn get_star_detection_settings(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<StarDetectionSettingsResponse>>, AppError> {
    let settings = load_settings(&state)?;
    Ok(respond(&state, settings).await)
}

/// PUT /api/settings/star-detection: replace the default row.
pub async fn update_star_detection_settings(
    State(state): State<Arc<AppState>>,
    Json(request): Json<UpdateStarDetectionRequest>,
) -> Result<Json<ApiResponse<StarDetectionSettingsResponse>>, AppError> {
    let settings = save_and_apply(&state, |settings| {
        settings.default = request.default.into();
        Ok(())
    })
    .await?;
    Ok(respond(&state, settings).await)
}

/// The rig's name as the request sends it: trimmed, absent when blank,
/// refused when too long.
fn rig_name(name: Option<&str>) -> Result<Option<String>, AppError> {
    let name = name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    if name
        .as_ref()
        .is_some_and(|name| name.chars().count() > MAX_RIG_NAME)
    {
        return Err(AppError::BadRequest(format!(
            "a rig's name is at most {MAX_RIG_NAME} characters"
        )));
    }
    Ok(name)
}

/// Give a rig its own row, within [`MAX_RIGS`].
fn set_rig(
    settings: &mut StarDetectionSettings,
    key: String,
    name: Option<String>,
    row: RigStarDetection,
) -> Result<(), AppError> {
    if !settings.rigs.contains_key(&key) && settings.rigs.len() >= MAX_RIGS {
        return Err(AppError::BadRequest(format!(
            "at most {MAX_RIGS} rigs can have star detection settings of their own; set one \
             to use the default first"
        )));
    }
    settings.rigs.insert(
        key,
        RigStarDetectionEntry {
            name,
            settings: row,
        },
    );
    Ok(())
}

/// PUT /api/settings/star-detection/rigs/{rig_uuid}: give a rig its own
/// row, replacing any it had.
pub async fn update_rig_star_detection_settings(
    State(state): State<Arc<AppState>>,
    Path(rig_uuid): Path<String>,
    Json(request): Json<UpdateRigStarDetectionRequest>,
) -> Result<Json<ApiResponse<StarDetectionSettingsResponse>>, AppError> {
    let key = rig_key(&rig_uuid)?;
    let name = rig_name(request.name.as_deref())?;
    let row: RigStarDetection = request.settings.into();
    let settings = save_and_apply(&state, |settings| set_rig(settings, key, name, row)).await?;
    Ok(respond(&state, settings).await)
}

/// DELETE /api/settings/star-detection/rigs/{rig_uuid}: the rig uses the
/// default row again.
pub async fn delete_rig_star_detection_settings(
    State(state): State<Arc<AppState>>,
    Path(rig_uuid): Path<String>,
) -> Result<Json<ApiResponse<StarDetectionSettingsResponse>>, AppError> {
    let key = rig_key(&rig_uuid)?;
    let settings = save_and_apply(&state, |settings| {
        settings.rigs.remove(&key);
        Ok(())
    })
    .await?;
    Ok(respond(&state, settings).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::star_detection::{DetectorChoice, HocusFocusVersion, Sensitivity};
    use serde_json::json;

    #[test]
    fn a_request_must_name_every_field_with_a_known_value() {
        let good: UpdateStarDetectionRequest = serde_json::from_value(json!({
            "default": {
                "detector": "hocus_focus",
                "sensitivity": "highest",
                "noise_reduction": "median",
                "hocus_focus_version": "v3",
            }
        }))
        .unwrap();
        let row: RigStarDetection = good.default.into();
        assert_eq!(row.detector, DetectorChoice::HocusFocus);
        assert_eq!(row.hocus_focus_version, HocusFocusVersion::V3);
        for bad in [
            json!({"default": {"detector": "auto"}}),
            json!({"default": {
                "detector": "sextractor", "sensitivity": "high",
                "noise_reduction": "none", "hocus_focus_version": "v4"}}),
            json!({"default": {
                "detector": "auto", "sensitivity": "high", "noise_reduction": "none",
                "hocus_focus_version": "v4", "colour": "red"}}),
            json!({"default": {
                "detector": "auto", "sensitivity": "high",
                "noise_reduction": "none", "hocus_focus_version": "v5"}}),
        ] {
            assert!(
                serde_json::from_value::<UpdateStarDetectionRequest>(bad.clone()).is_err(),
                "{bad} is refused"
            );
        }
        assert!(
            serde_json::from_value::<UpdateRigStarDetectionRequest>(json!({
                "settings": {
                    "detector": "nina", "sensitivity": "normal",
                    "noise_reduction": "none", "hocus_focus_version": "v4"},
                "rules": {}
            }))
            .is_err()
        );
    }

    #[test]
    fn rig_names_and_counts_are_bounded() {
        assert_eq!(rig_name(Some("  C925  ")).unwrap(), Some("C925".into()));
        assert_eq!(rig_name(Some("   ")).unwrap(), None);
        assert!(rig_name(Some(&"x".repeat(MAX_RIG_NAME + 1))).is_err());

        let mut settings = StarDetectionSettings::default();
        for index in 0..MAX_RIGS {
            set_rig(
                &mut settings,
                format!("rig-{index}"),
                None,
                RigStarDetection::default(),
            )
            .unwrap();
        }
        let refused = set_rig(
            &mut settings,
            "one-too-many".into(),
            None,
            RigStarDetection::default(),
        );
        assert!(matches!(refused, Err(AppError::BadRequest(_))));
        // A rig that has a row may still change it.
        set_rig(
            &mut settings,
            "rig-0".into(),
            Some("Askar".into()),
            RigStarDetection {
                sensitivity: Sensitivity::Normal,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            settings.rigs["rig-0"].settings.sensitivity,
            Sensitivity::Normal
        );
    }

    #[test]
    fn defaults_are_not_written_down() {
        assert_eq!(tidy(StarDetectionSettings::default()), None);
        let mut changed = StarDetectionSettings::default();
        changed.default.detector = DetectorChoice::Nina;
        assert_eq!(tidy(changed.clone()), Some(changed));
        // A rig set to exactly the defaults still has a row of its own: it
        // keeps them when the default row changes.
        let mut pinned = StarDetectionSettings::default();
        pinned
            .rigs
            .insert("rig".into(), RigStarDetectionEntry::default());
        assert!(tidy(pinned).is_some());
    }

    #[test]
    fn rigs_with_rows_come_first_and_every_catalog_is_listed() {
        let rig = |uuid: &str, name: &str| crate::calibration::RigListing {
            rig_uuid: uuid.into(),
            name: name.into(),
            width: Some(6248),
            height: Some(4176),
            binning_x: Some(1),
            binning_y: Some(1),
            frames: 4,
        };
        let mut settings = StarDetectionSettings::default();
        settings.rigs.insert(
            "u2".into(),
            RigStarDetectionEntry {
                name: Some("Redcat".into()),
                settings: RigStarDetection {
                    detector: DetectorChoice::HocusFocus,
                    ..Default::default()
                },
            },
        );
        settings
            .rigs
            .insert("gone".into(), RigStarDetectionEntry::default());
        let views = merge_rig_views(
            vec![
                ("a".into(), "Askar".into(), vec![rig("u1", "Askar107")]),
                (
                    "b".into(),
                    "Remote".into(),
                    vec![rig("u1", "Askar107"), rig("u2", "Redcat")],
                ),
            ],
            &settings,
        );
        let names: Vec<&str> = views.iter().map(|view| view.name.as_str()).collect();
        assert_eq!(names, vec!["Redcat", "Unnamed rig", "Askar107"]);
        assert_eq!(views[2].databases, vec!["Askar", "Remote"]);
        assert_eq!(views[2].detail.as_deref(), Some("6248×4176 · bin 1×1"));
        assert_eq!(views[2].settings, None);
        assert_eq!(
            views[0].settings.map(|row| row.detector),
            Some(DetectorChoice::HocusFocus)
        );
    }
}
