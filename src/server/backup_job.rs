//! Backups from Settings › Backups and on a schedule.
//!
//! One backup runs at a time, server-wide, on a runner thread of its own.
//! A handler only checks the request and queues the run; the plan, the
//! copies and the archive are made on the runner. A scheduled run is
//! `Background` work: before each file and each SQLite step it waits while a
//! person's work runs, and the job line lists it as waiting meanwhile. A run
//! a person starts does not wait. Stop and the server's stop end a run at its
//! next step; nothing of it is left in the folder.
//!
//! Every route sits behind the database-management gate and needs an
//! editor: an archive holds every catalog and the server's settings.
//!
//! See `docs/BACKUPS.md` and `src/backup/`.

use crate::backup::{
    self, schedule::BackupSettings, secrets::SecretsSummary, BackupScope, Listing, Locations,
    Trigger,
};
use crate::db_registry::DbRegistry;
use crate::server::api::ApiResponse;
use crate::server::auth::RequestAccess;
use crate::server::handlers::{
    require_database_management_allowed, require_registry_path, update_registry, AppError,
};
use crate::server::state::AppState;
use anyhow::Context;
use axum::{
    extract::{Extension, Path as UrlPath, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

/// How often the schedule is looked at.
pub const TICK: Duration = Duration::from_secs(30);
/// After a scheduled run fails, the next attempt waits this long.
pub const RETRY_AFTER: Duration = Duration::from_secs(3600);
/// How often a waiting run looks again for a person's work to end.
const NAP: Duration = Duration::from_millis(500);
/// Archives a browser download leaves on the server, and for how long.
const DOWNLOADS_KEPT: usize = 3;
const DOWNLOADS_MAX_AGE: Duration = Duration::from_secs(24 * 3600);
/// The folder below the cache for archives made to be downloaded.
const DOWNLOADS_FOLDER: &str = "backup-downloads";

/// Where an archive is: the schedule's folder, or the server's download area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Location {
    Destination,
    Downloads,
    /// A folder named for one run only.
    Elsewhere,
}

/// The run under way, for Settings and the job line.
#[derive(Debug, Clone, Serialize)]
pub struct JobStatus {
    pub trigger: Trigger,
    pub scope: BackupScope,
    pub include_secrets: bool,
    pub destination: String,
    pub location: Location,
    /// `preparing`, `copying`, `compressing`, `finishing` or `pruning`.
    pub stage: String,
    /// The file at hand: `catalog imaging-rig`, `config.json`.
    pub current: Option<String>,
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// What a scheduled run steps aside for, while it does.
    pub waiting_for: Option<String>,
    pub started_at: i64,
    /// Stop was asked; the run ends at its next step.
    pub stopping: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Succeeded,
    Failed,
    Stopped,
}

/// How the last run went. Kept across restarts in `backup-state.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LastRun {
    pub trigger: Trigger,
    pub scope: BackupScope,
    pub started_at: i64,
    pub finished_at: i64,
    pub outcome: Outcome,
    pub location: Location,
    pub destination: String,
    pub file_name: Option<String>,
    pub bytes: Option<u64>,
    pub error: Option<String>,
    /// Files the archive had to leave out, such as an unreachable catalog.
    pub problems: Vec<String>,
    /// Older scheduled archives the run removed.
    pub pruned: Vec<String>,
    /// Older archives the retention would remove, kept because they hold
    /// catalogs this run left out.
    pub prune_kept: Vec<String>,
    /// Those catalogs, by name.
    pub prune_kept_for: Vec<String>,
    /// Why the run removed nothing at all, when it did not prune.
    pub prune_skipped: Option<String>,
}

#[derive(Default)]
struct Inner {
    job: Option<JobStatus>,
    last: Option<LastRun>,
    /// The newest scheduled archive in a folder, read once per folder and
    /// kept up to date by the runs.
    newest_scheduled: Option<(PathBuf, Option<i64>)>,
    /// A stopped scheduled run: the schedule waits for its next slot rather
    /// than starting again at once.
    held_since: Option<i64>,
    /// A failed scheduled run: no new attempt before this.
    retry_after: Option<i64>,
}

/// The server's backups: the run under way, the last one, and what the
/// schedule knows.
#[derive(Default)]
pub struct BackupManager {
    inner: Mutex<Inner>,
    cancel: AtomicBool,
    server_config: RwLock<Option<PathBuf>>,
    director_meta: RwLock<Option<PathBuf>>,
    /// Pause between SQLite steps; tests slow it down.
    step_pause: RwLock<Option<Duration>>,
    /// Where a stopped or failed scheduled run, and the last run, are
    /// remembered across restarts, beside the registry.
    state_path: RwLock<Option<PathBuf>>,
    /// Held from the change to the rename, so the file ends with the newest
    /// state and two saves never share the temporary file.
    saving: Mutex<()>,
}

/// What the backups keep across restarts: a stopped run waits for its
/// next slot, and a failed one an hour, even when the server restarts; and
/// Settings still says how the last backup went.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct SavedState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    held_since: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retry_after: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last: Option<LastRun>,
}

/// `backup-state.json` beside the standard registry, `<stem>.backup-state.json`
/// beside another.
pub fn state_path_for(registry: &Path) -> PathBuf {
    if registry.file_name().and_then(|name| name.to_str()) == Some("config.json") {
        return registry.with_file_name("backup-state.json");
    }
    let stem = registry
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("config");
    registry.with_file_name(format!("{stem}.backup-state.json"))
}

impl BackupManager {
    /// The server's TOML file and Director store, as the start found them.
    pub fn set_locations(&self, server_config: Option<PathBuf>, director_meta: Option<PathBuf>) {
        *self.server_config.write().unwrap() = server_config;
        *self.director_meta.write().unwrap() = director_meta;
    }

    /// Read what the backups remembered before a restart, and keep it
    /// beside `registry` from now on.
    pub fn load_state(&self, registry: &Path) {
        let path = state_path_for(registry);
        let saved: SavedState = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        {
            let mut inner = self.inner.lock().unwrap();
            inner.held_since = saved.held_since;
            inner.retry_after = saved.retry_after;
            inner.last = saved.last;
        }
        *self.state_path.write().unwrap() = Some(path);
    }

    /// Change what the backups remember, and save it.
    fn remember(&self, change: impl FnOnce(&mut Inner)) {
        let _saving = self
            .saving
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let saved = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            change(&mut inner);
            SavedState {
                held_since: inner.held_since,
                retry_after: inner.retry_after,
                last: inner.last.clone(),
            }
        };
        let Some(path) = self.state_path.read().unwrap().clone() else {
            return;
        };
        let write = || -> std::io::Result<()> {
            if saved == SavedState::default() {
                return match std::fs::remove_file(&path) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                    _ => Ok(()),
                };
            }
            let temporary = path.with_extension("json.tmp");
            std::fs::write(&temporary, serde_json::to_vec_pretty(&saved)?)?;
            std::fs::rename(&temporary, &path)
        };
        if let Err(error) = write() {
            tracing::warn!(
                "Could not save the backup schedule's state to {}: {error}",
                path.display()
            );
        }
    }

    fn update(&self, change: impl FnOnce(&mut JobStatus)) {
        if let Some(job) = self.inner.lock().unwrap().job.as_mut() {
            change(job);
        }
    }

    pub fn job(&self) -> Option<JobStatus> {
        self.inner.lock().unwrap().job.clone()
    }

    pub fn last_run(&self) -> Option<LastRun> {
        self.inner.lock().unwrap().last.clone()
    }

    /// Ask the run under way to stop. False when none runs.
    pub fn stop(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        match inner.job.as_mut() {
            Some(job) => {
                job.stopping = true;
                self.cancel.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_step_pause(&self, pause: Duration) {
        *self.step_pause.write().unwrap() = Some(pause);
    }

    /// The newest archive this install's schedule wrote in `folder`,
    /// reading the folder only when it is not the one last read.
    fn newest_scheduled(&self, folder: &Path, install: &str) -> Option<i64> {
        if let Some((known, newest)) = &self.inner.lock().unwrap().newest_scheduled
            && known == folder
        {
            return *newest;
        }
        let newest = backup::list(folder)
            .into_iter()
            .find(|listing| {
                listing.trigger == Trigger::Schedule
                    && listing.install_id.as_deref() == Some(install)
            })
            .map(|listing| listing.created_unix);
        self.inner.lock().unwrap().newest_scheduled = Some((folder.to_path_buf(), newest));
        newest
    }

    /// When the schedule runs next, as unix seconds; at or before now means
    /// due. `None` when off.
    pub fn next_run_at(
        &self,
        settings: &BackupSettings,
        install: &str,
        now: DateTime<Utc>,
    ) -> Option<i64> {
        let folder = PathBuf::from(settings.destination_path()?);
        let newest = self.newest_scheduled(&folder, install);
        let (held, retry) = {
            let inner = self.inner.lock().unwrap();
            (inner.held_since, inner.retry_after)
        };
        let last = newest
            .max(held)
            .and_then(|s| Utc.timestamp_opt(s, 0).single());
        let next = backup::schedule::next_run(settings, last, now, &chrono::Local)?.timestamp();
        Some(retry.map_or(next, |retry| next.max(retry)))
    }

    /// A changed schedule starts afresh: no held run, no pending retry.
    fn schedule_changed(&self) {
        self.remember(|inner| {
            inner.held_since = None;
            inner.retry_after = None;
            inner.newest_scheduled = None;
        });
    }
}

/// What a run is asked to do.
#[derive(Debug, Clone)]
pub struct RunRequest {
    pub trigger: Trigger,
    pub scope: BackupScope,
    pub include_secrets: bool,
    /// `None` makes an archive to download.
    pub destination: Option<PathBuf>,
}

/// Why a run could not start.
#[derive(Debug)]
pub enum StartError {
    Busy,
    Invalid(String),
}

fn locations(state: &AppState, registry: PathBuf) -> Locations {
    Locations {
        registry,
        server_config: state.backups.server_config.read().unwrap().clone(),
        director_meta: state.backups.director_meta.read().unwrap().clone(),
        cache_root: Some(state.storage_roots.cache.clone()),
    }
}

fn downloads_folder(state: &AppState) -> PathBuf {
    state.storage_roots.cache.join(DOWNLOADS_FOLDER)
}

/// Where a backup folder sits that a person should hear about.
fn warnings_for(state: &AppState, registry: &DbRegistry, folder: &Path) -> Vec<String> {
    let generated = [
        state.storage_roots.cache.clone(),
        state.storage_roots.stacks.clone(),
        state.storage_roots.calibration.clone(),
    ];
    backup::destination_warnings(folder, registry, &generated)
}

/// Queue a run on its own thread. Returns its status as it starts.
pub fn start(state: &Arc<AppState>, request: RunRequest) -> Result<JobStatus, StartError> {
    let registry_path = require_registry_path(state)
        .map_err(|_| StartError::Invalid("this server keeps no registry to back up".into()))?;
    if let Some(destination) = &request.destination
        && !destination.is_absolute()
    {
        return Err(StartError::Invalid(
            "the backup folder must be an absolute path".into(),
        ));
    }
    let (destination, location) = match &request.destination {
        Some(folder) => {
            let configured = DbRegistry::load_or_init(&registry_path)
                .ok()
                .and_then(|registry| registry.backups)
                .and_then(|settings| settings.destination_path().map(PathBuf::from));
            let location = if configured.as_deref() == Some(folder.as_path()) {
                Location::Destination
            } else {
                Location::Elsewhere
            };
            (folder.clone(), location)
        }
        None => (downloads_folder(state), Location::Downloads),
    };
    let status = JobStatus {
        trigger: request.trigger,
        scope: request.scope,
        include_secrets: request.include_secrets,
        destination: destination.display().to_string(),
        location,
        stage: "preparing".into(),
        current: None,
        files_done: 0,
        files_total: 0,
        bytes_done: 0,
        bytes_total: 0,
        waiting_for: None,
        started_at: Utc::now().timestamp(),
        stopping: false,
    };
    {
        let mut inner = state.backups.inner.lock().unwrap();
        if inner.job.is_some() {
            return Err(StartError::Busy);
        }
        inner.job = Some(status.clone());
        state.backups.cancel.store(false, Ordering::SeqCst);
    }
    let runner_state = Arc::clone(state);
    let spawned = std::thread::Builder::new()
        .name("backup".into())
        .spawn(move || {
            let claim = RunnerClaim(&runner_state);
            run(
                &runner_state,
                &request,
                registry_path,
                destination,
                location,
            );
            drop(claim);
        });
    if let Err(error) = spawned {
        state.backups.inner.lock().unwrap().job = None;
        return Err(StartError::Invalid(format!(
            "could not start the backup: {error}"
        )));
    }
    Ok(status)
}

/// Clears the run's slot however the runner ends, a panic included.
struct RunnerClaim<'a>(&'a AppState);

impl Drop for RunnerClaim<'_> {
    fn drop(&mut self) {
        self.0.backups.remember(|inner| {
            if let Some(job) = inner.job.take()
                && inner
                    .last
                    .as_ref()
                    .is_none_or(|last| last.started_at != job.started_at)
            {
                // The runner did not get to record how it went.
                inner.last = Some(LastRun {
                    trigger: job.trigger,
                    scope: job.scope,
                    started_at: job.started_at,
                    finished_at: Utc::now().timestamp(),
                    outcome: Outcome::Failed,
                    location: job.location,
                    destination: job.destination,
                    file_name: None,
                    bytes: None,
                    error: Some("the backup stopped unexpectedly".into()),
                    problems: Vec::new(),
                    pruned: Vec::new(),
                    prune_kept: Vec::new(),
                    prune_kept_for: Vec::new(),
                    prune_skipped: None,
                });
            }
        });
    }
}

fn run(
    state: &Arc<AppState>,
    request: &RunRequest,
    registry_path: PathBuf,
    destination: PathBuf,
    location: Location,
) {
    let manager = &state.backups;
    let _hold = state.shutdown.hold("a backup");
    let started = Utc::now();
    let scheduled = request.trigger == Trigger::Schedule;
    let cancelled = || manager.cancel.load(Ordering::SeqCst) || state.shutdown.is_stopping();
    // A scheduled run waits while a person's work runs, but Stop ends the
    // wait as it ends the run.
    let yield_now = || {
        if scheduled {
            state.wait_for_interactive_work_unless(NAP, cancelled, |waiting_for| {
                manager.update(|job| job.waiting_for = waiting_for.map(str::to_string))
            });
        }
    };
    let progress = |progress: &backup::Progress| {
        manager.update(|job| {
            job.stage = progress.stage.to_string();
            job.current = progress.current.clone();
            job.files_done = progress.files_done;
            job.files_total = progress.files_total;
            job.bytes_done = progress.bytes_done;
            job.bytes_total = progress.bytes_total;
        })
    };
    let hooks = backup::Hooks {
        cancelled: &cancelled,
        yield_now: &yield_now,
        progress: &progress,
        sqlite_pause: manager
            .step_pause
            .read()
            .unwrap()
            .unwrap_or(backup::SQLITE_STEP_PAUSE),
    };

    // The download area is PSF Guard's own, below the cache: made as needed.
    let ready = if location == Location::Downloads {
        std::fs::create_dir_all(&destination)
            .with_context(|| format!("creating {}", destination.display()))
    } else {
        Ok(())
    };
    let registry = ready.and_then(|_| DbRegistry::load_or_init(&registry_path));
    let result = registry.and_then(|registry| {
        let plan = backup::plan(
            &locations(state, registry_path.clone()),
            &registry,
            request.scope,
            request.include_secrets,
        );
        // Accounted against the background share: deflating is one core.
        let _lease = state.lease_workers(crate::concurrency::Priority::Background, 1);
        let written = backup::write(&plan, &destination, request.trigger, started, &hooks)?;
        Ok((registry, written))
    });

    let finished_at = Utc::now().timestamp();
    let mut last = LastRun {
        trigger: request.trigger,
        scope: request.scope,
        started_at: manager
            .job()
            .map_or(started.timestamp(), |job| job.started_at),
        finished_at,
        outcome: Outcome::Succeeded,
        location,
        destination: destination.display().to_string(),
        file_name: None,
        bytes: None,
        error: None,
        problems: Vec::new(),
        pruned: Vec::new(),
        prune_kept: Vec::new(),
        prune_kept_for: Vec::new(),
        prune_skipped: None,
    };
    match result {
        Ok((registry, written)) => {
            tracing::info!(
                "💾 Backup written to {} ({} bytes)",
                written.path.display(),
                written.bytes
            );
            last.file_name = Some(written.file_name.clone());
            last.bytes = Some(written.bytes);
            last.problems = written.manifest.problems.clone();
            manager.update(|job| {
                job.stage = "pruning".into();
                job.current = None;
            });
            if scheduled {
                // A catalog this run could not reach keeps the older
                // archives that hold it (`prune` sees to that), and the rest
                // go as usual, so an unplugged disk does not stop pruning.
                // A settings file it could not copy is in no manifest's
                // catalog list, so then nothing is removed.
                if written.settings_incomplete {
                    last.prune_skipped = Some(
                        "This run could not copy a settings file, so every older backup was \
                         kept."
                            .into(),
                    );
                } else {
                    let retention = registry
                        .backups
                        .as_ref()
                        .map(|s| s.retention)
                        .unwrap_or_default();
                    match backup::prune(&destination, retention, Some(&written.path), finished_at) {
                        Ok(pruned) => {
                            last.pruned = pruned.removed;
                            last.prune_kept = pruned.kept;
                            last.prune_kept_for = pruned
                                .kept_for
                                .iter()
                                .map(|id| {
                                    registry
                                        .find(id)
                                        .map(|entry| entry.name.clone())
                                        .unwrap_or_else(|| id.clone())
                                })
                                .collect();
                        }
                        Err(error) => last
                            .problems
                            .push(format!("could not remove older backups: {error:#}")),
                    }
                }
                manager.remember(|inner| {
                    inner.newest_scheduled = Some((destination.clone(), Some(started.timestamp())));
                    inner.held_since = None;
                    inner.retry_after = None;
                });
            } else if location == Location::Downloads {
                tidy_downloads(&destination, Some(&written.path), finished_at);
            }
        }
        Err(error) if error.downcast_ref::<backup::Cancelled>().is_some() => {
            tracing::info!("💾 Backup stopped");
            last.outcome = Outcome::Stopped;
            if scheduled {
                manager.remember(|inner| inner.held_since = Some(finished_at));
            }
        }
        Err(error) => {
            tracing::warn!("💾 Backup failed: {error:#}");
            last.outcome = Outcome::Failed;
            last.error = Some(format!("{error:#}"));
            if scheduled {
                manager.remember(|inner| {
                    inner.retry_after = Some(finished_at + RETRY_AFTER.as_secs() as i64)
                });
            }
        }
    }
    manager.remember(|inner| {
        inner.last = Some(last);
        inner.job = None;
    });
}

/// Archives made for a download are kept a day, three at most. Swept after
/// each download run, on every tick of the schedule and at start.
fn tidy_downloads(folder: &Path, keep: Option<&Path>, now_unix: i64) {
    for (index, listing) in backup::list(folder).into_iter().enumerate() {
        let old = now_unix - listing.created_unix > DOWNLOADS_MAX_AGE.as_secs() as i64;
        if keep != Some(listing.path.as_path()) && (index >= DOWNLOADS_KEPT || old) {
            let _ = std::fs::remove_file(&listing.path);
        }
    }
    backup::remove_stale_temporaries(folder);
}

/// Look at the schedule once: start a run when one is due. Returns whether
/// it started one.
pub fn tick(state: &Arc<AppState>, now: DateTime<Utc>) -> bool {
    if state.shutdown.is_stopping() || state.backups.job().is_some() {
        return false;
    }
    tidy_downloads(&downloads_folder(state), None, now.timestamp());
    let Ok(registry_path) = require_registry_path(state) else {
        return false;
    };
    let Ok(registry) = DbRegistry::load_or_init(&registry_path) else {
        return false;
    };
    let Some(settings) = registry.backups.clone() else {
        return false;
    };
    let install = backup::install_id(&registry, &registry_path);
    let Some(next) = state.backups.next_run_at(&settings, &install, now) else {
        return false;
    };
    if next > now.timestamp() {
        return false;
    }
    let request = RunRequest {
        trigger: Trigger::Schedule,
        scope: settings.scope,
        include_secrets: settings.include_secrets,
        destination: settings.destination_path().map(PathBuf::from),
    };
    match start(state, request) {
        Ok(_) => {
            tracing::info!("💾 Scheduled backup started");
            true
        }
        Err(StartError::Busy) => false,
        Err(StartError::Invalid(message)) => {
            tracing::warn!("💾 Scheduled backup could not start: {message}");
            state.backups.remember(|inner| {
                inner.retry_after = Some(now.timestamp() + RETRY_AFTER.as_secs() as i64)
            });
            false
        }
    }
}

/// The schedule's loop, for the server's life. It reads the registry each
/// tick, so a schedule changed in Settings applies without a restart.
pub fn spawn(state: Arc<AppState>) {
    if let Ok(registry) = require_registry_path(&state) {
        state.backups.load_state(&registry);
    }
    tokio::spawn(async move {
        let sweeping = Arc::clone(&state);
        let _ = tokio::task::spawn_blocking(move || {
            tidy_downloads(&downloads_folder(&sweeping), None, Utc::now().timestamp())
        })
        .await;
        loop {
            if !state.shutdown.sleep(TICK).await {
                return;
            }
            let ticking = Arc::clone(&state);
            let _ = tokio::task::spawn_blocking(move || tick(&ticking, Utc::now())).await;
        }
    });
}

// ---- Routes ----------------------------------------------------------------

/// Backups expose every catalog and the server's settings: the management
/// gate, and a registry to read.
fn require_backups(state: &AppState) -> Result<PathBuf, AppError> {
    require_database_management_allowed(state)?;
    require_registry_path(state)
}

/// One archive, where it is and how to fetch it.
#[derive(Debug, Clone, Serialize)]
pub struct ListedBackup {
    #[serde(flatten)]
    pub listing: Listing,
    pub location: Location,
    /// Below `/api`.
    pub download_path: String,
}

/// `GET /api/backups`: everything Settings › Backups shows.
#[derive(Debug, Clone, Serialize)]
pub struct BackupsOverview {
    pub settings: BackupSettings,
    /// `~/Documents/psf-guard/backups`, for a person who has not chosen a
    /// folder.
    pub suggested_destination: String,
    pub job: Option<JobStatus>,
    pub last_run: Option<LastRun>,
    /// Unix seconds; at or before now means due.
    pub next_run_at: Option<i64>,
    pub destination_warnings: Vec<String>,
    /// The schedule's folder, then the download area, newest first.
    pub backups: Vec<ListedBackup>,
    pub secrets: SecretsSummary,
    /// The size of what each scope copies, on disk now.
    pub meta_bytes: u64,
    pub catalog_bytes: u64,
    pub catalogs: usize,
    /// Catalogs a full backup cannot reach now.
    pub unreachable_catalogs: Vec<String>,
}

/// `GET /api/backups/status`: the run under way and the last one, for the
/// job line.
#[derive(Debug, Clone, Serialize)]
pub struct BackupJobView {
    pub job: Option<JobStatus>,
    pub last_run: Option<LastRun>,
}

fn overview(state: &AppState, registry_path: &Path) -> Result<BackupsOverview, AppError> {
    let registry = DbRegistry::load_or_init(registry_path)
        .map_err(|error| AppError::InternalError(format!("{error:#}")))?;
    let settings = registry.backups.clone().unwrap_or_default();
    let locations = locations(state, registry_path.to_path_buf());
    let plan = backup::plan(&locations, &registry, BackupScope::Full, false);
    let (catalog_bytes, meta_bytes) = plan.items.iter().fold((0, 0), |(c, m), item| {
        if item.role == backup::FileRole::Catalog {
            (c + item.bytes, m)
        } else {
            (c, m + item.bytes)
        }
    });
    let mut backups = Vec::new();
    let mut destination_warnings = Vec::new();
    if let Some(folder) = settings.destination_path().map(PathBuf::from) {
        destination_warnings = warnings_for(state, &registry, &folder);
        backups.extend(
            backup::list(&folder)
                .into_iter()
                .map(|listing| ListedBackup {
                    download_path: format!("/backups/files/destination/{}", listing.file_name),
                    listing,
                    location: Location::Destination,
                }),
        );
    }
    backups.extend(
        backup::list(&downloads_folder(state))
            .into_iter()
            .map(|listing| ListedBackup {
                download_path: format!("/backups/files/downloads/{}", listing.file_name),
                listing,
                location: Location::Downloads,
            }),
    );
    backups.sort_by_key(|backup| std::cmp::Reverse(backup.listing.created_unix));
    Ok(BackupsOverview {
        next_run_at: state.backups.next_run_at(
            &settings,
            &backup::install_id(&registry, registry_path),
            Utc::now(),
        ),
        suggested_destination: backup::default_destination()
            .unwrap_or_else(|| registry_path.with_file_name("backups"))
            .display()
            .to_string(),
        settings,
        job: state.backups.job(),
        last_run: state.backups.last_run(),
        destination_warnings,
        backups,
        secrets: backup::secrets::summary(&locations),
        meta_bytes,
        catalog_bytes,
        catalogs: plan.catalogs.iter().filter(|c| c.included).count(),
        unreachable_catalogs: plan
            .catalogs
            .iter()
            .filter(|c| !c.included)
            .map(|c| c.name.clone())
            .collect(),
    })
}

/// GET /api/backups
pub async fn get_backups(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<BackupsOverview>>, AppError> {
    let registry_path = require_backups(&state)?;
    let view = tokio::task::spawn_blocking(move || overview(&state, &registry_path))
        .await
        .map_err(|error| AppError::InternalError(error.to_string()))??;
    Ok(Json(ApiResponse::success(view)))
}

/// GET /api/backups/status
pub async fn get_backup_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<BackupJobView>>, AppError> {
    require_backups(&state)?;
    Ok(Json(ApiResponse::success(BackupJobView {
        job: state.backups.job(),
        last_run: state.backups.last_run(),
    })))
}

/// Whether a request came with an API token rather than a person's session
/// or the desktop app.
fn by_api_token(access: &Option<Extension<RequestAccess>>) -> bool {
    access
        .as_ref()
        .is_some_and(|Extension(access)| access.api_token)
}

const SECRETS_NEED_A_PERSON: &str = "Backups with secrets are made only by a person signed in \
     to the server, or in the desktop app, and saved to a server folder; an API token cannot \
     ask for one.";

/// PUT /api/backups/settings
pub async fn update_backup_settings(
    State(state): State<Arc<AppState>>,
    access: Option<Extension<RequestAccess>>,
    Json(mut wanted): Json<BackupSettings>,
) -> Result<Json<ApiResponse<BackupsOverview>>, AppError> {
    let registry_path = require_backups(&state)?;
    wanted.destination = wanted.destination_path().map(|path| path.to_string());
    wanted
        .validate()
        .map_err(|error| AppError::BadRequest(format!("{error:#}")))?;
    if wanted.include_secrets && by_api_token(&access) {
        return Err(AppError::Forbidden(SECRETS_NEED_A_PERSON.into()));
    }
    // The schedule writes only into a folder that is there, so an unmounted
    // disk fails a run instead of filling the system disk. Saving makes the
    // last folder, as a person's own action.
    if wanted.enabled
        && let Some(folder) = wanted.destination_path().map(PathBuf::from)
        && !folder.is_dir()
        && !backup::make_default_destination(&folder).map_err(|error| {
            AppError::BadRequest(format!("could not create {}: {error:#}", folder.display()))
        })?
    {
        let parent_exists = folder.parent().is_some_and(Path::is_dir);
        if folder.exists() || !parent_exists {
            return Err(AppError::BadRequest(format!(
                "destination not found: is its disk mounted? ({})",
                folder.display()
            )));
        }
        std::fs::create_dir(&folder).map_err(|error| {
            AppError::BadRequest(format!("could not create {}: {error}", folder.display()))
        })?;
    }
    let changed = update_registry(&state, |registry| {
        let before = registry.backups.clone().unwrap_or_default();
        // The id is the server's: kept once minted, whatever was sent.
        wanted.install_id = before
            .install_id
            .clone()
            .or_else(|| Some(uuid::Uuid::new_v4().to_string()));
        wanted.since = if before.timing_differs(&wanted) || before.since.is_none() {
            Some(Utc::now().timestamp())
        } else {
            before.since
        };
        let changed = before.timing_differs(&wanted);
        registry.backups = Some(wanted);
        Ok(changed)
    })
    .await?;
    if changed {
        state.backups.schedule_changed();
    }
    let view = tokio::task::spawn_blocking(move || overview(&state, &registry_path))
        .await
        .map_err(|error| AppError::InternalError(error.to_string()))??;
    Ok(Json(ApiResponse::success(view)))
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunBackupRequest {
    #[serde(default)]
    pub scope: BackupScope,
    #[serde(default)]
    pub include_secrets: bool,
    /// A server folder; absent or empty makes an archive to download.
    #[serde(default)]
    pub destination: Option<String>,
}

/// POST /api/backups/run: queue a backup now. 202 while it runs. Secrets
/// are kept only in a server folder, at a person's request: never in an
/// archive to download, nor for an API token.
pub async fn run_backup(
    State(state): State<Arc<AppState>>,
    access: Option<Extension<RequestAccess>>,
    Json(request): Json<RunBackupRequest>,
) -> Result<Response, AppError> {
    require_backups(&state)?;
    let destination = request
        .destination
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    if request.include_secrets {
        if by_api_token(&access) {
            return Err(AppError::Forbidden(SECRETS_NEED_A_PERSON.into()));
        }
        if destination.is_none() {
            return Err(AppError::Forbidden(
                "Secrets are never offered as a download; choose a server folder to keep them."
                    .into(),
            ));
        }
    }
    let request = RunRequest {
        trigger: Trigger::Manual,
        scope: request.scope,
        include_secrets: request.include_secrets,
        destination,
    };
    match start(&state, request) {
        Ok(job) => Ok((
            StatusCode::ACCEPTED,
            Json(ApiResponse::success(BackupJobView {
                job: Some(job),
                last_run: state.backups.last_run(),
            })),
        )
            .into_response()),
        Err(StartError::Busy) => Err(AppError::Conflict(
            "A backup is already running; wait for it or stop it first.".into(),
        )),
        Err(StartError::Invalid(message)) => Err(AppError::BadRequest(message)),
    }
}

/// POST /api/backups/stop
pub async fn stop_backup(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<BackupJobView>>, AppError> {
    require_backups(&state)?;
    state.backups.stop();
    Ok(Json(ApiResponse::success(BackupJobView {
        job: state.backups.job(),
        last_run: state.backups.last_run(),
    })))
}

#[derive(Debug, Clone, Deserialize)]
pub struct CheckDestinationRequest {
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DestinationCheck {
    /// Why the folder cannot be used, if it cannot.
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub exists: bool,
}

/// POST /api/backups/check-destination: what Settings says under the folder
/// field before it is saved.
pub async fn check_destination(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CheckDestinationRequest>,
) -> Result<Json<ApiResponse<DestinationCheck>>, AppError> {
    let registry_path = require_backups(&state)?;
    let check = tokio::task::spawn_blocking(move || {
        let path = PathBuf::from(request.path.trim());
        if request.path.trim().is_empty() {
            return DestinationCheck {
                error: Some("Choose a folder.".into()),
                warnings: Vec::new(),
                exists: false,
            };
        }
        if !path.is_absolute() {
            return DestinationCheck {
                error: Some("Use an absolute path.".into()),
                warnings: Vec::new(),
                exists: false,
            };
        }
        let registry = DbRegistry::load_or_init(&registry_path).unwrap_or_default();
        DestinationCheck {
            error: (path.exists() && !path.is_dir())
                .then(|| "That is a file, not a folder.".into()),
            warnings: warnings_for(&state, &registry, &path),
            exists: path.is_dir(),
        }
    })
    .await
    .map_err(|error| AppError::InternalError(error.to_string()))?;
    Ok(Json(ApiResponse::success(check)))
}

/// GET /api/backups/files/{location}/{name}: one archive, as a download.
pub async fn download_backup(
    State(state): State<Arc<AppState>>,
    UrlPath((location, name)): UrlPath<(String, String)>,
) -> Result<Response, AppError> {
    let registry_path = require_backups(&state)?;
    if !backup::is_archive_name(&name) {
        return Err(AppError::NotFound);
    }
    let folder = match location.as_str() {
        "downloads" => downloads_folder(&state),
        "destination" => DbRegistry::load_or_init(&registry_path)
            .ok()
            .and_then(|registry| registry.backups)
            .and_then(|settings| settings.destination_path().map(PathBuf::from))
            .ok_or(AppError::NotFound)?,
        _ => return Err(AppError::NotFound),
    };
    let path = folder.join(&name);
    if !path.is_file() {
        return Err(AppError::NotFoundMessage(
            "That backup is no longer there.".into(),
        ));
    }
    // An archive with secrets stays on the server's disk, for everyone: as
    // does one whose manifest cannot say it holds none.
    let holds_secrets = {
        let path = path.clone();
        tokio::task::spawn_blocking(move || {
            backup::read_manifest(&path)
                .map(|manifest| manifest.secrets == backup::SecretsMode::Included)
                .unwrap_or(true)
        })
        .await
        .unwrap_or(true)
    };
    if holds_secrets {
        return Err(AppError::Forbidden(
            "This backup holds secrets, or cannot say it holds none, so it is never offered \
             as a download. It stays on the server's disk."
                .into(),
        ));
    }
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFoundMessage("That backup is no longer there.".into()))?;
    let length = file.metadata().await.map(|meta| meta.len()).ok();
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, "application/zip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        )
        .header(header::CACHE_CONTROL, "no-store");
    if let Some(length) = length {
        response = response.header(header::CONTENT_LENGTH, length);
    }
    response
        .body(body)
        .map_err(|error| AppError::InternalError(format!("building response: {error}")))
}

pub fn routes() -> axum::Router<Arc<AppState>> {
    use axum::routing::{get, post, put};
    axum::Router::new()
        .route("/backups", get(get_backups))
        .route("/backups/status", get(get_backup_status))
        .route("/backups/settings", put(update_backup_settings))
        .route("/backups/run", post(run_backup))
        .route("/backups/stop", post(stop_backup))
        .route("/backups/check-destination", post(check_destination))
        .route("/backups/files/{location}/{name}", get(download_backup))
}

#[cfg(test)]
mod tests;
