//! Settings › Review › Star detection: how the quality scan detects stars
//! on each rig's frames. One process-global block in the database registry
//! ([`crate::db_registry::DbRegistry::star_detection`]), shared by every
//! database and both serving modes: a default row, each rig's own, and each
//! N.I.N.A. profile's manual row and recorded settings.
//!
//! Profile settings arrive two ways: an editor imports a `.profile` file
//! (`POST …/profiles/import`), or the PSF Guard plugin sends a snapshot as
//! it captures (`POST /sync/v1/star-detection/profile`, authenticated by
//! the catalog's paired client token like the rest of `/sync/v1`). A
//! snapshot equal to the record already in effect, from the same catalog,
//! stores nothing.
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
use crate::star_detection::nina_profile::{self, ProfileSettings};
use crate::star_detection::{
    profile_key, HocusFocusKnobs, HocusFocusVersion, ProfileRecord, ProfileStarDetection,
    RecordSource, RigStarDetection, RigStarDetectionEntry, StarDetectionSettings,
};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StarDetectionSettingsResponse {
    /// What a rig without a row of its own uses.
    pub default: RigStarDetection,
    /// N.I.N.A.'s own defaults, which Reset goes back to.
    pub builtin: RigStarDetection,
    /// Every rig an open catalog records, and every rig with a row of its
    /// own, by name.
    pub rigs: Vec<StarDetectionRigView>,
    /// Every N.I.N.A. profile an open catalog's frames record, and every
    /// profile with settings of its own; profiles with settings first.
    pub profiles: Vec<ProfileView>,
}

/// One N.I.N.A. profile: its manual row and recorded settings.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileView {
    pub profile_id: String,
    /// The name its newest record or import gave; absent until one does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The databases whose frames record it, by name.
    pub databases: Vec<String>,
    /// Frames those databases hold from it.
    pub frames: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manual: Option<RigStarDetection>,
    /// Its records, newest start first.
    pub records: Vec<RecordView>,
}

/// One recorded snapshot, with what Hocus Focus makes of it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordView {
    #[serde(flatten)]
    pub record: ProfileRecord,
    /// `simple`, `optimized` or `advanced`: how Hocus Focus derives its
    /// values.
    pub hocus_focus_mode: &'static str,
    /// The Hocus Focus values the record gives.
    pub hocus_focus: HocusFocusKnobs,
    /// Stored options PSF Guard cannot reproduce.
    pub notes: Vec<String>,
}

impl RecordView {
    fn of(record: &ProfileRecord) -> Self {
        let (knobs, mut notes) = HocusFocusKnobs::from_profile(&record.settings);
        let brightest = record.settings.autofocus_brightest_stars;
        if brightest > 0
            && record.settings.star_detector
                != crate::star_detection::StarDetectorSelection::HocusFocus
        {
            notes.push(format!(
                "Not reproduced: HFR from the {brightest} brightest stars with N.I.N.A.'s \
                 detector; HFR can differ from N.I.N.A.'s"
            ));
        }
        let flag =
            |key: &str| record.settings.hocus_focus.get(key) == Some(&serde_json::json!(true));
        Self {
            record: record.clone(),
            hocus_focus_mode: if flag("UseAdvanced") {
                "advanced"
            } else if flag("UseOptimizedSettings") {
                "optimized"
            } else {
                "simple"
            },
            hocus_focus: knobs,
            notes,
        }
    }
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

/// The N.I.N.A. profiles each open catalog's frames record, as (database
/// name, profile id, frames). A catalog that cannot be read lists none.
async fn catalog_profiles(state: &Arc<AppState>) -> Vec<(String, String, usize)> {
    let catalogs: Vec<(String, String)> = state
        .all_databases()
        .iter()
        .map(|ctx| (ctx.name.clone(), ctx.database_path.clone()))
        .collect();
    tokio::task::spawn_blocking(move || {
        let mut listed = Vec::new();
        for (name, path) in catalogs {
            let Ok(conn) = crate::server::database_context::open_scheduler_connection_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            ) else {
                continue;
            };
            let rows = conn
                .prepare(
                    "SELECT profileId, COUNT(*) FROM acquiredimage \
                     WHERE profileId IS NOT NULL AND profileId <> '' GROUP BY profileId",
                )
                .and_then(|mut statement| {
                    statement
                        .query_map([], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()
                });
            match rows {
                Ok(rows) => listed.extend(rows.into_iter().filter_map(|(id, frames)| {
                    Some((name.clone(), profile_key(&id)?, frames.max(0) as usize))
                })),
                Err(error) => tracing::debug!("listing profiles in {path}: {error:#}"),
            }
        }
        listed
    })
    .await
    .unwrap_or_default()
}

/// Every profile the catalogs record, merged by id, with every profile that
/// has settings of its own.
fn merge_profile_views(
    listed: Vec<(String, String, usize)>,
    settings: &StarDetectionSettings,
) -> Vec<ProfileView> {
    let mut views: BTreeMap<String, ProfileView> = BTreeMap::new();
    let empty = |profile_id: &str| ProfileView {
        profile_id: profile_id.to_string(),
        name: None,
        databases: Vec::new(),
        frames: 0,
        manual: None,
        records: Vec::new(),
    };
    for (database, profile_id, frames) in listed {
        let view = views
            .entry(profile_id.clone())
            .or_insert_with(|| empty(&profile_id));
        view.frames += frames;
        if !view.databases.contains(&database) {
            view.databases.push(database);
        }
    }
    for (profile_id, profile) in &settings.profiles {
        let view = views
            .entry(profile_id.clone())
            .or_insert_with(|| empty(profile_id));
        view.name = profile.name.clone();
        view.manual = profile.manual;
        let mut records: Vec<&ProfileRecord> = profile.records.iter().collect();
        records.sort_by(|left, right| {
            (right.effective_from, right.received_at).cmp(&(left.effective_from, left.received_at))
        });
        view.records = records.into_iter().map(RecordView::of).collect();
    }
    let mut views: Vec<ProfileView> = views.into_values().collect();
    views.sort_by(|left, right| {
        let bare = |view: &ProfileView| view.manual.is_none() && view.records.is_empty();
        bare(left)
            .cmp(&bare(right))
            .then_with(|| right.frames.cmp(&left.frames))
            .then_with(|| left.profile_id.cmp(&right.profile_id))
    });
    views
}

async fn respond(
    state: &Arc<AppState>,
    settings: StarDetectionSettings,
) -> Json<ApiResponse<StarDetectionSettingsResponse>> {
    let rigs = merge_rig_views(catalog_rigs(state).await, &settings);
    let profiles = merge_profile_views(catalog_profiles(state).await, &settings);
    Json(ApiResponse::success(StarDetectionSettingsResponse {
        default: settings.default,
        builtin: RigStarDetection::default(),
        rigs,
        profiles,
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

/// The most profiles with settings of their own.
const MAX_PROFILES: usize = 256;
/// The most records one profile keeps. A plugin stores one only when the
/// settings change, so this is years of changes.
const MAX_RECORDS: usize = 200;

/// The registry's key for a profile, refusing an id that is not a GUID.
fn profile_id_key(profile_id: &str) -> Result<String, AppError> {
    uuid::Uuid::parse_str(profile_id.trim())
        .map(|id| id.hyphenated().to_string())
        .map_err(|_| AppError::BadRequest(format!("`{profile_id}` is not a N.I.N.A. profile id")))
}

/// The profile's entry, made when it has none, within [`MAX_PROFILES`].
fn profile_entry<'a>(
    settings: &'a mut StarDetectionSettings,
    key: &str,
) -> Result<&'a mut ProfileStarDetection, AppError> {
    if !settings.profiles.contains_key(key) && settings.profiles.len() >= MAX_PROFILES {
        return Err(AppError::BadRequest(format!(
            "at most {MAX_PROFILES} profiles can have star detection settings"
        )));
    }
    Ok(settings.profiles.entry(key.to_string()).or_default())
}

/// Drop a profile's entry once it holds nothing.
fn tidy_profile(settings: &mut StarDetectionSettings, key: &str) {
    if settings
        .profiles
        .get(key)
        .is_some_and(|profile| profile.manual.is_none() && profile.records.is_empty())
    {
        settings.profiles.remove(key);
    }
}

/// Store a snapshot as a new record, unless the record in effect at its
/// start already holds the same settings. Returns whether it was stored.
fn add_record(
    settings: &mut StarDetectionSettings,
    key: &str,
    name: Option<String>,
    record: ProfileRecord,
) -> Result<bool, AppError> {
    let captured = record.effective_from.map(|from| from.timestamp());
    let profile = profile_entry(settings, key)?;
    if name.is_some() {
        profile.name = name;
    }
    // A snapshot from another catalog is stored even with the same
    // settings: it shows which catalog the profile now pairs with, and
    // Director plans each catalog for that profile.
    let same = |current: &ProfileRecord| {
        current.settings == record.settings
            && current.hocus_focus_version == record.hocus_focus_version
            && current.catalog_id == record.catalog_id
    };
    // A dated snapshot repeats the record in effect at its date; one for
    // every frame repeats the newest record for every frame.
    let unchanged = match record.effective_from {
        Some(_) => profile.record_at(captured).is_some_and(same),
        None => profile
            .records
            .iter()
            .filter(|current| current.effective_from.is_none())
            .max_by_key(|current| current.received_at)
            .is_some_and(same),
    };
    if unchanged {
        return Ok(false);
    }
    if profile.records.len() >= MAX_RECORDS {
        return Err(AppError::BadRequest(format!(
            "this profile already has {MAX_RECORDS} records; delete old ones first"
        )));
    }
    profile.records.push(record);
    Ok(true)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateProfileManualRequest {
    #[serde(default)]
    pub name: Option<String>,
    pub settings: StrictRigStarDetection,
}

/// PUT /api/settings/star-detection/profiles/{profile_id}/manual: settings
/// that beat the profile's records.
pub async fn update_profile_manual(
    State(state): State<Arc<AppState>>,
    Path(profile_id): Path<String>,
    Json(request): Json<UpdateProfileManualRequest>,
) -> Result<Json<ApiResponse<StarDetectionSettingsResponse>>, AppError> {
    let key = profile_id_key(&profile_id)?;
    let name = rig_name(request.name.as_deref())?;
    let row: RigStarDetection = request.settings.into();
    let settings = save_and_apply(&state, |settings| {
        let profile = profile_entry(settings, &key)?;
        profile.manual = Some(row);
        if name.is_some() {
            profile.name = name;
        }
        Ok(())
    })
    .await?;
    Ok(respond(&state, settings).await)
}

/// DELETE /api/settings/star-detection/profiles/{profile_id}/manual: the
/// profile's records apply again.
pub async fn delete_profile_manual(
    State(state): State<Arc<AppState>>,
    Path(profile_id): Path<String>,
) -> Result<Json<ApiResponse<StarDetectionSettingsResponse>>, AppError> {
    let key = profile_id_key(&profile_id)?;
    let settings = save_and_apply(&state, |settings| {
        if let Some(profile) = settings.profiles.get_mut(&key) {
            profile.manual = None;
        }
        tidy_profile(settings, &key);
        Ok(())
    })
    .await?;
    Ok(respond(&state, settings).await)
}

/// DELETE /api/settings/star-detection/profiles/{profile_id}/records/{record_id}
pub async fn delete_profile_record(
    State(state): State<Arc<AppState>>,
    Path((profile_id, record_id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<StarDetectionSettingsResponse>>, AppError> {
    let key = profile_id_key(&profile_id)?;
    let settings = save_and_apply(&state, |settings| {
        let removed = settings.profiles.get_mut(&key).is_some_and(|profile| {
            let before = profile.records.len();
            profile.records.retain(|record| record.id != record_id);
            profile.records.len() != before
        });
        if !removed {
            return Err(AppError::NotFound);
        }
        tidy_profile(settings, &key);
        Ok(())
    })
    .await?;
    Ok(respond(&state, settings).await)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportProfileQuery {
    /// From when the settings apply (RFC 3339); absent for every frame.
    #[serde(default)]
    pub effective_from: Option<chrono::DateTime<chrono::Utc>>,
    /// The Hocus Focus version the profile ran; 4 when left out.
    #[serde(default)]
    pub hocus_focus_version: Option<HocusFocusVersion>,
    /// Read the file and show what it holds, without storing it.
    #[serde(default)]
    pub preview: bool,
}

#[derive(Debug, Serialize)]
pub struct ImportProfileResponse {
    pub profile_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The record the import makes, or made.
    pub record: RecordView,
    /// Profile settings PSF Guard read but cannot reproduce.
    pub warnings: Vec<String>,
    /// False for a preview, and when the profile already had the same
    /// settings in effect from that date.
    pub stored: bool,
    /// The settings after the import, for the panel.
    pub settings: StarDetectionSettingsResponse,
}

/// POST /api/settings/star-detection/profiles/import: read a N.I.N.A.
/// `.profile` file sent as the body, and store its settings as a record.
pub async fn import_profile(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<ImportProfileQuery>,
    body: axum::body::Bytes,
) -> Result<Json<ApiResponse<ImportProfileResponse>>, AppError> {
    let parsed = nina_profile::parse_profile(&body).map_err(AppError::BadRequest)?;
    let record = ProfileRecord {
        id: uuid::Uuid::new_v4().to_string(),
        effective_from: query.effective_from,
        source: RecordSource::Import,
        received_at: chrono::Utc::now(),
        catalog_id: None,
        nina_version: None,
        hocus_focus_version: query.hocus_focus_version.unwrap_or_default(),
        settings: parsed.settings.clone(),
    };
    let view = RecordView::of(&record);
    let (stored, settings) = if query.preview {
        (false, load_settings(&state)?)
    } else {
        let mut stored = false;
        let settings = save_and_apply(&state, |settings| {
            stored = add_record(settings, &parsed.profile_id, parsed.name.clone(), record)?;
            Ok(())
        })
        .await?;
        (stored, settings)
    };
    let Json(response) = respond(&state, settings).await;
    Ok(Json(ApiResponse::success(ImportProfileResponse {
        profile_id: parsed.profile_id,
        name: parsed.name,
        record: view,
        warnings: parsed.warnings,
        stored,
        settings: response.data.expect("respond always succeeds"),
    })))
}

/// The largest plugin snapshot accepted.
pub const MAX_PLUGIN_BODY_BYTES: usize = 64 * 1024;

/// What a PSF Guard N.I.N.A. plugin sends as it captures with a profile.
/// The Sync plugin sends its catalog's id; the Director plugin's comes from its
/// rig's pairing, so it sends none.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginProfileRequest {
    pub protocol_version: u32,
    /// The registry id of the catalog the Sync plugin is paired with.
    #[serde(default)]
    pub catalog_id: Option<String>,
    pub profile_id: String,
    #[serde(default)]
    pub profile_name: Option<String>,
    /// When the plugin read these settings.
    pub captured_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub nina_version: Option<String>,
    /// Hocus Focus's version, such as `4.0.0.21`; absent when it is not
    /// installed.
    #[serde(default)]
    pub hocus_focus_version: Option<String>,
    #[serde(deserialize_with = "nina_profile::strict_settings")]
    pub settings: ProfileSettings,
}

#[derive(Debug, Serialize)]
pub struct PluginProfileResponse {
    pub profile_id: String,
    /// False when the profile already had these settings in effect, from
    /// either plugin or an import.
    pub stored: bool,
}

/// A plugin body that does not parse, answered 400 like a snapshot that
/// fails validation, so both plugins hold it until it changes rather than
/// retrying axum's 422.
pub(crate) fn plugin_body(
    body: Result<Json<PluginProfileRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<PluginProfileRequest, AppError> {
    body.map(|Json(request)| request)
        .map_err(|rejection| AppError::BadRequest(rejection.body_text()))
}

/// The Hocus Focus version a plugin reports, by its major number.
fn reported_hocus_focus_version(version: Option<&str>) -> Result<HocusFocusVersion, AppError> {
    let Some(version) = version.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(HocusFocusVersion::default());
    };
    match version.trim_start_matches(['v', 'V']).split('.').next() {
        Some("3") | Some("2") | Some("1") => Ok(HocusFocusVersion::V3),
        Some(major) if major.parse::<u32>().is_ok_and(|major| major >= 4) => {
            Ok(HocusFocusVersion::V4)
        }
        _ => Err(AppError::BadRequest(format!(
            "`{version}` is not a Hocus Focus version"
        ))),
    }
}

/// Check a plugin's snapshot and store it from `captured_at` on, unless
/// the record in effect then already holds the same settings, whichever
/// source recorded them. Returns the profile key and whether it stored.
pub(crate) async fn store_plugin_snapshot(
    state: &Arc<AppState>,
    source: RecordSource,
    catalog_id: String,
    request: PluginProfileRequest,
) -> Result<PluginProfileResponse, AppError> {
    if request.protocol_version != 1 {
        return Err(AppError::BadRequest("protocol_version must be 1".into()));
    }
    let key = profile_id_key(&request.profile_id)?;
    let name = rig_name(request.profile_name.as_deref())?;
    for text in [&request.nina_version, &request.hocus_focus_version]
        .into_iter()
        .flatten()
    {
        if text.chars().count() > 64 {
            return Err(AppError::BadRequest(
                "a version is at most 64 characters".into(),
            ));
        }
    }
    request.settings.validate().map_err(AppError::BadRequest)?;
    let record = ProfileRecord {
        id: uuid::Uuid::new_v4().to_string(),
        effective_from: Some(request.captured_at),
        source,
        received_at: chrono::Utc::now(),
        catalog_id: Some(catalog_id),
        nina_version: request.nina_version,
        hocus_focus_version: reported_hocus_focus_version(request.hocus_focus_version.as_deref())?,
        settings: request.settings,
    };
    // Most captures repeat the settings in force: answer those without
    // writing the registry.
    let mut probe = (*crate::star_detection::current()).clone();
    if !add_record(&mut probe, &key, None, record.clone())? {
        return Ok(PluginProfileResponse {
            profile_id: key,
            stored: false,
        });
    }
    let mut stored = false;
    save_and_apply(state, |settings| {
        stored = add_record(settings, &key, name, record)?;
        Ok(())
    })
    .await?;
    Ok(PluginProfileResponse {
        profile_id: key,
        stored,
    })
}

/// POST /api/sync/v1/star-detection/profile: the Sync plugin's snapshot,
/// authenticated by its catalog's paired client token.
pub async fn plugin_profile(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: Result<Json<PluginProfileRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<ApiResponse<PluginProfileResponse>>, AppError> {
    use crate::server::remote_audit::{AuditAction, AuditOutcome, AuditRecord};
    let ctx = crate::server::remote_sync::authenticated_catalog(
        &state,
        &headers,
        AuditAction::StarDetectionProfile,
    )?;
    let catalog_id = ctx.id.clone();
    let result = async {
        let request = plugin_body(body)?;
        let requested = request.catalog_id.as_deref().ok_or_else(|| {
            AppError::BadRequest("catalog_id is required: send the paired catalog's id".into())
        })?;
        crate::server::remote_sync::require_catalog(&ctx, requested)?;
        store_plugin_snapshot(
            &state,
            RecordSource::SyncPlugin,
            catalog_id.clone(),
            request,
        )
        .await
    }
    .await;
    let (outcome, detail) = match &result {
        Ok(response) if response.stored => (AuditOutcome::Ok, "stored"),
        Ok(_) => (AuditOutcome::Ok, "unchanged"),
        Err(AppError::BadRequest(_) | AppError::Forbidden(_)) => (AuditOutcome::Refused, "refused"),
        Err(_) => (AuditOutcome::Failed, "failed"),
    };
    state.remote_audit.record(
        &catalog_id,
        AuditAction::StarDetectionProfile,
        outcome,
        AuditRecord {
            operation: Some("star_detection_profile"),
            detail: Some(detail),
            ..Default::default()
        },
    );
    Ok(Json(ApiResponse::success(result?)))
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

    fn plugin_record(captured: &str, settings: ProfileSettings) -> ProfileRecord {
        ProfileRecord {
            id: uuid::Uuid::new_v4().to_string(),
            effective_from: Some(captured.parse().unwrap()),
            source: RecordSource::SyncPlugin,
            received_at: chrono::Utc::now(),
            catalog_id: Some("db-1".into()),
            nina_version: Some("3.2.0.9001".into()),
            hocus_focus_version: HocusFocusVersion::V4,
            settings,
        }
    }

    const PROFILE: &str = "46d7a6e3-3ae8-48a9-8d29-598e3647f131";

    #[test]
    fn a_snapshot_is_stored_only_when_the_settings_change() {
        let mut settings = StarDetectionSettings::default();
        let cropped = |ratio| ProfileSettings {
            autofocus_inner_crop_ratio: ratio,
            ..Default::default()
        };
        let add = |settings: &mut StarDetectionSettings, captured, ratio| {
            add_record(
                settings,
                PROFILE,
                Some("redcat".into()),
                plugin_record(captured, cropped(ratio)),
            )
            .unwrap()
        };
        assert!(add(&mut settings, "2026-09-01T03:00:00Z", 0.5));
        // Every later capture with the same settings stores nothing.
        assert!(!add(&mut settings, "2026-09-02T03:00:00Z", 0.5));
        assert!(!add(&mut settings, "2026-09-08T03:00:00Z", 0.5));
        assert!(add(&mut settings, "2026-09-09T03:00:00Z", 0.7));
        assert!(!add(&mut settings, "2026-09-10T03:00:00Z", 0.7));
        // A capture before the first record falls under the earliest one:
        // the same settings store nothing, other settings a record.
        assert!(!add(&mut settings, "2026-08-20T03:00:00Z", 0.5));
        assert!(add(&mut settings, "2026-08-20T03:00:00Z", 0.6));
        let profile = &settings.profiles[PROFILE];
        assert_eq!(profile.records.len(), 3);
        assert_eq!(profile.name.as_deref(), Some("redcat"));

        // Paired with another catalog, the same settings are stored once, so
        // that catalog learns its profile and the old one stops claiming it.
        let moved = |captured: &str| ProfileRecord {
            catalog_id: Some("db-2".into()),
            ..plugin_record(captured, cropped(0.7))
        };
        assert!(add_record(&mut settings, PROFILE, None, moved("2026-09-11T03:00:00Z")).unwrap());
        assert!(!add_record(&mut settings, PROFILE, None, moved("2026-09-12T03:00:00Z")).unwrap());
        assert_eq!(settings.plugin_profile("db-2"), Some(PROFILE));
        assert_eq!(settings.plugin_profile("db-1"), None);
        settings.profiles.get_mut(PROFILE).unwrap().records.pop();

        // The same file imported twice for every frame is stored once.
        let all_frames = ProfileRecord {
            effective_from: None,
            source: RecordSource::Import,
            ..plugin_record("2026-01-01T00:00:00Z", cropped(1.0))
        };
        assert!(add_record(&mut settings, PROFILE, None, all_frames.clone()).unwrap());
        assert!(!add_record(&mut settings, PROFILE, None, all_frames).unwrap());
    }

    #[test]
    fn records_and_profiles_are_bounded() {
        let mut settings = StarDetectionSettings::default();
        for index in 0..MAX_RECORDS {
            let captured =
                chrono::DateTime::from_timestamp(1_700_000_000 + index as i64 * 86_400, 0)
                    .unwrap()
                    .to_rfc3339();
            let ratio = if index % 2 == 0 { 0.5 } else { 0.7 };
            add_record(
                &mut settings,
                PROFILE,
                None,
                plugin_record(
                    &captured,
                    ProfileSettings {
                        autofocus_inner_crop_ratio: ratio,
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
        }
        let refused = add_record(
            &mut settings,
            PROFILE,
            None,
            plugin_record("2030-01-01T00:00:00Z", ProfileSettings::default()),
        );
        assert!(matches!(refused, Err(AppError::BadRequest(_))));
        assert!(profile_id_key("not-a-guid").is_err());
        assert_eq!(
            profile_id_key(" 46D7A6E3-3AE8-48A9-8D29-598E3647F131 ").unwrap(),
            PROFILE
        );
    }

    #[test]
    fn a_plugin_request_is_checked_and_its_version_read() {
        let request: PluginProfileRequest = serde_json::from_value(json!({
            "protocol_version": 1,
            "catalog_id": "db-49b9f973",
            "profile_id": PROFILE,
            "profile_name": "starfront-mini-redcat61",
            "captured_at": "2026-09-09T03:11:57Z",
            "nina_version": "3.2.0.9001",
            "hocus_focus_version": "4.0.0.21",
            "settings": {
                "star_detector": "hocus_focus",
                "star_sensitivity": "high",
                "noise_reduction": "none",
                "autofocus_inner_crop_ratio": 0.7,
                "hocus_focus": {"Simple_PixelScale": "WideField", "PixelSampleSize": 0.5},
            },
        }))
        .unwrap();
        assert!(request.settings.validate().is_ok());
        assert_eq!(
            reported_hocus_focus_version(request.hocus_focus_version.as_deref()).unwrap(),
            HocusFocusVersion::V4
        );
        assert_eq!(
            reported_hocus_focus_version(Some("3.0.0.26")).unwrap(),
            HocusFocusVersion::V3
        );
        assert_eq!(
            reported_hocus_focus_version(None).unwrap(),
            HocusFocusVersion::V4
        );
        assert!(reported_hocus_focus_version(Some("beta")).is_err());
        assert!(serde_json::from_value::<PluginProfileRequest>(json!({
            "protocol_version": 1, "catalog_id": "x", "profile_id": PROFILE,
            "captured_at": "2026-09-09T03:11:57Z", "settings": {}, "token": "psfrc_x",
        }))
        .is_err());
        let mut bad = request.settings.clone();
        bad.hocus_focus
            .insert("ServerUrl".into(), json!("https://example"));
        assert!(bad.validate().is_err());
        // The registry ignores a settings field it does not know; a
        // request does not.
        assert!(serde_json::from_value::<PluginProfileRequest>(json!({
            "protocol_version": 1, "catalog_id": "x", "profile_id": PROFILE,
            "captured_at": "2026-09-09T03:11:57Z",
            "settings": {"star_sensitivity": "high", "autofocus_inner_crop": 0.7},
        }))
        .is_err());
        assert!(serde_json::from_value::<ProfileSettings>(json!({
            "star_sensitivity": "high", "autofocus_inner_crop": 0.7,
        }))
        .is_ok());
    }

    #[test]
    fn a_profile_using_brightest_stars_without_hocus_focus_says_its_hfr_can_differ() {
        let record = |detector| {
            plugin_record(
                "2026-09-09T03:00:00Z",
                ProfileSettings {
                    star_detector: detector,
                    autofocus_brightest_stars: 30,
                    ..Default::default()
                },
            )
        };
        let nina = RecordView::of(&record(crate::star_detection::StarDetectorSelection::Nina));
        assert!(nina
            .notes
            .iter()
            .any(|note| note.contains("30 brightest stars")));
        let hocus_focus = RecordView::of(&record(
            crate::star_detection::StarDetectorSelection::HocusFocus,
        ));
        assert!(hocus_focus.notes.is_empty());
    }

    #[test]
    fn profiles_merge_catalog_counts_with_their_records() {
        let mut settings = StarDetectionSettings::default();
        add_record(
            &mut settings,
            PROFILE,
            Some("redcat".into()),
            plugin_record(
                "2026-09-09T03:00:00Z",
                ProfileSettings {
                    hocus_focus: [("UseAdvanced".to_string(), json!(true))]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        let views = merge_profile_views(
            vec![
                (
                    "Askar".into(),
                    "daacea0c-6d1d-4163-b582-78b2bb178fd6".into(),
                    12_000,
                ),
                ("Redcat".into(), PROFILE.into(), 1_900),
                ("Remote".into(), PROFILE.into(), 20),
            ],
            &settings,
        );
        assert_eq!(views[0].profile_id, PROFILE, "profiles with settings first");
        assert_eq!(views[0].frames, 1_920);
        assert_eq!(views[0].databases, vec!["Redcat", "Remote"]);
        assert_eq!(views[0].records[0].hocus_focus_mode, "advanced");
        assert_eq!(views[1].frames, 12_000);
        assert!(views[1].records.is_empty());
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
