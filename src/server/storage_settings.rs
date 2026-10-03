//! `GET`/`PUT /api/settings/storage`: the cache's disk limit, and what the
//! last pass over the cache volumes found. `PUT /api/settings/storage/folders`
//! chooses where the cache, stacks and calibration masters go from the next
//! start on.

use crate::db_registry::{DbRegistry, StorageSettings};
use crate::server::api::ApiResponse;
use crate::server::cache_budget::{self, VolumeReport};
use crate::server::handlers::{
    require_database_management_allowed, require_registry_path, update_registry, AppError,
};
use crate::server::state::AppState;
use crate::server::storage::{
    relocate::{overlaps, same_folder},
    FolderSource, StorageKind,
};
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
    /// Cache, stacks and calibration masters, in that order.
    pub folders: Vec<StorageFolderResponse>,
    /// What this start's folder move did, one line per folder.
    pub folder_notes: Vec<String>,
    /// Whether this server keeps a registry Settings can save folders to.
    pub can_choose_folders: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageFolderResponse {
    pub kind: StorageKind,
    /// In use now.
    pub path: String,
    /// What the next start will use; differs from `path` until a restart.
    pub next_path: String,
    pub source: FolderSource,
    /// The folder chosen in Settings, if one is.
    pub chosen: Option<String>,
}

fn response(state: &AppState) -> StorageSettingsResponse {
    let registry = require_registry_path(state)
        .ok()
        .and_then(|path| DbRegistry::load_or_init(&path).ok());
    let settings = registry
        .as_ref()
        .and_then(|registry| registry.storage.as_ref());
    let status = state.storage_status.read().unwrap().clone();
    let (next, sources) = status.config.resolve(settings);
    let folders = StorageKind::ALL
        .into_iter()
        .map(|kind| StorageFolderResponse {
            kind,
            path: state.storage_roots.get(kind).display().to_string(),
            next_path: next.get(kind).display().to_string(),
            source: sources[kind as usize],
            chosen: settings.and_then(|settings| chosen(settings, kind).clone()),
        })
        .collect();
    StorageSettingsResponse {
        max_volume_percent: cache_budget::max_volume_percent(),
        default_max_volume_percent: cache_budget::DEFAULT_MAX_VOLUME_PERCENT,
        min_max_volume_percent: cache_budget::MIN_MAX_VOLUME_PERCENT,
        volumes: cache_budget::last_reports(),
        folders,
        folder_notes: status.notes,
        can_choose_folders: registry.is_some(),
    }
}

fn chosen(settings: &StorageSettings, kind: StorageKind) -> &Option<String> {
    match kind {
        StorageKind::Cache => &settings.cache_dir,
        StorageKind::Stacks => &settings.stack_dir,
        StorageKind::Calibration => &settings.calibration_dir,
    }
}

fn chosen_mut(settings: &mut StorageSettings, kind: StorageKind) -> &mut Option<String> {
    match kind {
        StorageKind::Cache => &mut settings.cache_dir,
        StorageKind::Stacks => &mut settings.stack_dir,
        StorageKind::Calibration => &mut settings.calibration_dir,
    }
}

/// Keep the block only while it holds something, so a registry that only
/// ever held defaults stays clean for older builds.
fn store(registry: &mut DbRegistry, edit: impl FnOnce(&mut StorageSettings)) {
    let mut storage = registry.storage.take().unwrap_or_default();
    edit(&mut storage);
    registry.storage = (!storage.is_empty()).then_some(storage);
}

/// GET /api/settings/storage
pub async fn get_storage_settings(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<StorageSettingsResponse>> {
    Json(ApiResponse::success(response(&state)))
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
    let storage = update_registry(&state, |registry| {
        store(registry, |storage| {
            storage.max_volume_percent =
                (percent != cache_budget::DEFAULT_MAX_VOLUME_PERCENT).then_some(percent);
        });
        Ok(registry.storage.clone())
    })
    .await?;
    cache_budget::configure(storage.as_ref());
    let state_for_pass = Arc::clone(&state);
    let reports = tokio::task::spawn_blocking(move || cache_budget::pass(&state_for_pass))
        .await
        .map_err(|error| AppError::InternalError(format!("storage check failed: {error}")))?;
    cache_budget::record(reports);
    Ok(Json(ApiResponse::success(response(&state))))
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateStorageFoldersRequest {
    /// Absent or empty goes back to the default for each.
    #[serde(default)]
    pub cache_dir: Option<String>,
    #[serde(default)]
    pub stack_dir: Option<String>,
    #[serde(default)]
    pub calibration_dir: Option<String>,
}

/// PUT /api/settings/storage/folders — names server paths, so it sits
/// behind database management. Takes effect at the next start, which moves
/// the files.
pub async fn update_storage_folders(
    State(state): State<Arc<AppState>>,
    Json(request): Json<UpdateStorageFoldersRequest>,
) -> Result<Json<ApiResponse<StorageSettingsResponse>>, AppError> {
    require_database_management_allowed(&state)?;
    let config = state.storage_status.read().unwrap().config.clone();
    let clean = |value: Option<String>| {
        value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    // A folder the config file fixes keeps whatever Settings held for it,
    // so removing the config line later brings that choice back.
    let mut chosen_now: Vec<(StorageKind, Option<String>)> = [
        (StorageKind::Cache, clean(request.cache_dir)),
        (StorageKind::Stacks, clean(request.stack_dir)),
        (StorageKind::Calibration, clean(request.calibration_dir)),
    ]
    .into_iter()
    .filter(|(kind, _)| !config.is_fixed(*kind))
    .collect();
    for (kind, value) in &chosen_now {
        if value
            .as_deref()
            .is_some_and(|path| !std::path::Path::new(path).is_absolute())
        {
            return Err(AppError::BadRequest(format!(
                "{}: use an absolute path",
                kind.label()
            )));
        }
    }
    let mut candidate = StorageSettings::default();
    for (kind, value) in &chosen_now {
        *chosen_mut(&mut candidate, *kind) = value.clone();
    }
    let (next, sources) = config.resolve(Some(&candidate));

    // Two folders that differ must not sit one inside the other: a move
    // would carry one along with the other, and a database slug could
    // collide with a folder name.
    for (index, kind) in StorageKind::ALL.iter().enumerate() {
        for other in &StorageKind::ALL[index + 1..] {
            let (left, right) = (next.get(*kind), next.get(*other));
            if !same_folder(left, right) && overlaps(left, right) {
                return Err(AppError::BadRequest(format!(
                    "the {} folder and the {} folder cannot sit one inside the other",
                    kind.label(),
                    other.label()
                )));
            }
        }
    }
    let in_use = state.storage_roots.clone();
    if !same_folder(&next.cache, &in_use.cache) && overlaps(&next.cache, &in_use.cache) {
        return Err(AppError::BadRequest(
            "the new cache folder cannot be inside the current one, or hold it".into(),
        ));
    }

    // Off the async threads: a folder on a network mount can stall.
    let to_check: Vec<(StorageKind, std::path::PathBuf)> = chosen_now
        .iter()
        .filter(|(_, value)| value.is_some())
        .map(|(kind, _)| (*kind, next.get(*kind).to_path_buf()))
        .collect();
    tokio::task::spawn_blocking(move || {
        to_check.iter().try_for_each(|(kind, path)| {
            check_folder(path).map_err(|error| format!("{}: {error}", kind.label()))
        })
    })
    .await
    .map_err(|error| AppError::InternalError(format!("folder check failed: {error}")))?
    .map_err(AppError::BadRequest)?;

    let moves = crate::server::storage::planned_moves(&in_use, &next, &sources);
    update_registry(&state, |registry| {
        store(registry, |storage| {
            for (kind, value) in chosen_now.drain(..) {
                *chosen_mut(storage, kind) = value;
            }
            storage.moves = moves;
        });
        Ok(())
    })
    .await?;
    Ok(Json(ApiResponse::success(response(&state))))
}

/// A folder the server can write: created if missing, and taking a test
/// file.
fn check_folder(path: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|error| format!("cannot create it: {error}"))?;
    let probe = path.join(format!(".psf-guard-write-test-{}", std::process::id()));
    std::fs::write(&probe, b"").map_err(|error| format!("cannot write to it: {error}"))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}
