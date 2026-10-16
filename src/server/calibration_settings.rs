//! The calibration settings panel: one process-global block in the database
//! registry, shared by every database and both serving modes, with each
//! rig's own values beside it.
//!
//! GET is open to viewers so the panel can show the current values; PUT and
//! DELETE land in `requires_write`. A change applies immediately — the next
//! match, check, grade or stack uses it — and persists through the
//! registry, so browser and desktop modes read the same value after a
//! restart.
//!
//! A value comes from the light's rig when it has one, then the server's,
//! then the default (see [`crate::calibration::rules`]). Every value is
//! checked against [`crate::calibration::rules::fields`] here: a key the
//! list does not know, a value of the wrong kind or out of range is refused
//! with a message naming it.

use axum::{
    extract::{Path, State},
    Json,
};
use rusqlite::OpenFlags;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::calibration::rules::{self, FieldSpec, RuleValues};
use crate::calibration::{DarkChoice, ExternalMasterPolicy};
use crate::db_registry::{CalibrationSettings, DbRegistry, RigCalibrationSettings};
use crate::server::{
    api::ApiResponse,
    handlers::{require_registry_path, update_registry, AppError},
    state::AppState,
};

#[derive(Debug, Serialize)]
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
    /// Which darks a master starts from. Always populated: the default is
    /// `complete`.
    pub dark_choice: DarkChoice,
    /// Days; absent when the default applies, zero when the limit is lifted.
    pub flat_max_age_days: Option<f64>,
    pub default_flat_max_age_days: f64,
    /// Stacks leave out lights that cannot be calibrated.
    pub exclude_uncalibrated: bool,
    /// Every tunable setting, with its range, default and what changing it
    /// changes.
    pub fields: &'static [FieldSpec],
    /// The server's values, by key; a key not listed uses the default.
    pub rules: RuleValues,
    /// Every rig an open catalog records, and every rig with values of its
    /// own, by name.
    pub rigs: Vec<RigSettingsView>,
}

/// One rig, and the values it sets for itself.
#[derive(Debug, Clone, Serialize)]
pub struct RigSettingsView {
    pub rig_uuid: String,
    pub name: String,
    /// Its frame size and binning, which tell two rigs of one name apart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The databases that record it.
    pub databases: Vec<String>,
    /// Calibration frames those databases hold from it.
    pub frames: usize,
    /// Its own values; a key not listed inherits.
    pub rules: RuleValues,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateCalibrationSettingsRequest {
    /// Degrees. Omitted preserves the saved value; `null` restores the
    /// default.
    #[serde(default, deserialize_with = "present")]
    pub rotation_tolerance_deg: Option<Option<f64>>,
    /// Omitted preserves the saved policy.
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
    /// Omitted preserves the saved choice.
    #[serde(default)]
    pub dark_choice: Option<DarkChoice>,
    /// Days, zero for no limit. Omitted preserves the saved value; `null`
    /// restores the default.
    #[serde(default, deserialize_with = "present")]
    pub flat_max_age_days: Option<Option<f64>>,
    /// Omitted preserves the existing setting.
    #[serde(default)]
    pub exclude_uncalibrated: Option<bool>,
    /// Any setting of [`rules::fields`], by key. A key left out keeps its
    /// value; `null` restores the default.
    #[serde(default)]
    pub rules: Option<serde_json::Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateRigSettingsRequest {
    /// The rig's name as the panel showed it, kept to name the rig when no
    /// open catalog holds it.
    #[serde(default)]
    pub name: Option<String>,
    /// Every value the rig sets for itself, replacing what it set before.
    /// A key left out, or `null`, inherits.
    pub rules: serde_json::Map<String, Value>,
}

/// A field that was sent, `null` included, as opposed to one left out.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// The longest rig name kept.
const MAX_RIG_NAME: usize = 200;

/// A stored value as the panel should see it: only what this build can read
/// and would apply.
fn readable_values(values: &RuleValues) -> RuleValues {
    values
        .iter()
        .filter(|(key, value)| {
            rules::field(key).is_some_and(|spec| rules::validate(spec, value).is_ok())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn current_response(
    settings: Option<&CalibrationSettings>,
    rigs: Vec<RigSettingsView>,
) -> CalibrationSettingsResponse {
    let values = settings
        .map(|settings| readable_values(&settings.rules))
        .unwrap_or_default();
    let number = |key: &str| values.get(key).and_then(Value::as_f64);
    let resolved = rules::RuleBook::new(values.clone(), BTreeMap::new()).global();
    CalibrationSettingsResponse {
        rotation_tolerance_deg: number("rotation_tolerance_deg"),
        default_rotation_tolerance_deg: seiza_calibration::MatchTolerances::default().rotation_deg,
        external_masters: resolved.external_masters,
        flat_star_masking: settings
            .and_then(|settings| settings.flat_star_masking)
            .unwrap_or(false),
        dark_reach_days: number("dark_reach_days"),
        default_dark_reach_days: crate::calibration::DEFAULT_DARK_REACH_DAYS,
        complete_dark_frames: values
            .get("complete_dark_frames")
            .and_then(Value::as_u64)
            .map(|frames| frames as usize),
        default_complete_dark_frames: crate::calibration::DEFAULT_COMPLETE_DARK_FRAMES,
        dark_choice: resolved.dark_choice,
        flat_max_age_days: number("flat_max_age_days"),
        default_flat_max_age_days: crate::calibration::DEFAULT_FLAT_MAX_AGE_DAYS,
        exclude_uncalibrated: settings
            .and_then(|settings| settings.exclude_uncalibrated)
            .unwrap_or(false),
        fields: rules::fields(),
        rules: values,
        rigs,
    }
}

/// Check one value against its setting; `null` passes as "unset".
fn checked(key: &str, value: &Value) -> Result<(), AppError> {
    let spec = rules::field(key)
        .ok_or_else(|| AppError::BadRequest(format!("unknown calibration setting `{key}`")))?;
    if value.is_null() {
        return Ok(());
    }
    rules::validate(spec, value).map_err(AppError::BadRequest)?;
    rules::stacker_accepts(key, value).map_err(AppError::BadRequest)
}

/// The global edits a request makes, key to new value (`None` clears),
/// every one checked.
fn global_edits(
    request: &UpdateCalibrationSettingsRequest,
) -> Result<Vec<(String, Option<Value>)>, AppError> {
    let mut edits: Vec<(String, Option<Value>)> = Vec::new();
    let mut set = |key: &str, value: Option<Value>| edits.push((key.to_string(), value));
    if let Some(sent) = request.rotation_tolerance_deg {
        set("rotation_tolerance_deg", sent.map(Value::from));
    }
    if let Some(policy) = request.external_masters {
        set("external_masters", Some(Value::from(policy.as_str())));
    }
    if let Some(sent) = request.dark_reach_days {
        set("dark_reach_days", sent.map(Value::from));
    }
    if let Some(sent) = request.complete_dark_frames {
        set("complete_dark_frames", sent.map(Value::from));
    }
    if let Some(choice) = request.dark_choice {
        set(
            "dark_choice",
            Some(serde_json::to_value(choice).expect("a choice serializes")),
        );
    }
    if let Some(sent) = request.flat_max_age_days {
        set("flat_max_age_days", sent.map(Value::from));
    }
    if let Some(values) = &request.rules {
        for (key, value) in values {
            set(key, (!value.is_null()).then(|| value.clone()));
        }
    }
    for (key, value) in &edits {
        checked(key, value.as_ref().unwrap_or(&Value::Null))?;
    }
    Ok(edits)
}

/// A default is not written down, so a registry that only ever held
/// defaults stays clean and older builds see nothing new.
fn apply_edits(values: &mut RuleValues, edits: Vec<(String, Option<Value>)>) {
    for (key, value) in edits {
        let is_default = value
            .as_ref()
            .zip(rules::field(&key))
            .is_some_and(|(value, spec)| rules::equivalent(value, &spec.default));
        match value {
            Some(value) if !is_default => {
                values.insert(key, value);
            }
            _ => {
                values.remove(&key);
            }
        }
    }
}

/// Keep `calibration` only when it holds anything.
fn tidy(settings: CalibrationSettings) -> Option<CalibrationSettings> {
    (settings != CalibrationSettings::default()).then_some(settings)
}

/// The registry's calibration block as it stands, or none.
fn load_settings(state: &AppState) -> Result<Option<CalibrationSettings>, AppError> {
    // A server without a persistent registry still has a current value: the
    // default. The panel shows it read-only rather than erroring.
    match require_registry_path(state) {
        Ok(path) => Ok(DbRegistry::load_or_init(&path)
            .map_err(|error| AppError::InternalError(error.to_string()))?
            .calibration),
        Err(_) => Ok(None),
    }
}

/// Every rig the open catalogs record, merged by UUID, with every rig that
/// has values of its own; rigs with values first, then by name.
async fn rig_views(
    state: &Arc<AppState>,
    settings: Option<&CalibrationSettings>,
) -> Vec<RigSettingsView> {
    let catalogs: Vec<(String, String)> = state
        .all_databases()
        .iter()
        .map(|ctx| (ctx.name.clone(), ctx.database_path.clone()))
        .collect();
    let listed = tokio::task::spawn_blocking(move || {
        catalogs
            .into_iter()
            .filter_map(|(name, path)| {
                let conn = crate::server::database_context::open_scheduler_connection_with_flags(
                    &path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
                )
                .ok()?;
                match crate::calibration::rig_listing(&conn) {
                    Ok(rigs) => Some((name, rigs)),
                    Err(error) => {
                        tracing::debug!("listing rigs in {path}: {error:#}");
                        None
                    }
                }
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    let mut views: BTreeMap<String, RigSettingsView> = BTreeMap::new();
    for (database, rigs) in listed {
        for rig in rigs {
            let view = views
                .entry(rig.rig_uuid.clone())
                .or_insert_with(|| RigSettingsView {
                    rig_uuid: rig.rig_uuid.clone(),
                    name: rig.name.clone(),
                    detail: rig_detail(&rig),
                    databases: Vec::new(),
                    frames: 0,
                    rules: RuleValues::new(),
                });
            view.frames += rig.frames;
            if !view.databases.contains(&database) {
                view.databases.push(database.clone());
            }
        }
    }
    for (uuid, rig) in settings
        .map(|settings| &settings.rigs)
        .into_iter()
        .flatten()
    {
        let view = views
            .entry(uuid.clone())
            .or_insert_with(|| RigSettingsView {
                rig_uuid: uuid.clone(),
                name: rig.name.clone().unwrap_or_else(|| "Unnamed rig".into()),
                detail: None,
                databases: Vec::new(),
                frames: 0,
                rules: RuleValues::new(),
            });
        view.rules = readable_values(&rig.rules);
    }
    let mut views: Vec<RigSettingsView> = views.into_values().collect();
    views.sort_by(|left, right| {
        left.rules
            .is_empty()
            .cmp(&right.rules.is_empty())
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.rig_uuid.cmp(&right.rig_uuid))
    });
    views
}

fn rig_detail(rig: &crate::calibration::RigListing) -> Option<String> {
    let size = rig
        .width
        .zip(rig.height)
        .filter(|(width, height)| *width > 0 && *height > 0)
        .map(|(width, height)| format!("{width}×{height}"));
    let binning = rig
        .binning_x
        .map(|x| format!("bin {x}×{}", rig.binning_y.unwrap_or(x)));
    let parts: Vec<String> = [size, binning].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

async fn respond(
    state: &Arc<AppState>,
    settings: Option<CalibrationSettings>,
) -> Json<ApiResponse<CalibrationSettingsResponse>> {
    let rigs = rig_views(state, settings.as_ref()).await;
    Json(ApiResponse::success(current_response(
        settings.as_ref(),
        rigs,
    )))
}

/// The most rigs that may have values of their own. Far more than any
/// observatory has, and a bound on what one editor can make the registry
/// hold.
const MAX_RIGS: usize = 256;

/// Writes of the calibration block, numbered in the order they were made
/// under the registry's lock.
static WRITTEN: AtomicU64 = AtomicU64::new(0);
/// The newest write put in force.
static APPLIED: Mutex<u64> = Mutex::new(0);

/// Change the calibration block, save the registry, and only once it is
/// saved put the settings in force. A save that fails changes nothing in
/// force. Writes go into force in the order they were saved: a write that
/// lost the race to a newer one is already part of it, so it is skipped.
async fn save_and_apply(
    state: &Arc<AppState>,
    edit: impl FnOnce(&mut CalibrationSettings) -> Result<(), AppError>,
) -> Result<Option<CalibrationSettings>, AppError> {
    let (written, calibration) = update_registry(state, |registry| {
        let mut settings = registry.calibration.clone().unwrap_or_default();
        edit(&mut settings)?;
        registry.calibration = tidy(settings);
        Ok((
            WRITTEN.fetch_add(1, Ordering::SeqCst) + 1,
            registry.calibration.clone(),
        ))
    })
    .await?;
    let mut applied = APPLIED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if written > *applied {
        apply(state, calibration.as_ref());
        *applied = written;
    }
    Ok(calibration)
}

/// Put the saved settings in force and judge what they decide again: grades
/// when a setting grading reads changed (the health pass follows the
/// grading), else only the lights' health. No pixels are read for either.
fn apply(state: &Arc<AppState>, settings: Option<&CalibrationSettings>) {
    let before = crate::calibration::rule_book();
    crate::calibration::configure(settings);
    let after = crate::calibration::rule_book();
    if before.grades_differ(&after) {
        crate::server::calibration_quality::request_all(state);
    } else {
        crate::server::calibration_health::request_all(state);
    }
    // Grouping regroups from the stored measurements as soon as it is read;
    // only a rig whose new grouping asks for something not measured yet
    // (switched from time to measured) has pixels read, by the flat job.
    crate::server::flat_measure::settings_changed(state, &before, &after);
}

/// GET /api/settings/calibration
pub async fn get_calibration_settings(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<CalibrationSettingsResponse>>, AppError> {
    let settings = load_settings(&state)?;
    Ok(respond(&state, settings).await)
}

/// PUT /api/settings/calibration
pub async fn update_calibration_settings(
    State(state): State<Arc<AppState>>,
    Json(request): Json<UpdateCalibrationSettingsRequest>,
) -> Result<Json<ApiResponse<CalibrationSettingsResponse>>, AppError> {
    let edits = global_edits(&request)?;
    let calibration = save_and_apply(&state, |settings| {
        apply_edits(&mut settings.rules, edits);
        if let Some(masking) = request.flat_star_masking {
            settings.flat_star_masking = masking.then_some(true);
        }
        if let Some(exclude) = request.exclude_uncalibrated {
            settings.exclude_uncalibrated = exclude.then_some(true);
        }
        Ok(())
    })
    .await?;
    Ok(respond(&state, calibration).await)
}

/// The registry's key for a rig: its UUID, as the library writes it.
fn rig_key(rig_uuid: &str) -> Result<String, AppError> {
    uuid::Uuid::parse_str(rig_uuid.trim())
        .map(|uuid| uuid.hyphenated().to_string())
        .map_err(|_| AppError::BadRequest(format!("`{rig_uuid}` is not a rig's UUID")))
}

/// PUT /api/settings/calibration/rigs/{rig_uuid}
pub async fn update_rig_calibration_settings(
    State(state): State<Arc<AppState>>,
    Path(rig_uuid): Path<String>,
    Json(request): Json<UpdateRigSettingsRequest>,
) -> Result<Json<ApiResponse<CalibrationSettingsResponse>>, AppError> {
    let key = rig_key(&rig_uuid)?;
    let name = request
        .name
        .as_deref()
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
    let mut values = RuleValues::new();
    for (field, value) in &request.rules {
        checked(field, value)?;
        if !value.is_null() {
            values.insert(field.clone(), value.clone());
        }
    }
    let calibration = save_and_apply(&state, |settings| {
        if values.is_empty() {
            settings.rigs.remove(&key);
        } else {
            if !settings.rigs.contains_key(&key) && settings.rigs.len() >= MAX_RIGS {
                return Err(AppError::BadRequest(format!(
                    "at most {MAX_RIGS} rigs can have settings of their own; let one \
                     inherit everything first"
                )));
            }
            settings.rigs.insert(
                key.clone(),
                RigCalibrationSettings {
                    name: name.clone(),
                    rules: values.clone(),
                },
            );
        }
        Ok(())
    })
    .await?;
    Ok(respond(&state, calibration).await)
}

/// DELETE /api/settings/calibration/rigs/{rig_uuid}: the rig inherits every
/// value again.
pub async fn delete_rig_calibration_settings(
    State(state): State<Arc<AppState>>,
    Path(rig_uuid): Path<String>,
) -> Result<Json<ApiResponse<CalibrationSettingsResponse>>, AppError> {
    let key = rig_key(&rig_uuid)?;
    let calibration = save_and_apply(&state, |settings| {
        settings.rigs.remove(&key);
        Ok(())
    })
    .await?;
    Ok(respond(&state, calibration).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        let response = current_response(None, Vec::new());
        assert_eq!(response.dark_reach_days, None);
        assert_eq!(response.default_dark_reach_days, 183.0);
        assert_eq!(response.default_complete_dark_frames, 10);
        assert_eq!(response.default_flat_max_age_days, 60.0);
        let lifted: UpdateCalibrationSettingsRequest =
            serde_json::from_str(r#"{"rotation_tolerance_deg":null,"flat_max_age_days":0}"#)
                .unwrap();
        assert_eq!(lifted.flat_max_age_days, Some(Some(0.0)));
        assert!(!response.exclude_uncalibrated, "off unless turned on");
        assert_eq!(lifted.exclude_uncalibrated, None);
        assert_eq!(response.dark_choice, DarkChoice::Complete);
        assert_eq!(lifted.dark_choice, None, "left out keeps the saved choice");
        let nearest: UpdateCalibrationSettingsRequest =
            serde_json::from_str(r#"{"rotation_tolerance_deg":null,"dark_choice":"nearest"}"#)
                .unwrap();
        assert_eq!(nearest.dark_choice, Some(DarkChoice::Nearest));
    }

    #[test]
    fn a_left_out_rotation_keeps_its_value_and_null_clears_it() {
        let left_out: UpdateCalibrationSettingsRequest =
            serde_json::from_str(r#"{"flat_star_masking":true}"#).unwrap();
        assert_eq!(left_out.rotation_tolerance_deg, None);
        assert!(global_edits(&left_out).unwrap().is_empty());
        let cleared: UpdateCalibrationSettingsRequest =
            serde_json::from_str(r#"{"rotation_tolerance_deg":null}"#).unwrap();
        assert_eq!(
            global_edits(&cleared).unwrap(),
            vec![("rotation_tolerance_deg".to_string(), None)]
        );
    }

    #[test]
    fn flat_star_masking_defaults_off_and_settings_keep_it() {
        assert!(!current_response(None, Vec::new()).flat_star_masking);
        let settings = CalibrationSettings {
            flat_star_masking: Some(true),
            ..Default::default()
        };
        assert!(current_response(Some(&settings), Vec::new()).flat_star_masking);
    }

    #[test]
    fn an_unknown_key_or_value_is_refused_with_its_name() {
        assert!(
            serde_json::from_str::<UpdateCalibrationSettingsRequest>(r#"{"rotation":2}"#).is_err(),
            "unknown top-level keys are refused"
        );
        let unknown: UpdateCalibrationSettingsRequest =
            serde_json::from_value(json!({"rules": {"dark_temperatur_c": 2}})).unwrap();
        match global_edits(&unknown) {
            Err(AppError::BadRequest(message)) => assert!(message.contains("dark_temperatur_c")),
            other => panic!("expected a refusal, got {other:?}"),
        }
        for (key, value) in [
            ("dark_temperature_tolerance_c", json!(25)),
            ("dark_temperature_tolerance_c", json!("2")),
            ("thin_master_frames", json!(1)),
            ("thin_master_frames", json!(3.5)),
            ("check_no_flat", json!("poor")),
            ("use_darks", json!("no")),
            ("external_masters", json!("sometimes")),
            ("grade_flat_dim", json!({"suspect": 0.01, "poor": 0.05})),
            ("grade_flat_illumination", json!(0.05)),
        ] {
            let request: UpdateCalibrationSettingsRequest =
                serde_json::from_value(json!({ "rules": { key: value } })).unwrap();
            assert!(
                matches!(global_edits(&request), Err(AppError::BadRequest(_))),
                "{key} = {value} is refused"
            );
        }
        let legacy: UpdateCalibrationSettingsRequest =
            serde_json::from_value(json!({"rotation_tolerance_deg": 200})).unwrap();
        assert!(matches!(
            global_edits(&legacy),
            Err(AppError::BadRequest(_))
        ));
        let good: UpdateCalibrationSettingsRequest = serde_json::from_value(json!({
            "rules": {
                "dark_temperature_tolerance_c": 2,
                "check_no_flat": "advisory",
                "grade_flat_dim": {"suspect": 0.05, "poor": null},
                "thin_master_frames": null,
            }
        }))
        .unwrap();
        assert_eq!(global_edits(&good).unwrap().len(), 4);
    }

    #[test]
    fn a_default_is_not_written_down() {
        let mut values = RuleValues::new();
        apply_edits(
            &mut values,
            vec![
                ("dark_reach_days".into(), Some(json!(90))),
                ("rotation_tolerance_deg".into(), Some(json!(2.0))),
                ("check_no_flat".into(), Some(json!("flag"))),
            ],
        );
        assert_eq!(values.keys().collect::<Vec<_>>(), vec!["dark_reach_days"]);
        apply_edits(&mut values, vec![("dark_reach_days".into(), None)]);
        assert!(values.is_empty());
        assert_eq!(tidy(CalibrationSettings::default()), None);
    }

    #[test]
    fn a_rig_key_must_be_a_uuid() {
        assert_eq!(
            rig_key("5A1E0000-0000-5000-8000-000000000001").unwrap(),
            "5a1e0000-0000-5000-8000-000000000001"
        );
        assert!(rig_key("../../etc").is_err());
        let request: Result<UpdateRigSettingsRequest, _> =
            serde_json::from_value(json!({"rules": {}, "colour": "red"}));
        assert!(request.is_err(), "unknown keys are refused");
    }

    #[test]
    fn the_response_lists_only_values_this_build_reads() {
        let settings = CalibrationSettings {
            rules: [
                ("dark_choice".to_string(), json!("by_gain")),
                ("dark_reach_days".to_string(), json!(90)),
                ("from_the_future".to_string(), json!(true)),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let response = current_response(Some(&settings), Vec::new());
        assert_eq!(
            response.rules.keys().collect::<Vec<_>>(),
            vec!["dark_reach_days"]
        );
        assert_eq!(response.dark_reach_days, Some(90.0));
        assert_eq!(response.dark_choice, DarkChoice::Complete);
        assert!(response.fields.len() > 30);
    }
}
