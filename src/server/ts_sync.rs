//! Owned TS imports and automatic file sync. One runner per catalog; HTTP
//! handlers only enqueue imports. File copies use SQLite's online backup API.

use super::{
    api::ApiResponse,
    handlers::{require_database_management_allowed, require_registry_path, AppError},
    state::AppState,
};
use crate::{db_registry::DbRegistry, ts_sync::Settings};
use anyhow::{Context, Result};
use axum::{
    extract::{Path as UrlPath, State},
    http::StatusCode,
    Json,
};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize)]
pub struct Job {
    #[serde(skip)]
    priority: crate::concurrency::Priority,
    pub stopping: bool,
    pub waiting_for: Option<String>,
    pub started_at: i64,
    pub id: String,
    pub catalog_id: Option<String>,
    pub name: String,
    pub running: bool,
    pub automatic: bool,
    pub source_connected: bool,
    pub stage: String,
    pub pages_done: u64,
    pub pages_total: u64,
    pub error: Option<String>,
    pub report: Option<crate::ts_sync::Report>,
}

#[derive(Default)]
pub struct Manager {
    jobs: Mutex<HashMap<String, Job>>,
}

impl Manager {
    fn update(&self, id: &str, change: impl FnOnce(&mut Job)) {
        if let Some(job) = self.jobs.lock().unwrap().get_mut(id) {
            change(job);
        }
    }
    fn begin(
        &self,
        key: String,
        catalog_id: Option<String>,
        name: String,
        automatic: bool,
    ) -> Option<Job> {
        let mut jobs = self.jobs.lock().unwrap();
        if key.starts_with("import:")
            && jobs
                .values()
                .any(|j| j.running && j.id.starts_with("import:"))
        {
            return None;
        }
        if jobs.get(&key).is_some_and(|job| job.running) {
            return None;
        }
        jobs.retain(|_, job| job.running || job.catalog_id.is_some());
        let job = Job {
            priority: if automatic {
                crate::concurrency::Priority::Background
            } else {
                crate::concurrency::Priority::Interactive
            },
            stopping: false,
            waiting_for: None,
            started_at: chrono::Utc::now().timestamp_millis(),
            id: key.clone(),
            catalog_id,
            name,
            running: true,
            automatic,
            source_connected: true,
            stage: "copying".into(),
            pages_done: 0,
            pages_total: 0,
            error: None,
            report: None,
        };
        jobs.insert(key, job.clone());
        Some(job)
    }
    fn finish(&self, id: &str, result: Result<()>) {
        self.update(id, |job| {
            job.running = false;
            job.waiting_for = None;
            let stopped = result
                .as_ref()
                .is_err_and(|error| error.is::<crate::backup::Cancelled>());
            job.stage = if stopped {
                "stopped"
            } else if result.is_ok() {
                "complete"
            } else {
                "error"
            }
            .into();
            job.error = if stopped {
                None
            } else {
                result.err().map(|e| format!("{e:#}"))
            };
        });
    }

    pub(super) fn stopped(&self, id: &str) -> bool {
        self.jobs
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|j| j.stopping)
    }
}

pub async fn stop_route(
    State(state): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    require_database_management_allowed(&state)?;
    state.ts_sync.update(&id, |job| job.stopping = true);
    Ok(Json(ApiResponse::success(())))
}

pub async fn status_route(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<Job>>>, AppError> {
    require_database_management_allowed(&state)?;
    let mut jobs: Vec<_> = state
        .ts_sync
        .jobs
        .lock()
        .unwrap()
        .values()
        .filter(|job| {
            job.running
                || job
                    .catalog_id
                    .as_ref()
                    .is_none_or(|id| state.get_database(id).is_some())
        })
        .cloned()
        .collect();
    jobs.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Json(ApiResponse::success(jobs)))
}

#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    pub name: String,
    pub db_path: String,
    #[serde(default)]
    pub image_dirs: Vec<String>,
    #[serde(default)]
    pub slug: Option<String>,
    /// False imports a standalone catalog copy, retaining all embedded PSF data.
    #[serde(default = "connected_by_default")]
    pub connect_source: bool,
    /// Advanced: a new catalog file in an existing directory on this server.
    #[serde(default)]
    pub destination_path: Option<String>,
}

fn connected_by_default() -> bool {
    true
}

pub async fn import_route(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ImportRequest>,
) -> Result<(StatusCode, Json<ApiResponse<Job>>), AppError> {
    require_database_management_allowed(&state)?;
    require_registry_path(&state)?;
    if request.image_dirs.is_empty() {
        return Err(AppError::BadRequest(
            "Add at least one image folder before importing Target Scheduler.".into(),
        ));
    }
    if !Path::new(&request.db_path).is_absolute() {
        return Err(AppError::BadRequest(
            "Choose an absolute Target Scheduler database path.".into(),
        ));
    }
    if let Some(slug) = &request.slug {
        super::slug::validate_slug(slug).map_err(AppError::BadRequest)?;
    }
    let key = format!("import:{}", uuid::Uuid::new_v4());
    let mut job = state
        .ts_sync
        .begin(key.clone(), None, request.name.clone(), false)
        .ok_or_else(||AppError::BadRequest("A Target Scheduler import is already running. Wait for it to finish or stop it from the job list.".into()))?;
    job.source_connected = request.connect_source;
    state.ts_sync.update(&key, |stored| {
        stored.source_connected = request.connect_source
    });
    let returned = job.clone();
    let runtime = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        let _runtime = runtime.enter();
        let label = if request.connect_source {
            "Target Scheduler import"
        } else {
            "Catalog import"
        };
        let _hold = state.shutdown.hold(label);
        let _interactive = state.begin_named_interactive_job(label);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            import(&state, &key, request)
        }))
        .unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "Target Scheduler import stopped unexpectedly."
            ))
        });
        state.ts_sync.finish(&key, result);
    });
    Ok((StatusCode::ACCEPTED, Json(ApiResponse::success(returned))))
}

fn snapshot(state: &AppState, job: &str, source: &Path, destination: &Path) -> Result<()> {
    crate::backup::sqlite_copy::copy_online(
        source,
        destination,
        &crate::backup::sqlite_copy::StepHooks {
            pause: Duration::ZERO,
            cancelled: &|| state.shutdown.is_stopping() || state.ts_sync.stopped(job),
            before_step: &|| {
                let automatic = state
                    .ts_sync
                    .jobs
                    .lock()
                    .unwrap()
                    .get(job)
                    .is_some_and(|j| j.priority != crate::concurrency::Priority::Interactive);
                if automatic {
                    state.wait_for_interactive_work_unless(
                        Duration::from_millis(250),
                        || state.ts_sync.stopped(job),
                        |waiting| {
                            state
                                .ts_sync
                                .update(job, |j| j.waiting_for = waiting.map(str::to_owned))
                        },
                    );
                }
            },
            progress: &|done, total| {
                state.ts_sync.update(job, |j| {
                    j.pages_done = done;
                    j.pages_total = total;
                })
            },
        },
    )?;
    Ok(())
}

fn import(state: &Arc<AppState>, job: &str, request: ImportRequest) -> Result<()> {
    let registry_path = state
        .registry_path
        .read()
        .unwrap()
        .clone()
        .context("The database registry is unavailable.")?;
    for directory in &request.image_dirs {
        anyhow::ensure!(
            Path::new(directory).is_dir(),
            "Image directory does not exist: {directory}"
        );
    }
    let source = dunce::canonicalize(&request.db_path)
        .context("Opening the Target Scheduler source file")?;
    let base = registry_path
        .parent()
        .context("database registry directory")?
        .join("databases");
    std::fs::create_dir_all(&base)?;
    let destination = if let Some(path) = &request.destination_path {
        let path = Path::new(path);
        anyhow::ensure!(
            path.is_absolute(),
            "Choose an absolute destination database path."
        );
        anyhow::ensure!(!path.exists(), "The destination already exists. Choose a new file; import never overwrites a database.");
        let parent = path
            .parent()
            .context("Choose a database filename inside an existing folder.")?;
        let parent =
            dunce::canonicalize(parent).context("The destination folder must already exist")?;
        parent.join(
            path.file_name()
                .context("Choose a destination database filename.")?,
        )
    } else {
        dunce::canonicalize(&base)?.join(format!("{}.sqlite", uuid::Uuid::new_v4()))
    };
    anyhow::ensure!(
        destination != source,
        "The source and destination database must be different files."
    );
    let temporary_directory = tempfile::tempdir_in(destination.parent().unwrap())?;
    let copied = temporary_directory.path().join("catalog.sqlite");
    snapshot(state, job, &source, &copied)?;
    state
        .ts_sync
        .update(job, |j| j.stage = "initializing".into());
    if request.connect_source {
        let mut conn = Connection::open(&copied)?;
        let tx = conn.transaction()?;
        crate::ts_sync::initialize(&tx)?;
        tx.commit()?;
    }
    let _guard = state.registry_write.blocking_lock();
    if state.shutdown.is_stopping() || state.ts_sync.stopped(job) {
        return Err(crate::backup::Cancelled.into());
    }
    let mut registry = DbRegistry::load_or_init(&registry_path)?;
    anyhow::ensure!(
        !request.connect_source
            || !registry.databases.iter().any(|e| e
                .ts_sync
                .as_ref()
                .is_some_and(|s| Path::new(&s.source_path) == source)),
        "This Target Scheduler file is already connected to a catalog."
    );
    let name = if request.name.trim().is_empty() {
        source
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    } else {
        request.name
    };
    let entry = registry
        .add(
            name,
            destination.to_string_lossy().into_owned(),
            request.image_dirs,
            request.slug,
        )?
        .clone();
    let entry = registry
        .databases
        .iter_mut()
        .find(|e| e.id == entry.id)
        .unwrap();
    entry.ts_sync = request.connect_source.then(|| Settings {
        source_path: source.to_string_lossy().into_owned(),
        ..Default::default()
    });
    let entry = entry.clone();
    // The snapshot is complete and closed before it becomes the managed file.
    tempfile::TempPath::try_from_path(copied)?
        .persist_noclobber(&destination)
        .map_err(|e| e.error)
        .context("Publishing the imported catalog without overwriting another file")?;
    let result = (|| -> Result<_> {
        let context = Arc::new(super::database_context::DatabaseContext::from_entry(
            &entry,
            state.storage_roots.clone(),
        )?);
        registry.save(&registry_path)?;
        Ok(context)
    })();
    let context = match result {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&destination);
            return Err(e);
        }
    };
    state
        .databases
        .write()
        .unwrap()
        .insert(entry.id.clone(), context.clone());
    state.ts_sync.update(job, |j| {
        j.catalog_id = Some(entry.id.clone());
        j.name = entry.name.clone();
        j.source_connected = entry.ts_sync.is_some();
    });
    let _ = context.ensure_cache_available();
    super::calibration_quality::request(state, &context.database_path);
    super::flat_measure::queue(
        state,
        &context.database_path,
        crate::calibration::flat_groups::job::Scope::everything(),
    );
    Ok(())
}

/// Metadata is a wake-up hint, never a sync cursor. Each run merges a fresh
/// SQLite snapshot by GUID, including grade edits and old-row updates.
type Stamp = Vec<Option<(u64, std::time::SystemTime)>>;

fn stamp(path: &str) -> Stamp {
    [
        path.to_string(),
        format!("{path}-wal"),
        format!("{path}-journal"),
    ]
    .iter()
    .map(|p| {
        std::fs::metadata(p)
            .ok()
            .and_then(|m| Some((m.len(), m.modified().ok()?)))
    })
    .collect()
}

fn due(settings: &Settings, previous: Option<(&Stamp, Duration)>, current: &Stamp) -> bool {
    match previous {
        None => settings.on_open,
        Some((stamp, elapsed)) => {
            elapsed >= Duration::from_secs(30)
                && ((settings.on_change && stamp != current)
                    || (settings.interval_minutes > 0
                        && elapsed
                            >= Duration::from_secs(u64::from(settings.interval_minutes) * 60)))
        }
    }
}

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut observed: HashMap<String, (Stamp, Instant)> = HashMap::new();
        loop {
            if state.shutdown.is_stopping() {
                break;
            }
            if let Some(path) = state.registry_path.read().unwrap().clone()
                && let Ok(registry) = DbRegistry::load_or_init(&path)
            {
                for entry in registry.databases {
                    let Some(settings) = entry.ts_sync.clone() else {
                        continue;
                    };
                    if settings.validate(Path::new(&entry.db_path)).is_err() {
                        continue;
                    }
                    let current = [stamp(&settings.source_path), stamp(&entry.db_path)].concat();
                    let now = Instant::now();
                    let run = due(
                        &settings,
                        observed
                            .get(&entry.id)
                            .map(|(stamp, attempt)| (stamp, attempt.elapsed())),
                        &current,
                    );
                    if !observed.contains_key(&entry.id) {
                        observed.insert(entry.id.clone(), (current.clone(), now));
                    }
                    if !run {
                        continue;
                    }
                    if let Some(job) = state.ts_sync.begin(
                        entry.id.clone(),
                        Some(entry.id.clone()),
                        entry.name.clone(),
                        true,
                    ) {
                        observed.insert(entry.id.clone(), (current, now));
                        let state = state.clone();
                        let runtime = tokio::runtime::Handle::current();
                        std::thread::spawn(move || {
                            let _runtime = runtime.enter();
                            let _hold = state.shutdown.hold(format!("TS sync ({})", entry.name));
                            state.wait_for_interactive_work_unless(
                                Duration::from_millis(250),
                                || state.ts_sync.stopped(&job.id),
                                |waiting| {
                                    state.ts_sync.update(&job.id, |j| {
                                        j.waiting_for = waiting.map(str::to_owned)
                                    })
                                },
                            );
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    synchronize(&state, &job.id, &entry.db_path, &settings)?;
                                    runtime.block_on(super::director::retry_ts_planning(
                                        &state, &entry.id,
                                    ))
                                }))
                                .unwrap_or_else(|_| {
                                    Err(anyhow::anyhow!("TS sync stopped unexpectedly."))
                                });
                            state.ts_sync.finish(&job.id, result);
                        });
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

fn synchronize(state: &Arc<AppState>, job: &str, catalog: &str, settings: &Settings) -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let copied = temporary.path().join("source.sqlite");
    snapshot(state, job, Path::new(&settings.source_path), &copied)?;
    state.ts_sync.update(job, |j| j.stage = "syncing".into());
    let snapshot = Connection::open_with_flags(copied, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut local = super::database_context::open_scheduler_connection(catalog)?;
    if state.shutdown.is_stopping() || state.ts_sync.stopped(job) {
        return Err(crate::backup::Cancelled.into());
    }
    let _sync_guard = state.sync_apply_lock.blocking_lock();
    if state.shutdown.is_stopping() || state.ts_sync.stopped(job) {
        return Err(crate::backup::Cancelled.into());
    }
    let current = state
        .registry_path
        .read()
        .unwrap()
        .clone()
        .and_then(|p| DbRegistry::load_or_init(&p).ok())
        .and_then(|r| r.find(job).and_then(|e| e.ts_sync.clone()));
    let current =
        current.context("The TS sync connection was removed while the job was running.")?;
    anyhow::ensure!(
        current.source_path == settings.source_path,
        "The TS source changed while sync was running. Start sync again for the new source."
    );
    let writer = if current.writeback {
        Some(
            super::database_context::open_scheduler_connection_with_flags(
                &settings.source_path,
                OpenFlags::SQLITE_OPEN_READ_WRITE,
            ),
        )
    } else {
        None
    };
    let (mut destination, write_error) = match writer {
        Some(Ok(connection)) => (Some(connection), None),
        Some(Err(error)) => (None, Some(error)),
        None => (None, None),
    };
    let report = crate::ts_sync::synchronize(&mut local, &snapshot, destination.as_mut())?;
    if let Some(context) = state.get_database(job) {
        context.register_remote_image_arrivals(&report.arrivals);
        let _ = context.ensure_cache_available();
        if report.inserted > 0 || report.updated > 0 {
            super::quality_arrival::note_arrival(job);
            state
                .auto_stacks
                .touch_database(job, super::stack_preview::automatic::RefreshReason::Sync);
            super::flat_measure::queue(
                state,
                catalog,
                crate::calibration::flat_groups::job::Scope::everything(),
            );
        }
    }
    let conflicts = report.conflicts;
    state.ts_sync.update(job, |j| j.report = Some(report));
    if let Some(error) = write_error {
        anyhow::bail!("Captures were imported, but the TS file could not be opened for grade writeback: {error}");
    }
    anyhow::ensure!(conflicts==0,"{conflicts} grade changes conflict with Target Scheduler. Review the differences in Settings › Sync; those grades were not written.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_change_detection_and_schedule_are_independent() {
        let unchanged = vec![None];
        let changed = vec![];
        let mut settings = Settings::default();
        assert!(due(&settings, None, &unchanged));
        settings.on_open = false;
        assert!(!due(&settings, None, &unchanged));
        assert!(!due(
            &settings,
            Some((&unchanged, Duration::from_secs(29))),
            &changed
        ));
        assert!(due(
            &settings,
            Some((&unchanged, Duration::from_secs(30))),
            &changed
        ));
        settings.on_change = false;
        assert!(!due(
            &settings,
            Some((&unchanged, Duration::from_secs(299))),
            &changed
        ));
        assert!(due(
            &settings,
            Some((&unchanged, Duration::from_secs(300))),
            &unchanged
        ));
        settings.interval_minutes = 0;
        assert!(!due(
            &settings,
            Some((&unchanged, Duration::from_secs(86400))),
            &changed
        ));
        settings.on_change = true;
        assert!(due(
            &settings,
            Some((&unchanged, Duration::from_secs(30))),
            &changed
        ));
    }

    #[tokio::test]
    async fn removing_a_catalog_keeps_its_running_job_visible_until_it_stops() {
        let temporary = tempfile::tempdir().unwrap();
        let state = Arc::new(
            AppState::from_databases(
                vec![],
                temporary
                    .path()
                    .join("cache")
                    .to_string_lossy()
                    .into_owned(),
                crate::cli::PregenerationConfig::default(),
            )
            .unwrap(),
        );
        state.set_allow_database_management(true);
        state
            .ts_sync
            .begin("removed".into(), Some("removed".into()), "Rig".into(), true)
            .unwrap();
        let jobs = status_route(State(state.clone()))
            .await
            .unwrap()
            .0
            .data
            .unwrap();
        assert_eq!(jobs.len(), 1);
        let _ = stop_route(State(state.clone()), UrlPath("removed".into()))
            .await
            .unwrap();
        assert!(state.ts_sync.stopped("removed"));
        state
            .ts_sync
            .finish("removed", Err(crate::backup::Cancelled.into()));
        let jobs = status_route(State(state.clone()))
            .await
            .unwrap()
            .0
            .data
            .unwrap();
        assert!(jobs.is_empty());
        state.shutdown.begin();
    }
}
