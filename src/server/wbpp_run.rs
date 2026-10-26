//! Running WBPP on a project or a target from inside PSF Guard.
//!
//! The run is the referenced WBPP export made real: PSF Guard plans the
//! frames, writes `run-wbpp.js` and its launchers into a work folder below
//! the runs folder, and starts PixInsight on it headless, with a virtual display
//! when the server has none. One run per database at a time, as the export
//! and import jobs are. Progress comes from WBPP's own log, which the run
//! reads every couple of seconds; the results are whatever WBPP wrote below
//! the work folder, listed and downloadable when it exits.
//!
//! An editor starts, queues, cancels and publishes runs. The program and
//! the runs folder come from the PixInsight settings, which only the
//! database-management gate may change; a run folder outside that folder,
//! or reading a run's files, follows the same rules (see
//! [`start_wbpp_run`] and [`get_wbpp_run_file`]).

use axum::{
    extract::{Path as AxumPath, State},
    http::header,
    Json,
};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::commands::export::wbpp::{
    WbppFiles, WbppOptions, WbppRun, WbppScriptSpec, OUTPUT_DIRECTORY,
};
use crate::commands::export::{plan_export, sanitize_component, ExportLayout, ExportOptions};
use crate::concurrency::Stop;
use crate::db_registry::{DbRegistry, PixInsightSettings};
use crate::pixinsight::{
    self, detect, display_plan, Detection, DisplayPlan, OutputFile, PixInsightInstall,
};
use crate::server::wbpp_queue::QueuedRun;
use crate::server::{
    api::ApiResponse,
    database_context::open_scheduler_connection_with_flags,
    extract::DbContext,
    handlers::{
        require_database_management_allowed, require_registry_path, update_registry, AppError,
    },
    state::AppState,
};

/// How many log lines the status carries.
const LOG_TAIL: usize = 40;
/// How often the run re-reads WBPP's log while PixInsight runs.
const POLL: Duration = Duration::from_secs(2);
/// How long a cancelled PixInsight gets to exit before it is killed.
const GRACE: Duration = Duration::from_secs(10);

/// Progress of the (singleton per-database) WBPP run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WbppRunProgress {
    pub running: bool,
    /// `planning`, `launching`, `running`, `complete`, `error`, or
    /// `cancelled`; empty before the first run.
    pub stage: String,
    /// What was asked for, for display ("project Bubble").
    pub scope: String,
    /// The run's folder: the script, the launchers, and WBPP's output.
    pub work_dir: String,
    /// WBPP's output folder below it.
    pub output_dir: String,
    /// Bytes free where the run folder is, when the run began.
    pub free_bytes_at_start: Option<u64>,
    pub options: Option<WbppOptions>,
    /// Frames the plan named, and how many were lights.
    pub frames: usize,
    pub lights: usize,
    /// Catalog rows whose file was not found, so they are not in the run.
    pub missing_files: usize,
    /// Calibration frames the library keeps out: graded poor, kept out by
    /// hand, or marked for stray light. Runs recorded before grading called
    /// this `stray_light_darks`.
    #[serde(default, alias = "stray_light_darks")]
    pub kept_out_frames: usize,
    /// Lights left out because they can't be calibrated properly, as stacks
    /// leave them out when the calibration setting asks.
    #[serde(default)]
    pub uncalibrated_lights: usize,
    /// The calibration settings that chose the frames and differ from the
    /// defaults, in words with who set them; empty when the defaults did.
    #[serde(default)]
    pub calibration_settings: Vec<String>,
    /// The measured flat changes the lights' flats were chosen around, in
    /// words with their windows.
    #[serde(default)]
    pub flat_changes: Vec<String>,
    /// Lights whose flats are from across such a change, which WBPP may not
    /// calibrate as a PSF Guard stack would.
    #[serde(default)]
    pub flat_warnings: usize,
    /// The command line, as a person would type it.
    pub command: Option<String>,
    pub pid: Option<u32>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i32>,
    /// What WBPP's log says: its current step, how many it announced, and
    /// its own timing once it closed.
    pub wbpp_stage: Option<String>,
    pub wbpp_steps: usize,
    pub wbpp_elapsed: Option<String>,
    pub log_path: Option<String>,
    pub log_tail: Vec<String>,
    pub log_errors: Vec<String>,
    /// What WBPP wrote, masters first, once it exited.
    pub outputs: Vec<OutputFile>,
    pub error: Option<String>,
    /// The project the run stacked, when it was a project, so a save can
    /// remember its folder.
    pub project_id: Option<i32>,
    /// The target the run was for, when it was for one.
    #[serde(default)]
    pub target_id: Option<i32>,
    /// The masters' save below the database's process directory: what was
    /// asked for at the start, and how it went.
    pub publish: Option<PublishOutcome>,
    /// Taking the masters in as stacks for the Stacks view, once a project's
    /// run completes. PixInsight stays reserved until this finishes, so a
    /// page that showed `importing` can refresh its stacks on `done`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stacks: Option<StacksTakenIn>,
}

/// What became of a run's masters as stacks.
#[derive(Debug, Clone, Default, Serialize)]
pub struct StacksTakenIn {
    /// `importing`, `done`, or `error`.
    pub state: String,
    pub imported: usize,
    pub skipped: usize,
    /// Why nothing was taken in, such as a run across several targets.
    pub error: Option<String>,
}

/// What happened to a run's masters when saved to the process directory.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PublishOutcome {
    /// `running`, `complete`, or `error`.
    pub state: String,
    /// The folder the masters went to.
    pub directory: String,
    pub copied: usize,
    pub total: usize,
    pub current_file: Option<String>,
    /// Already there with the same size, so left alone.
    pub skipped_existing: usize,
    /// Already there with a different size; left alone and named here.
    pub conflicts: Vec<String>,
    pub errors: Vec<String>,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Default)]
pub struct WbppRunStore {
    pub progress: WbppRunProgress,
    /// Set by a cancel; read by the watcher to name the outcome.
    cancel: Option<Stop>,
    /// What the run under way was asked for, so the job journal can start it
    /// again after a restart cut it off.
    request: Option<StartWbppRunRequest>,
}

/// The target of the database's last run, when it had one.
pub(crate) fn last_target(store: &WbppRunStore) -> Option<i32> {
    store.progress.target_id
}

impl WbppRunStore {
    /// The request and PixInsight's pid of the run under way, if one is.
    pub fn running_request(&self) -> Option<(StartWbppRunRequest, Option<u32>)> {
        self.progress
            .running
            .then(|| {
                self.request
                    .clone()
                    .map(|request| (request, self.progress.pid))
            })
            .flatten()
    }
}

pub type SharedWbppRun = Arc<RwLock<WbppRunStore>>;

fn publish_running(progress: &WbppRunProgress) -> bool {
    progress
        .publish
        .as_ref()
        .is_some_and(|publish| publish.state == "running")
}

/// Claim the copy while holding the same lock that protects new runs.
fn begin_publish(
    store: &RwLock<WbppRunStore>,
    process_dir: &Path,
    folder: &str,
) -> Result<(PathBuf, Option<i32>), AppError> {
    let mut store = store.write().unwrap();
    let progress = &mut store.progress;
    if progress.running || progress.stage != "complete" || publish_running(progress) {
        return Err(AppError::Conflict(
            "Wait until the WBPP run and current copy finish before saving masters".into(),
        ));
    }
    progress.publish = Some(PublishOutcome {
        state: "running".into(),
        directory: process_dir
            .join(folder)
            .join("master")
            .display()
            .to_string(),
        ..Default::default()
    });
    Ok((PathBuf::from(&progress.output_dir), progress.project_id))
}

/// Mark a run as started on its database, stopped by `cancel`. False when
/// the database already has a run or a copy under way.
fn try_begin(
    store: &RwLock<WbppRunStore>,
    scope: String,
    options: WbppOptions,
    cancel: Stop,
) -> bool {
    let mut s = store.write().unwrap();
    if s.progress.running || publish_running(&s.progress) {
        return false;
    }
    s.progress = WbppRunProgress {
        running: true,
        stage: "planning".to_string(),
        scope,
        options: Some(options),
        started_at: Some(chrono::Utc::now().timestamp()),
        ..Default::default()
    };
    s.cancel = Some(cancel);
    true
}

fn update(store: &RwLock<WbppRunStore>, apply: impl FnOnce(&mut WbppRunProgress)) {
    let mut s = store.write().unwrap();
    apply(&mut s.progress);
}

fn finish(store: &RwLock<WbppRunStore>, stage: &str, error: Option<String>) {
    let mut s = store.write().unwrap();
    s.progress.running = false;
    s.progress.stage = stage.to_string();
    s.progress.error = error;
    s.progress.finished_at = Some(chrono::Utc::now().timestamp());
    s.cancel = None;
    s.request = None;
}

pub fn progress_snapshot(store: &RwLock<WbppRunStore>) -> WbppRunProgress {
    store.read().unwrap().progress.clone()
}

// ----------------------------------------------------------------------------
// PixInsight settings
// ----------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct PixInsightSettingsResponse {
    /// The executable the settings name, if any.
    pub binary: Option<String>,
    pub detection: Detection,
    pub display: DisplayPlan,
    /// Whether a run could start now: PixInsight found and a display
    /// available.
    pub ready: bool,
    /// The runs folder the settings name, if any. Absent means each
    /// database's export directory when it has one, else its stack root.
    pub runs_dir: Option<String>,
    /// Bytes free where that folder is (or would be), when known.
    pub runs_dir_free_bytes: Option<u64>,
    pub storage: crate::db_registry::WbppStoragePolicy,
    pub storage_report: super::wbpp_storage::Report,
}

#[derive(Debug, Deserialize)]
pub struct UpdatePixInsightSettingsRequest {
    /// Empty or absent means look in the standard places.
    #[serde(default)]
    pub binary: Option<String>,
    /// Empty or absent means the export directory or the stack root.
    #[serde(default)]
    pub runs_dir: Option<String>,
    #[serde(default)]
    pub storage: Option<crate::db_registry::WbppStoragePolicy>,
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn configured(state: &AppState) -> Result<PixInsightSettings, AppError> {
    let Ok(path) = require_registry_path(state) else {
        return Ok(PixInsightSettings::default());
    };
    let registry = DbRegistry::load_or_init(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    let settings = registry.pixinsight.unwrap_or_default();
    Ok(PixInsightSettings {
        binary: clean(settings.binary),
        runs_dir: clean(settings.runs_dir),
        storage: settings.storage,
    })
}

fn settings_response(
    settings: PixInsightSettings,
    storage_report: super::wbpp_storage::Report,
) -> PixInsightSettingsResponse {
    let detection = detect(settings.binary.as_deref());
    let display = display_plan();
    let ready = detection.install.is_some() && display != DisplayPlan::Missing;
    let runs_dir_free_bytes = settings
        .runs_dir
        .as_deref()
        .and_then(|dir| pixinsight::free_bytes(Path::new(dir)));
    PixInsightSettingsResponse {
        binary: settings.binary,
        detection,
        display,
        ready,
        runs_dir: settings.runs_dir,
        runs_dir_free_bytes,
        storage: settings.storage,
        storage_report,
    }
}

/// The folder runs go in when a request names none: the settings' runs
/// folder, else the database's export directory, else the stack root.
pub fn runs_folder(
    settings_runs_dir: Option<&str>,
    export_dir: Option<&Path>,
    stack_root: &Path,
) -> PathBuf {
    if let Some(root) = settings_runs_dir
        .map(str::trim)
        .filter(|root| !root.is_empty())
    {
        return PathBuf::from(root);
    }
    if let Some(export_dir) = export_dir {
        return export_dir.join("wbpp");
    }
    stack_root.join(crate::server::storage::WBPP_RUNS)
}

/// Whether `path` lies inside `folder`: lexically, with no `..` to climb
/// out, and once links are resolved in the part of it that exists.
fn within_folder(path: &Path, folder: &Path) -> bool {
    let plain = path.is_absolute()
        && !path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
        && path.starts_with(folder);
    if !plain {
        return false;
    }
    match dunce::canonicalize(folder) {
        Ok(folder) => path
            .ancestors()
            .find(|ancestor| ancestor.exists())
            .and_then(|ancestor| dunce::canonicalize(ancestor).ok())
            .is_some_and(|ancestor| ancestor.starts_with(folder)),
        // A runs folder not made yet holds no link to follow.
        Err(_) => true,
    }
}

/// Where a run's folder goes: what the request names, else the settings'
/// runs folder, else the database's export directory, else the stack root.
/// Every root but the stack root gets the database's slug below it, so two
/// databases' runs never share a folder.
pub fn run_root(
    requested: Option<&str>,
    settings_runs_dir: Option<&str>,
    export_dir: Option<&Path>,
    stack_root: &Path,
    db_id: &str,
) -> PathBuf {
    if let Some(root) = requested.map(str::trim).filter(|root| !root.is_empty()) {
        return PathBuf::from(root).join(db_id);
    }
    if let Some(root) = settings_runs_dir
        .map(str::trim)
        .filter(|root| !root.is_empty())
    {
        return PathBuf::from(root).join(db_id);
    }
    if let Some(export_dir) = export_dir {
        return export_dir.join("wbpp");
    }
    stack_root.join(crate::server::storage::WBPP_RUNS)
}

/// GET /api/settings/pixinsight
pub async fn get_pixinsight_settings(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<PixInsightSettingsResponse>>, AppError> {
    let settings = configured(&state)?;
    let storage_report = state.wbpp_storage.report();
    let response = tokio::task::spawn_blocking(move || settings_response(settings, storage_report))
        .await
        .map_err(|error| AppError::InternalError(format!("settings task: {error}")))?;
    Ok(Json(ApiResponse::success(response)))
}

/// PUT /api/settings/pixinsight
pub async fn update_pixinsight_settings(
    State(state): State<Arc<AppState>>,
    Json(request): Json<UpdatePixInsightSettingsRequest>,
) -> Result<Json<ApiResponse<PixInsightSettingsResponse>>, AppError> {
    require_database_management_allowed(&state)?;
    let settings = PixInsightSettings {
        binary: clean(request.binary),
        runs_dir: clean(request.runs_dir),
        storage: request.storage.unwrap_or(configured(&state)?.storage),
    };
    settings.storage.validate().map_err(AppError::BadRequest)?;
    if settings
        .runs_dir
        .as_deref()
        .is_some_and(|path| !Path::new(path).is_absolute())
    {
        return Err(AppError::BadRequest(
            "Use an absolute WBPP runs folder path".into(),
        ));
    }
    // Absent is what the defaults already mean; storing nothing keeps the
    // registry clean for older builds reading the same file.
    let stored = (settings != PixInsightSettings::default()).then(|| settings.clone());
    let old_state = state.clone();
    tokio::task::spawn_blocking(move || super::wbpp_storage::remember_roots(&old_state))
        .await
        .map_err(|error| AppError::InternalError(error.to_string()))?
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    update_registry(&state, |registry| {
        registry.pixinsight = stored;
        Ok(())
    })
    .await?;
    let changed_state = state.clone();
    tokio::task::spawn_blocking(move || super::wbpp_storage::policy_changed(&changed_state))
        .await
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    let storage_report = state.wbpp_storage.report();
    let response = tokio::task::spawn_blocking(move || settings_response(settings, storage_report))
        .await
        .map_err(|error| AppError::InternalError(format!("settings task: {error}")))?;
    Ok(Json(ApiResponse::success(response)))
}

// ----------------------------------------------------------------------------
// Runs
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StartWbppRunRequest {
    #[serde(default)]
    pub project_id: Option<i32>,
    #[serde(default)]
    pub target_id: Option<i32>,
    /// Count ungraded lights too. Rejects never go in.
    #[serde(default)]
    pub include_pending: bool,
    /// Restrict to one filter name (exact, case-insensitive).
    #[serde(default)]
    pub filter_name: Option<String>,
    #[serde(default)]
    pub options: WbppOptions,
    /// Further `name=value` WBPP automation parameters, passed as given.
    #[serde(default)]
    pub extra_params: Vec<String>,
    /// Display label for the progress line ("project Bubble").
    #[serde(default)]
    pub scope_label: Option<String>,
    /// Where this run's folder goes, overriding the settings for one run.
    #[serde(default)]
    pub work_root: Option<String>,
    /// Save the masters below the database's process directory when the run
    /// finishes, in this folder (`<process_dir>/<folder>/master/`).
    #[serde(default)]
    pub publish_folder: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PublishRequest {
    /// The folder below the process directory (`<process_dir>/<folder>/master/`).
    pub folder: String,
}

/// A processing folder name as a person types it, kept to one path
/// component: separators and control characters become `_`, and a name
/// that is nothing but dots or spaces is refused.
pub fn publish_folder_name(folder: &str) -> Result<String, AppError> {
    let cleaned = sanitize_component(folder.trim());
    if cleaned == "unnamed" || cleaned.is_empty() || cleaned.chars().all(|c| c == '.') {
        return Err(AppError::BadRequest(
            "name a folder for the masters, such as 2026-iris-v1".into(),
        ));
    }
    Ok(cleaned)
}

/// Copy a run's `master/` files to `<process_dir>/<folder>/master/`,
/// never over an existing file: one already there with the same size is
/// taken as the same file and skipped, one with another size is left alone
/// and reported. Each copy lands under a temporary name and is renamed
/// into place, so a failed copy leaves no half file.
#[cfg(test)]
fn publish_masters(
    output_dir: &Path,
    process_dir: &Path,
    folder: &str,
) -> Result<PublishOutcome, AppError> {
    publish_masters_with_progress(output_dir, process_dir, folder, |_| {})
}

fn publish_masters_with_progress(
    output_dir: &Path,
    process_dir: &Path,
    folder: &str,
    mut report: impl FnMut(&PublishOutcome),
) -> Result<PublishOutcome, AppError> {
    let folder = publish_folder_name(folder)?;
    let destination = process_dir.join(&folder).join("master");
    let source = output_dir.join("master");
    let mut outcome = PublishOutcome {
        state: "running".to_string(),
        directory: destination.display().to_string(),
        ..Default::default()
    };
    let entries = std::fs::read_dir(&source).map_err(|error| {
        AppError::BadRequest(format!(
            "the run has no master folder to save ({}: {error})",
            source.display()
        ))
    })?;
    std::fs::create_dir_all(&destination).map_err(|error| {
        AppError::InternalError(format!("creating {}: {error}", destination.display()))
    })?;
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    outcome.total = files.len();
    report(&outcome);
    for file in files {
        let Some(name) = file.file_name() else {
            continue;
        };
        outcome.current_file = Some(name.to_string_lossy().into_owned());
        report(&outcome);
        let target = destination.join(name);
        let size = std::fs::metadata(&file).map(|meta| meta.len()).unwrap_or(0);
        if let Ok(existing) = std::fs::metadata(&target) {
            if existing.len() == size {
                outcome.skipped_existing += 1;
            } else {
                outcome.conflicts.push(name.to_string_lossy().into_owned());
            }
            report(&outcome);
            continue;
        }
        let part = destination.join(format!("{}.part", name.to_string_lossy()));
        let copied = std::fs::copy(&file, &part).and_then(|_| std::fs::rename(&part, &target));
        match copied {
            Ok(()) => outcome.copied += 1,
            Err(error) => {
                let _ = std::fs::remove_file(&part);
                outcome
                    .errors
                    .push(format!("{}: {error}", name.to_string_lossy()));
            }
        }
    }
    outcome.current_file = None;
    outcome.state = if outcome.errors.is_empty() {
        "complete"
    } else {
        "error"
    }
    .to_string();
    outcome.finished_at = Some(chrono::Utc::now().timestamp());
    report(&outcome);
    Ok(outcome)
}

/// Save the current run's masters and remember the folder for the project.
/// Runs on a blocking thread and publishes its outcome into the progress.
fn publish_and_record(
    ctx: &crate::server::database_context::DatabaseContext,
    store: &RwLock<WbppRunStore>,
    output_dir: &Path,
    process_dir: &Path,
    folder: &str,
    project_id: Option<i32>,
) {
    let outcome = match publish_masters_with_progress(output_dir, process_dir, folder, |outcome| {
        // Keep the claim until the project setting has also been saved.
        let mut progress = outcome.clone();
        progress.state = "running".into();
        update(store, |p| p.publish = Some(progress));
    }) {
        Ok(outcome) => outcome,
        Err(error) => PublishOutcome {
            state: "error".to_string(),
            directory: process_dir
                .join(folder)
                .join("master")
                .display()
                .to_string(),
            errors: vec![format!("{error:?}")],
            finished_at: Some(chrono::Utc::now().timestamp()),
            ..Default::default()
        },
    };
    if outcome.state == "complete"
        && let Some(project_id) = project_id
        && let Ok(name) = publish_folder_name(folder)
    {
        let conn = ctx.db();
        if let Ok(mut conn) = conn.lock()
            && let Err(error) = crate::server::exposure_groups::remember_process_folder(
                &mut conn, project_id, &name,
            )
        {
            tracing::warn!("remembering the process folder for project {project_id}: {error:?}");
        }
    }
    update(store, |progress| progress.publish = Some(outcome));
}

#[derive(Debug, Serialize)]
pub struct WbppRunStatusResponse {
    /// POST: whether this request started PixInsight now. GET: whether a run
    /// is under way.
    pub started: bool,
    /// The id this request's run took in the line, when it was queued
    /// rather than started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_id: Option<String>,
    /// This database's runs waiting their turn, with their place in the
    /// server-wide line.
    pub queued: Vec<crate::server::wbpp_queue::QueuedRunSummary>,
    pub progress: WbppRunProgress,
}

fn status(
    state: &AppState,
    ctx: &crate::server::database_context::DatabaseContext,
    started: bool,
    queue_id: Option<String>,
) -> WbppRunStatusResponse {
    WbppRunStatusResponse {
        started,
        queue_id,
        queued: state.wbpp_queue.summaries_for(&ctx.id),
        progress: progress_snapshot(&ctx.wbpp_run),
    }
}

/// Whether the database has a run under way or is copying masters: a new
/// run would replace that progress.
fn database_busy(store: &RwLock<WbppRunStore>) -> bool {
    let store = store.read().unwrap();
    store.progress.running || publish_running(&store.progress)
}

/// A prepared run: the work from export to taking in the masters.
type PreparedRun = Pin<Box<dyn Future<Output = ()> + Send>>;

/// A queued run that could not start: shown as a failed run of its
/// database, so the reason is where the person will look.
fn record_failed_launch(
    store: &RwLock<WbppRunStore>,
    scope: String,
    project_id: Option<i32>,
    error: String,
) {
    let now = chrono::Utc::now().timestamp();
    let mut s = store.write().unwrap();
    if s.progress.running || publish_running(&s.progress) {
        return;
    }
    s.progress = WbppRunProgress {
        stage: "error".to_string(),
        scope,
        project_id,
        started_at: Some(now),
        finished_at: Some(now),
        error: Some(error),
        ..Default::default()
    };
}

/// The install a run would use, or why it cannot start.
fn resolve_install(
    state: &AppState,
) -> Result<(PixInsightInstall, DisplayPlan, PixInsightSettings), AppError> {
    let settings = configured(state)?;
    let detection = detect(settings.binary.as_deref());
    let install = match detection.install {
        Some(install) => install,
        None => {
            let mut message = String::from("PixInsight was not found. ");
            match detection.problem {
                Some(problem) => message.push_str(&problem),
                None => message.push_str(&format!(
                    "Looked in {}. Set the executable path under Settings → Exports & integrations → PixInsight.",
                    detection
                        .checked
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
            return Err(AppError::BadRequest(message));
        }
    };
    let display = display_plan();
    if display == DisplayPlan::Missing {
        return Err(AppError::BadRequest(
            "PixInsight needs a display and this server has none. Install xvfb (xvfb-run) \
             so it can run on a virtual one."
                .into(),
        ));
    }
    Ok((install, display, settings))
}

/// A folder name for a run: the scope, then the time, so runs sort.
fn run_dir_name(scope: &str) -> String {
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    format!(
        "{}-{stamp}-{}",
        sanitize_component(scope).replace(' ', "_"),
        uuid::Uuid::new_v4().simple()
    )
}

/// Kill PixInsight and whatever xvfb-run started with it.
fn terminate(pid: u32) {
    #[cfg(unix)]
    {
        // The child was started in its own process group, so the group id
        // is its pid; a negative pid signals the whole group.
        // SAFETY: kill(2) with a group id we created and still track.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status();
    }
}

#[cfg(unix)]
fn kill_hard(pid: u32) {
    // SAFETY: as above.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_hard(pid: u32) {
    terminate(pid);
}

/// Read WBPP's log into the progress, if it has one yet.
fn refresh_log(store: &RwLock<WbppRunStore>, output_dir: &Path) {
    let Some(log_path) = pixinsight::newest_log(output_dir) else {
        return;
    };
    let Ok((tail, summary)) = pixinsight::read_log(&log_path, LOG_TAIL) else {
        return;
    };
    update(store, |progress| {
        progress.log_path = Some(log_path.display().to_string());
        progress.log_tail = tail;
        progress.log_errors = summary.errors;
        progress.wbpp_stage = summary.stage;
        progress.wbpp_steps = summary.steps;
        progress.wbpp_elapsed = summary.elapsed;
    });
}

/// `POST /api/db/{db_id}/wbpp/runs` — stack a project or target with WBPP.
pub async fn start_wbpp_run(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(req): Json<StartWbppRunRequest>,
) -> Result<Json<ApiResponse<WbppRunStatusResponse>>, AppError> {
    if req.project_id.is_none() && req.target_id.is_none() {
        return Err(AppError::BadRequest(
            "name a project_id or a target_id to stack".into(),
        ));
    }
    // An editor runs WBPP in the folders the settings define. A run folder
    // of the client's choosing is a server path, which takes the
    // database-management gate.
    if !state.database_management_allowed()
        && let Some(requested) = req
            .work_root
            .as_deref()
            .map(str::trim)
            .filter(|root| !root.is_empty())
    {
        let settings = configured(&state)?;
        let runs = runs_folder(
            settings.runs_dir.as_deref(),
            ctx.export_dir.as_deref(),
            &ctx.stack_root,
        );
        if !within_folder(Path::new(requested), &runs) {
            return Err(AppError::Forbidden(format!(
                "{requested} is outside the PixInsight runs folder ({}); choosing another \
                 run folder needs database management",
                runs.display()
            )));
        }
    }
    for param in &req.extra_params {
        if param.contains(',') || param.trim().is_empty() {
            return Err(AppError::BadRequest(format!(
                "'{param}' is not a WBPP parameter: one name=value, no commas"
            )));
        }
    }
    if req
        .publish_folder
        .as_deref()
        .is_some_and(|folder| !folder.trim().is_empty())
    {
        if ctx.process_dir.is_none() {
            return Err(AppError::BadRequest(
                "This database has no process directory to save masters to. Set one \
                 under Settings → Databases."
                    .into(),
            ));
        }
        publish_folder_name(req.publish_folder.as_deref().unwrap_or_default())?;
    }

    // One PixInsight at a time, server-wide. A start runs now only when
    // PixInsight is free, no run is queued, and its database is not busy;
    // otherwise it joins the line. PixInsight is reserved in one step,
    // before anything awaits.
    let pixinsight = if database_busy(&ctx.wbpp_run) {
        None
    } else {
        state.wbpp_queue.try_reserve_pixinsight()
    };
    // Detection awaits a blocking task. A copy may start on this database
    // meanwhile; the request then joins the line.
    if let Some(pixinsight) = pixinsight
        && let Some(run) =
            prepare(Arc::clone(&state), ctx.0.clone(), req.clone(), Stop::new()).await?
    {
        tokio::spawn(async move {
            run.await;
            // PixInsight is free: the line's next run can start.
            drop(pixinsight);
        });
        return Ok(Json(ApiResponse::success(status(&state, &ctx, true, None))));
    }

    if let Some(existing) = state.wbpp_queue.find_same(&ctx.id, &req) {
        return Ok(Json(ApiResponse::success(status(
            &state,
            &ctx,
            false,
            Some(existing.id),
        ))));
    }
    let scope = scope_of(&req);
    let (queue_id, position) = state.wbpp_queue.push(
        &state,
        &ctx.id,
        scope.clone(),
        req,
        chrono::Utc::now().timestamp(),
    );
    tracing::info!(
        "🔭 WBPP run for db={} ({scope}) queued at position {position}",
        ctx.id
    );
    Ok(Json(ApiResponse::success(status(
        &state,
        &ctx,
        false,
        Some(queue_id),
    ))))
}

/// Whether the process group a run started is still alive. PixInsight runs
/// in a group of its own and can outlive the server that started it.
fn group_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only asks whether the group exists.
        unsafe { libc::kill(-(pid as i32), 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// A run a restart cut off is not started again on its own: hours of work
/// would begin from nothing after every restart, and a deploy-on-merge host
/// would never finish one. It shows on its database as stopped by the
/// restart, to start again by hand. If its PixInsight outlived the server,
/// PixInsight stays reserved until that process ends, so no other run
/// starts beside it.
pub(crate) fn record_cut_off(
    state: &Arc<AppState>,
    db_id: &str,
    request: &StartWbppRunRequest,
    pid: Option<u32>,
) {
    let Some(ctx) = state.get_database(db_id) else {
        return;
    };
    let still_running = pid.is_some_and(group_alive);
    record_failed_launch(
        &ctx.wbpp_run,
        scope_of(request),
        request.project_id,
        if still_running {
            "A server restart lost track of this run; its PixInsight is still working and \
             nothing else starts until it ends. Start the run again to collect its masters."
                .into()
        } else {
            "A server restart stopped this run. Start it again when you are ready.".into()
        },
    );
    tracing::info!(
        "🔭 WBPP run for db={db_id} ({}) was cut off by a restart{}",
        scope_of(request),
        if still_running {
            "; its PixInsight is still running"
        } else {
            ""
        }
    );
    if let (true, Some(pid)) = (still_running, pid)
        && let Some(pixinsight) = state.wbpp_queue.try_reserve_pixinsight_now()
    {
        tokio::spawn(async move {
            while group_alive(pid) {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
            drop(pixinsight);
        });
    }
}

/// Queue the runs that the job journal stored, next first. The line starts
/// the first one when PixInsight is free.
pub(crate) fn restore_queue(state: &Arc<AppState>, runs: Vec<(String, StartWbppRunRequest)>) {
    let now = chrono::Utc::now().timestamp();
    for (db_id, request) in runs {
        if state.get_database(&db_id).is_none() {
            continue;
        }
        let scope = scope_of(&request);
        state.wbpp_queue.push(state, &db_id, scope, request, now);
    }
}

pub(crate) fn scope_of(req: &StartWbppRunRequest) -> String {
    req.scope_label
        .clone()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| "selection".into())
}

/// Resolve the install, mark the run as started on its database, and return
/// the run to await. The caller holds PixInsight's lease until the run
/// ends. `Ok(None)` means the database already has a run or a copy under
/// way.
async fn prepare(
    state: Arc<AppState>,
    ctx: Arc<crate::server::database_context::DatabaseContext>,
    req: StartWbppRunRequest,
    cancel: Stop,
) -> Result<Option<PreparedRun>, AppError> {
    let (install, display, settings) = {
        let state = state.clone();
        tokio::task::spawn_blocking(move || resolve_install(&state))
            .await
            .map_err(|error| AppError::InternalError(format!("detect task: {error}")))??
    };
    let root = run_root(
        req.work_root.as_deref(),
        settings.runs_dir.as_deref(),
        ctx.export_dir.as_deref(),
        &ctx.stack_root,
        &ctx.id,
    );
    if root.to_string_lossy().contains(',') {
        return Err(AppError::BadRequest(format!(
            "{} contains a comma, which WBPP's command line uses to separate parameters; \
             choose another run folder",
            root.display()
        )));
    }

    let scope = scope_of(&req);
    let publish_folder = match req.publish_folder.as_deref().map(str::trim) {
        Some(folder) if !folder.is_empty() => Some(publish_folder_name(folder)?),
        _ => None,
    };
    let store = ctx.wbpp_run.clone();
    if !try_begin(&store, scope.clone(), req.options.clone(), cancel.clone()) {
        return Ok(None);
    }
    store.write().unwrap().request = Some(req.clone());

    let work_dir = root.join(run_dir_name(&scope));
    let output_dir = work_dir.join(OUTPUT_DIRECTORY);
    let free_bytes = pixinsight::free_bytes(&root);
    let project_id = req.project_id;
    let target_id = req.target_id;
    update(&store, |progress| {
        progress.work_dir = work_dir.display().to_string();
        progress.output_dir = output_dir.display().to_string();
        progress.free_bytes_at_start = free_bytes;
        progress.project_id = project_id;
        progress.target_id = target_id;
    });

    let options = ExportOptions {
        include_pending: req.include_pending,
        project_id: req.project_id,
        target_id: req.target_id,
        filter_name: req.filter_name.clone(),
        layout: ExportLayout::Wbpp,
        exclude_uncalibrated: crate::calibration::exclude_uncalibrated_enabled(),
        ..Default::default()
    };
    let job_ctx = ctx.clone();
    let job_store = store.clone();
    let wbpp_options = req.options.clone();
    let extra = req.extra_params.clone();

    let protected = state.wbpp_storage.protect(&work_dir);
    Ok(Some(Box::pin(async move {
        let _protected = protected;
        let storage_path = work_dir.clone();
        let start_state = state.clone();
        let start_path = work_dir.clone();
        let start_result = tokio::task::spawn_blocking(move || {
            super::wbpp_storage::started(&start_state, &start_path)
        })
        .await;
        if !matches!(start_result, Ok(Ok(()))) {
            finish(
                &job_store,
                "error",
                Some(format!(
                    "Could not record WBPP storage ownership: {start_result:?}"
                )),
            );
            return;
        }
        let import_output = output_dir.clone();
        let outcome = run(
            job_ctx.clone(),
            job_store.clone(),
            cancel,
            install,
            display,
            work_dir,
            output_dir.clone(),
            options,
            wbpp_options,
            extra,
        )
        .await;
        // A run asked to save its masters does so before it is reported
        // done, so the outcome is there when the status turns.
        if let (Ok("complete"), Some(folder), Some(process_dir)) = (
            &outcome,
            publish_folder.as_deref(),
            job_ctx.process_dir.clone(),
        ) {
            update(&job_store, |progress| {
                progress.publish = Some(PublishOutcome {
                    state: "running".to_string(),
                    directory: process_dir
                        .join(folder)
                        .join("master")
                        .display()
                        .to_string(),
                    ..Default::default()
                })
            });
            let folder = folder.to_string();
            let publish_ctx = job_ctx.clone();
            let publish_store = job_store.clone();
            let _ = tokio::task::spawn_blocking(move || {
                publish_and_record(
                    &publish_ctx,
                    &publish_store,
                    &output_dir,
                    &process_dir,
                    &folder,
                    project_id,
                )
            })
            .await;
        }
        let complete = matches!(outcome, Ok("complete"));
        match outcome {
            Ok(stage) => finish(&job_store, stage, None),
            Err(error) => {
                tracing::warn!("WBPP run failed: {error:#}");
                finish(&job_store, "error", Some(format!("{error:#}")));
            }
        }
        // The masters become stacks in the Stacks view, where PSF Guard
        // composes their color preview. The progress already shows the run
        // as finished. PixInsight stays reserved until the stacks are taken
        // in, so the next run cannot replace this progress before the page
        // shows them.
        if let (true, Some(project_id)) = (complete, project_id) {
            update(&job_store, |progress| {
                progress.stacks = Some(StacksTakenIn {
                    state: "importing".into(),
                    ..Default::default()
                });
            });
            let import_ctx = job_ctx.clone();
            let taken = tokio::task::spawn_blocking(move || {
                let target = {
                    let conn = import_ctx.db();
                    let conn = conn.lock().map_err(|error| error.to_string())?;
                    crate::server::stack_preview::wbpp_stacks::run_target(
                        &conn, project_id, target_id,
                    )?
                };
                crate::server::stack_preview::wbpp_stacks::import_masters(
                    &import_ctx,
                    project_id,
                    target,
                    &import_output,
                )
            })
            .await
            .unwrap_or_else(|error| Err(error.to_string()));
            let stacks = match taken {
                Ok(outcome) => {
                    tracing::info!(
                        "🔭 WBPP masters taken in as stacks for db={} project={project_id}: {} new, {} skipped",
                        job_ctx.id,
                        outcome.imported.len(),
                        outcome.skipped.len()
                    );
                    StacksTakenIn {
                        state: "done".into(),
                        imported: outcome.imported.len(),
                        skipped: outcome.skipped.len(),
                        error: None,
                    }
                }
                Err(reason) => {
                    tracing::info!(
                        "🔭 WBPP masters for db={} project={project_id} not taken in as stacks: {reason}",
                        job_ctx.id
                    );
                    StacksTakenIn {
                        state: "error".into(),
                        error: Some(reason),
                        ..Default::default()
                    }
                }
            };
            update(&job_store, |progress| progress.stacks = Some(stacks));
        }
        let _ = tokio::task::spawn_blocking(move || {
            if let Err(error) = super::wbpp_storage::finished(&storage_path) {
                tracing::warn!("Could not record finished WBPP run: {error}");
            }
        })
        .await;
        drop(_protected);
        super::wbpp_storage::request(&state);
    })))
}

/// Start a run that waited in the line, and return when it ends. A run that
/// cannot start (its database removed, PixInsight missing) is recorded as a
/// failed run on its database.
/// `stop` is the line entry's, so Stop on the entry reaches the run.
pub(crate) async fn start_queued(state: Arc<AppState>, next: QueuedRun, stop: Stop) {
    let Some(ctx) = state.get_database(&next.db_id) else {
        tracing::info!(
            "🔭 Queued WBPP run {} dropped: database {} is gone",
            next.scope,
            next.db_id
        );
        return;
    };
    let scope = next.scope.clone();
    let project_id = next.project_id;
    match prepare(Arc::clone(&state), ctx.clone(), next.request.clone(), stop).await {
        Ok(Some(run)) => {
            tracing::info!("🔭 Queued WBPP run for db={} ({scope}) started", ctx.id);
            run.await;
        }
        Ok(None) => {
            // A copy started on the database after the line chose this run.
            // The run goes back to the front of the line, blocked until the
            // copy ends.
            tracing::info!(
                "🔭 Queued WBPP run for db={} ({scope}) waits: the database is busy",
                ctx.id
            );
            let copying = state.wbpp_queue.hold_database(&ctx.id);
            state.wbpp_queue.push_front(next);
            tokio::spawn(async move {
                while database_busy(&ctx.wbpp_run) {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                drop(copying);
            });
        }
        Err(error) => {
            let message = match &error {
                AppError::BadRequest(message)
                | AppError::Conflict(message)
                | AppError::Forbidden(message)
                | AppError::Gone(message)
                | AppError::InternalError(message)
                | AppError::NotFoundMessage(message)
                | AppError::DatabaseError(message) => message.clone(),
                AppError::NotFound => "not found".to_string(),
                AppError::NotImplemented => "not implemented".to_string(),
            };
            tracing::warn!(
                "🔭 Queued WBPP run for db={} ({scope}) could not start: {message}",
                ctx.id
            );
            record_failed_launch(&ctx.wbpp_run, scope, project_id, message);
        }
    }
}

/// What a run's plan holds, for its progress.
struct PlanSummary {
    frames: usize,
    lights: usize,
    missing: usize,
    kept_out: usize,
    uncalibrated: usize,
    calibration_settings: Vec<String>,
    flat_changes: Vec<String>,
    flat_warnings: usize,
}

/// The run itself: plan, write, launch, watch. Returns the closing stage.
#[allow(clippy::too_many_arguments)]
async fn run(
    ctx: Arc<crate::server::database_context::DatabaseContext>,
    store: SharedWbppRun,
    cancel: Stop,
    install: PixInsightInstall,
    display: DisplayPlan,
    work_dir: PathBuf,
    output_dir: PathBuf,
    options: ExportOptions,
    wbpp_options: WbppOptions,
    extra: Vec<String>,
) -> anyhow::Result<&'static str> {
    // Plan on a blocking thread: it queries the catalog and walks the image
    // folders, which can take a while on network storage.
    let plan_ctx = ctx.clone();
    let plan_store = store.clone();
    let bpp_main = install.bpp_main.clone();
    let plan_work_dir = work_dir.clone();
    let plan_output_dir = output_dir.clone();
    let (plan_summary, runner) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let conn = open_scheduler_connection_with_flags(
            &plan_ctx.database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| anyhow::anyhow!("opening {}: {e}", plan_ctx.database_path))?;
        let plan = plan_export(&conn, &plan_ctx.image_dirs, &options)?;
        let lights = plan
            .items
            .iter()
            .filter(|item| item.kind == crate::commands::export::FrameKind::Light)
            .count();
        if lights == 0 {
            anyhow::bail!(
                "no light frames to stack ({} catalog rows had missing files, {} lights \
                 can't be calibrated)",
                plan.missing.len(),
                plan.uncalibrated.len()
            );
        }
        std::fs::create_dir_all(&plan_output_dir)
            .map_err(|e| anyhow::anyhow!("creating {}: {e}", plan_output_dir.display()))?;
        let spec = WbppScriptSpec {
            run: WbppRun::Full,
            files: WbppFiles::Referenced {
                local_root: None,
                remote_root: None,
            },
            options: wbpp_options,
            bpp_main: Some(bpp_main),
            stale_frames: Vec::new(),
        };
        crate::commands::export::write_wbpp_scripts(&plan, &plan_work_dir, &spec)?;
        let runner = plan_work_dir.join(crate::commands::export::wbpp::JS_RUNNER);
        let summary = PlanSummary {
            frames: plan.items.len(),
            lights,
            missing: plan.missing.len(),
            kept_out: plan.kept_out.len(),
            uncalibrated: plan.uncalibrated.len(),
            calibration_settings: plan.calibration_settings.clone(),
            flat_changes: plan.flat_changes.clone(),
            flat_warnings: plan.flat_warnings().len(),
        };
        let _ = &plan_store;
        Ok((summary, runner))
    })
    .await
    .map_err(|e| anyhow::anyhow!("planning task: {e}"))??;

    update(&store, |progress| {
        progress.frames = plan_summary.frames;
        progress.lights = plan_summary.lights;
        progress.missing_files = plan_summary.missing;
        progress.kept_out_frames = plan_summary.kept_out;
        progress.uncalibrated_lights = plan_summary.uncalibrated;
        progress.calibration_settings = plan_summary.calibration_settings;
        progress.flat_changes = plan_summary.flat_changes;
        progress.flat_warnings = plan_summary.flat_warnings;
        progress.stage = "launching".to_string();
    });
    if cancel.requested() {
        return Ok("cancelled");
    }

    let command = pixinsight::wbpp_command(&install, &display, &runner, &output_dir, &extra)?;
    let console = std::fs::File::create(work_dir.join("pixinsight.log"))
        .map_err(|e| anyhow::anyhow!("creating the console log: {e}"))?;
    let console_err = console
        .try_clone()
        .map_err(|e| anyhow::anyhow!("creating the console log: {e}"))?;
    let mut child_command = tokio::process::Command::new(&command.program);
    child_command
        .args(&command.args)
        .current_dir(&work_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(console))
        .stderr(std::process::Stdio::from(console_err))
        .kill_on_drop(false);
    #[cfg(unix)]
    child_command.process_group(0);
    let mut child = child_command
        .spawn()
        .map_err(|e| anyhow::anyhow!("starting {}: {e}", command.program.display()))?;
    let pid = child.id();
    if let Some(pid) = pid {
        let folder = work_dir.clone();
        let recorded =
            tokio::task::spawn_blocking(move || super::wbpp_storage::record_pid(&folder, pid))
                .await;
        if !matches!(recorded, Ok(Ok(()))) {
            kill_hard(pid);
            let _ = child.wait().await;
            anyhow::bail!("Could not record the WBPP process for storage protection: {recorded:?}");
        }
    }
    tracing::info!(
        "🔭 WBPP run db={} started pid={:?}: {}",
        ctx.id,
        pid,
        command.display_line
    );
    update(&store, |progress| {
        progress.stage = "running".to_string();
        progress.command = Some(command.display_line.clone());
        progress.pid = pid;
    });

    // Watch: re-read WBPP's log until PixInsight exits, and kill it on a
    // cancel, gently first.
    let mut cancel_seen_at: Option<tokio::time::Instant> = None;
    let status = loop {
        tokio::select! {
            status = child.wait() => break status?,
            _ = tokio::time::sleep(POLL) => {
                let out = output_dir.clone();
                let poll_store = store.clone();
                let _ = tokio::task::spawn_blocking(move || refresh_log(&poll_store, &out)).await;
                if cancel.requested()
                    && let Some(pid) = pid
                {
                    match cancel_seen_at {
                        None => {
                            cancel_seen_at = Some(tokio::time::Instant::now());
                            terminate(pid);
                        }
                        Some(at) if at.elapsed() > GRACE => kill_hard(pid),
                        Some(_) => {}
                    }
                }
            }
        }
    };

    let out = output_dir.clone();
    let final_store = store.clone();
    let outputs = tokio::task::spawn_blocking(move || {
        refresh_log(&final_store, &out);
        pixinsight::list_outputs(&out)
    })
    .await
    .unwrap_or_default();
    let exit_code = status.code();
    let cancelled = cancel.requested();
    let (finished, errors) = {
        let s = store.read().unwrap();
        (
            s.progress.wbpp_elapsed.is_some(),
            s.progress.log_errors.clone(),
        )
    };
    update(&store, |progress| {
        progress.exit_code = exit_code;
        progress.outputs = outputs.clone();
    });
    let masters = outputs.iter().filter(|file| file.kind == "master").count();
    tracing::info!(
        "🔭 WBPP run db={} exited code={:?} masters={} cancelled={}",
        ctx.id,
        exit_code,
        masters,
        cancelled
    );
    if cancelled {
        return Ok("cancelled");
    }
    if !status.success() {
        anyhow::bail!(
            "PixInsight exited with {}; see pixinsight.log in {}",
            exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "a signal".into()),
            work_dir.display()
        );
    }
    if masters == 0 {
        let hint = errors
            .last()
            .cloned()
            .unwrap_or_else(|| "WBPP's log shows no error; read it in the run folder".into());
        anyhow::bail!("WBPP wrote no master: {hint}");
    }
    if !finished {
        tracing::warn!(
            "WBPP run db={}: masters written but no closing time line",
            ctx.id
        );
    }
    Ok("complete")
}

/// `GET /api/db/{db_id}/wbpp/runs/current` — the run's progress and the
/// database's queued runs.
pub async fn get_wbpp_run(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Result<Json<ApiResponse<WbppRunStatusResponse>>, AppError> {
    let running = ctx.wbpp_run.read().unwrap().progress.running;
    Ok(Json(ApiResponse::success(status(
        &state, &ctx, running, None,
    ))))
}

/// `POST /api/db/{db_id}/wbpp/runs/current/dismiss` — clear a finished run
/// from view. The run folder stays on disk.
pub async fn dismiss_wbpp_run(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Result<Json<ApiResponse<WbppRunStatusResponse>>, AppError> {
    {
        let mut s = ctx.wbpp_run.write().unwrap();
        if s.progress.running {
            return Err(AppError::Conflict(
                "WBPP is still running; stop the run first".into(),
            ));
        }
        if s.progress
            .publish
            .as_ref()
            .is_some_and(|publish| publish.state == "running")
        {
            return Err(AppError::Conflict(
                "the masters are still being saved; try again in a moment".into(),
            ));
        }
        s.progress = WbppRunProgress::default();
    }
    Ok(Json(ApiResponse::success(status(
        &state, &ctx, false, None,
    ))))
}

/// `DELETE /api/db/{db_id}/wbpp/runs/queue/{queue_id}` — take a waiting run
/// out of the line.
pub async fn remove_queued_wbpp_run(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    AxumPath((_db_id, queue_id)): AxumPath<(String, String)>,
) -> Result<Json<ApiResponse<WbppRunStatusResponse>>, AppError> {
    if state.wbpp_queue.remove(&ctx.id, &queue_id).is_none() {
        return Err(AppError::NotFoundMessage(
            "that run is no longer waiting; it may have started".into(),
        ));
    }
    let running = ctx.wbpp_run.read().unwrap().progress.running;
    Ok(Json(ApiResponse::success(status(
        &state, &ctx, running, None,
    ))))
}

/// A WBPP run under way, for the header's queue.
#[derive(Debug, Clone, Serialize)]
pub struct WbppActiveRun {
    pub db_id: String,
    pub db_name: String,
    pub scope: String,
    /// What the run stacks, so the header can link to it.
    pub project_id: Option<i32>,
    pub target_id: Option<i32>,
    /// PSF Guard's stage: `planning`, `running`, `publishing`.
    pub stage: String,
    /// WBPP's own step, from its log.
    pub wbpp_stage: Option<String>,
    pub wbpp_steps: usize,
    pub started_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WbppQueuedActivity {
    pub db_id: String,
    pub db_name: String,
    #[serde(flatten)]
    pub run: crate::server::wbpp_queue::QueuedRunSummary,
}

#[derive(Debug, Clone, Serialize)]
pub struct WbppActivity {
    pub running: Vec<WbppActiveRun>,
    /// The whole line, next first.
    pub queued: Vec<WbppQueuedActivity>,
}

/// `GET /api/wbpp/activity` — WBPP runs under way and waiting, on every
/// database, for the header's background jobs.
pub async fn get_wbpp_activity(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<WbppActivity>> {
    Json(ApiResponse::success(wbpp_activity(&state)))
}

/// The WBPP runs under way and queued, on every database.
pub(crate) fn wbpp_activity(state: &AppState) -> WbppActivity {
    let databases = state.all_databases();
    let name_of = |db_id: &str| {
        databases
            .iter()
            .find(|ctx| ctx.id == db_id)
            .map(|ctx| ctx.name.clone())
            .unwrap_or_else(|| db_id.to_string())
    };
    let running = databases
        .iter()
        .filter_map(|ctx| {
            let store = ctx.wbpp_run.read().unwrap();
            let copying = publish_running(&store.progress);
            (store.progress.running || copying).then(|| WbppActiveRun {
                db_id: ctx.id.clone(),
                db_name: ctx.name.clone(),
                scope: store.progress.scope.clone(),
                project_id: store.progress.project_id,
                target_id: store.progress.target_id,
                stage: if copying {
                    "publishing".into()
                } else {
                    store.progress.stage.clone()
                },
                wbpp_stage: if copying {
                    store.progress.publish.as_ref().map(|p| {
                        format!(
                            "{} of {} files processed{}",
                            p.copied + p.skipped_existing + p.conflicts.len() + p.errors.len(),
                            p.total,
                            p.current_file
                                .as_ref()
                                .map(|file| format!(" · {file}"))
                                .unwrap_or_default()
                        )
                    })
                } else {
                    store.progress.wbpp_stage.clone()
                },
                wbpp_steps: if copying {
                    0
                } else {
                    store.progress.wbpp_steps
                },
                started_at: store.progress.started_at,
            })
        })
        .collect();
    let queued = state
        .wbpp_queue
        .all()
        .into_iter()
        .map(|(db_id, run)| WbppQueuedActivity {
            db_name: name_of(&db_id),
            db_id,
            run,
        })
        .collect();
    WbppActivity { running, queued }
}

#[derive(Debug, Deserialize)]
pub struct MoveQueuedRunRequest {
    /// Place in the line, 0 next.
    pub position: usize,
}

/// `POST /api/wbpp/queue/{queue_id}/move` — change when a waiting run starts.
pub async fn move_queued_wbpp_run(
    State(state): State<Arc<AppState>>,
    AxumPath(queue_id): AxumPath<String>,
    Json(request): Json<MoveQueuedRunRequest>,
) -> Result<Json<ApiResponse<WbppActivity>>, AppError> {
    if !state.wbpp_queue.move_to(&queue_id, request.position) {
        return Err(AppError::NotFound);
    }
    Ok(get_wbpp_activity(State(state)).await)
}

/// `DELETE /api/db/{db_id}/wbpp/runs/current` — stop the run.
pub async fn cancel_wbpp_run(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Result<Json<ApiResponse<WbppRunStatusResponse>>, AppError> {
    let (running, cancel, pid) = {
        let s = ctx.0.wbpp_run.read().unwrap();
        (s.progress.running, s.cancel.clone(), s.progress.pid)
    };
    if running {
        if let Some(cancel) = cancel {
            cancel.request();
        }
        if let Some(pid) = pid {
            terminate(pid);
        }
    }
    let running = ctx.wbpp_run.read().unwrap().progress.running;
    Ok(Json(ApiResponse::success(status(
        &state, &ctx, running, None,
    ))))
}

/// `POST /api/db/{db_id}/wbpp/runs/current/publish` — save the finished
/// run's masters below the database's process directory.
pub async fn publish_wbpp_run(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<PublishRequest>,
) -> Result<Json<ApiResponse<WbppRunStatusResponse>>, AppError> {
    let Some(process_dir) = ctx.process_dir.clone() else {
        return Err(AppError::BadRequest(
            "This database has no process directory to save masters to. Set one under \
             Settings → Databases."
                .into(),
        ));
    };
    let folder = publish_folder_name(&request.folder)?;
    // The database's queued runs wait until the copy ends: a run would
    // replace the copy's progress.
    let copying = state.wbpp_queue.hold_database(&ctx.id);
    let (output_dir, project_id) = begin_publish(&ctx.wbpp_run, &process_dir, &folder)?;
    let protected = state
        .wbpp_storage
        .protect(output_dir.parent().unwrap_or(&output_dir));
    let publish_ctx = ctx.0.clone();
    let publish_store = ctx.0.wbpp_run.clone();
    let failed_store = publish_store.clone();
    let priority = state.begin_named_interactive_job("copying WBPP masters");
    tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            let _priority = priority;
            let _protected = protected;
            publish_and_record(
                &publish_ctx,
                &publish_store,
                &output_dir,
                &process_dir,
                &folder,
                project_id,
            )
        })
        .await;
        if let Err(error) = result {
            update(&failed_store, |p| {
                if let Some(publish) = p.publish.as_mut() {
                    publish.state = "error".into();
                    publish.current_file = None;
                    publish.errors.push(format!("Copy stopped: {error}"));
                    publish.finished_at = Some(chrono::Utc::now().timestamp());
                }
            });
        }
        drop(copying);
    });
    let running = ctx.wbpp_run.read().unwrap().progress.running;
    Ok(Json(ApiResponse::success(status(
        &state, &ctx, running, None,
    ))))
}

/// A path below the run's work folder, refused if it climbs out.
fn file_below(work_dir: &Path, requested: &str) -> Result<PathBuf, AppError> {
    let relative = Path::new(requested);
    if relative.is_absolute()
        || relative.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err(AppError::BadRequest(
            "path must stay below the run folder".into(),
        ));
    }
    let path = work_dir.join(relative);
    // A link inside the run folder may point anywhere: the file must still
    // be inside once links are resolved.
    let inside = match (dunce::canonicalize(&path), dunce::canonicalize(work_dir)) {
        (Ok(file), Ok(folder)) => file.starts_with(folder) && file.is_file(),
        _ => false,
    };
    if !inside {
        return Err(AppError::NotFoundMessage(format!(
            "{requested} is not a file of this run"
        )));
    }
    Ok(path)
}

/// `GET /api/db/{db_id}/wbpp/runs/current/files/{*path}` — a file the run
/// wrote: a master, a calibrated frame, a log, or the script itself.
pub async fn get_wbpp_run_file(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    AxumPath((_db_id, requested)): AxumPath<(String, String)>,
) -> Result<axum::response::Response, AppError> {
    let work_dir = {
        let s = ctx.0.wbpp_run.read().unwrap();
        s.progress.work_dir.clone()
    };
    if work_dir.is_empty() {
        return Err(AppError::NotFoundMessage("no WBPP run yet".into()));
    }
    let protected = state.wbpp_storage.protect(Path::new(&work_dir));
    let path = file_below(Path::new(&work_dir), &requested)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let content_type = match path.extension().and_then(|ext| ext.to_str()) {
        Some("xisf") => "application/octet-stream",
        Some("fits") | Some("fit") => "application/fits",
        Some("log") | Some("txt") | Some("js") | Some("sh") | Some("cmd") => {
            "text/plain; charset=utf-8"
        }
        _ => "application/octet-stream",
    };
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| AppError::InternalError(format!("opening {}: {e}", path.display())))?;
    let body = crate::server::file_body::file_body_with_guard(file, protected);
    axum::response::Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", name.replace('"', "")),
        )
        .body(body)
        .map_err(|e| AppError::InternalError(format!("building response: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_is_a_singleton_that_resets_prior_state() {
        let store = RwLock::new(WbppRunStore::default());
        assert!(try_begin(
            &store,
            "project A".into(),
            WbppOptions::default(),
            Stop::new()
        ));
        assert!(!try_begin(
            &store,
            "project B".into(),
            WbppOptions::default(),
            Stop::new()
        ));
        finish(&store, "error", Some("boom".into()));
        assert_eq!(progress_snapshot(&store).stage, "error");
        assert!(try_begin(
            &store,
            "project B".into(),
            WbppOptions::default(),
            Stop::new()
        ));
        let progress = progress_snapshot(&store);
        assert_eq!(progress.scope, "project B");
        assert!(progress.error.is_none());
        assert!(progress.outputs.is_empty());
    }

    fn state_with_management() -> (Arc<AppState>, DbContext) {
        let state = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        state.set_allow_database_management(true);
        let ctx = DbContext(state.all_databases().remove(0));
        (state, ctx)
    }

    fn request(project_id: i32, label: &str) -> StartWbppRunRequest {
        StartWbppRunRequest {
            project_id: Some(project_id),
            scope_label: Some(label.into()),
            ..Default::default()
        }
    }

    // These platforms can resolve an install without DISPLAY or xvfb.
    // The fixture never launches PixInsight.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[tokio::test]
    async fn a_copy_during_detection_queues_the_start_request() {
        let (state, ctx) = state_with_management();
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("bin/PixInsight");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"unused").unwrap();
        let script = dir
            .path()
            .join(crate::commands::export::wbpp::BPP_MAIN_BELOW_INSTALL);
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(script, b"// fixture").unwrap();
        let path = dir.path().join("registry.json");
        let registry = DbRegistry {
            pixinsight: Some(PixInsightSettings {
                binary: Some(binary.display().to_string()),
                runs_dir: None,
                ..Default::default()
            }),
            ..Default::default()
        };
        registry.save(&path).unwrap();
        state.set_registry_path(Some(path));
        finish(&ctx.wbpp_run, "complete", None);
        let mut req = request(2, "new run");
        req.include_pending = true;
        req.extra_params = vec!["maxStars=500".into()];
        let mut starting = Box::pin(start_wbpp_run(
            State(state.clone()),
            DbContext(ctx.0.clone()),
            Json(req),
        ));
        let copying;
        {
            // Poll through the PixInsight reservation, then block detection
            // at the registry read while a copy starts on the database, as
            // the publish route starts one.
            let _registry_lock = state.registry_path.write().unwrap();
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(starting.as_mut().poll(&mut context).is_pending());
            copying = state.wbpp_queue.hold_database(&ctx.id);
            begin_publish(&ctx.wbpp_run, dir.path(), "saved").unwrap();
        }
        let response = starting.await.unwrap().0.data.unwrap();
        assert!(!response.started);
        assert_eq!(response.queued.len(), 1);
        assert_eq!(
            response.queue_id.as_deref(),
            Some(response.queued[0].id.as_str())
        );
        assert_eq!(response.queued[0].scope, "new run");
        assert_eq!(response.queued[0].project_id, Some(2));
        let queued = state.wbpp_queue.journal_entries().remove(0);
        assert!(queued.request.include_pending);
        assert_eq!(queued.request.extra_params, vec!["maxStars=500"]);
        assert!(publish_running(&ctx.wbpp_run.read().unwrap().progress));
        drop(copying);
    }

    #[tokio::test]
    async fn copying_appears_in_activity_and_holds_new_runs_in_the_queue() {
        let (state, ctx) = state_with_management();
        finish(&ctx.wbpp_run, "complete", None);
        // As the publish route starts a copy.
        let _copying = state.wbpp_queue.hold_database(&ctx.id);
        begin_publish(&ctx.wbpp_run, Path::new("/process"), "Iris").unwrap();
        update(&ctx.wbpp_run, |p| {
            let copy = p.publish.as_mut().unwrap();
            copy.total = 3;
            copy.copied = 1;
            copy.current_file = Some("masterLight.xisf".into());
        });
        let activity = get_wbpp_activity(State(state.clone()))
            .await
            .0
            .data
            .unwrap();
        assert_eq!(activity.running.len(), 1);
        assert_eq!(activity.running[0].stage, "publishing");
        assert_eq!(
            activity.running[0].wbpp_stage.as_deref(),
            Some("1 of 3 files processed · masterLight.xisf")
        );
        assert!(
            dismiss_wbpp_run(State(state.clone()), DbContext(ctx.0.clone()))
                .await
                .is_err()
        );
        let queued = start_wbpp_run(
            State(state.clone()),
            DbContext(ctx.0.clone()),
            Json(request(2, "next")),
        )
        .await
        .unwrap()
        .0
        .data
        .unwrap();
        assert!(!queued.started);
        assert!(queued.queue_id.is_some());
        assert_eq!(state.wbpp_queue.len(), 1);
        assert!(publish_running(&ctx.wbpp_run.read().unwrap().progress));
    }

    #[tokio::test]
    async fn a_dropped_reservation_frees_pixinsight() {
        let (state, _ctx) = state_with_management();
        let pixinsight = state
            .wbpp_queue
            .try_reserve_pixinsight()
            .expect("free at first");
        assert!(
            state.wbpp_queue.try_reserve_pixinsight().is_none(),
            "one reservation at a time"
        );
        // A request dropped while PixInsight is detected drops its
        // reservation.
        drop(pixinsight);
        assert!(
            state.wbpp_queue.try_reserve_pixinsight().is_some(),
            "PixInsight is free again"
        );
    }

    #[tokio::test]
    async fn a_start_while_pixinsight_is_reserved_queues_and_the_line_keeps_it() {
        let (state, ctx) = state_with_management();
        // A run has reserved PixInsight but has not marked its database yet:
        // the window two starts used to slip through together.
        let _pixinsight = state.wbpp_queue.try_reserve_pixinsight().unwrap();
        let queued = start_wbpp_run(
            State(Arc::clone(&state)),
            DbContext(ctx.0.clone()),
            Json(request(3, "IC 447")),
        )
        .await
        .unwrap()
        .0
        .data
        .unwrap();
        assert!(!queued.started);
        assert!(queued.queue_id.is_some(), "it waits rather than launching");

        // While PixInsight is reserved, the run stays in line.
        tokio::task::yield_now().await;
        assert_eq!(state.wbpp_queue.len(), 1);
        assert!(state.wbpp_queue.try_reserve_pixinsight().is_none());
    }

    #[tokio::test]
    async fn a_start_while_pixinsight_is_busy_joins_the_line_once_and_can_leave_it() {
        let (state, ctx) = state_with_management();
        // A run under way on this database, as PixInsight would leave it.
        let _pixinsight = state.wbpp_queue.try_reserve_pixinsight().unwrap();
        assert!(try_begin(
            &ctx.wbpp_run,
            "Sh2 86".into(),
            WbppOptions::default(),
            Stop::new()
        ));
        update(&ctx.wbpp_run, |progress| progress.project_id = Some(1));

        let queued = start_wbpp_run(
            State(Arc::clone(&state)),
            DbContext(ctx.0.clone()),
            Json(request(2, "NGC 6820")),
        )
        .await
        .unwrap()
        .0
        .data
        .unwrap();
        assert!(!queued.started);
        let queue_id = queued.queue_id.clone().expect("queued, not started");
        assert_eq!(queued.queued.len(), 1);
        assert_eq!(queued.queued[0].position, 1);
        assert_eq!(queued.queued[0].scope, "NGC 6820");
        assert_eq!(
            queued.progress.scope, "Sh2 86",
            "the running run is untouched"
        );

        // The same project again does not queue twice.
        let again = start_wbpp_run(
            State(Arc::clone(&state)),
            DbContext(ctx.0.clone()),
            Json(request(2, "NGC 6820")),
        )
        .await
        .unwrap()
        .0
        .data
        .unwrap();
        assert_eq!(again.queue_id.as_deref(), Some(queue_id.as_str()));
        assert_eq!(state.wbpp_queue.len(), 1);

        // Dismissing is refused while the run is under way.
        assert!(matches!(
            dismiss_wbpp_run(State(Arc::clone(&state)), DbContext(ctx.0.clone())).await,
            Err(AppError::Conflict(_))
        ));

        let left = remove_queued_wbpp_run(
            State(Arc::clone(&state)),
            DbContext(ctx.0.clone()),
            AxumPath(("test".into(), queue_id.clone())),
        )
        .await
        .unwrap()
        .0
        .data
        .unwrap();
        assert!(left.queued.is_empty());
        assert!(matches!(
            remove_queued_wbpp_run(
                State(Arc::clone(&state)),
                DbContext(ctx.0.clone()),
                AxumPath(("test".into(), queue_id)),
            )
            .await,
            Err(AppError::NotFoundMessage(_))
        ));

        // Once the run ends, dismissing clears it from view.
        finish(&ctx.wbpp_run, "complete", None);
        let cleared = dismiss_wbpp_run(State(Arc::clone(&state)), DbContext(ctx.0.clone()))
            .await
            .unwrap()
            .0
            .data
            .unwrap();
        assert_eq!(cleared.progress.stage, "");
        assert!(cleared.progress.finished_at.is_none());
    }

    #[test]
    fn a_queued_run_that_cannot_start_is_shown_as_a_failed_run() {
        let store = RwLock::new(WbppRunStore::default());
        record_failed_launch(
            &store,
            "M31".into(),
            Some(3),
            "PixInsight is not installed".into(),
        );
        let progress = progress_snapshot(&store);
        assert_eq!(progress.stage, "error");
        assert_eq!(progress.scope, "M31");
        assert_eq!(progress.project_id, Some(3));
        assert!(!progress.running);
        assert!(progress.finished_at.is_some());
        // Never over a run under way.
        assert!(try_begin(
            &store,
            "live".into(),
            WbppOptions::default(),
            Stop::new()
        ));
        record_failed_launch(&store, "M31".into(), None, "x".into());
        assert_eq!(progress_snapshot(&store).scope, "live");
    }

    #[test]
    fn files_stay_below_the_run_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("wbpp-out/master")).unwrap();
        std::fs::write(dir.path().join("wbpp-out/master/m.xisf"), b"x").unwrap();
        assert!(file_below(dir.path(), "wbpp-out/master/m.xisf").is_ok());
        assert!(file_below(dir.path(), "../secret").is_err());
        assert!(file_below(dir.path(), "/etc/passwd").is_err());
        assert!(file_below(dir.path(), "wbpp-out/master/none.xisf").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_run_folder_is_not_a_file_of_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("log.txt"),
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("out")).unwrap();
        assert!(file_below(dir.path(), "log.txt").is_err());
        assert!(file_below(dir.path(), "out/secret.txt").is_err());
        // A link that stays inside is fine.
        std::fs::write(dir.path().join("real.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real.txt"), dir.path().join("alias.txt"))
            .unwrap();
        assert!(file_below(dir.path(), "alias.txt").is_ok());
    }

    #[test]
    fn a_requested_run_folder_must_stay_in_the_runs_folder() {
        let runs = tempfile::tempdir().unwrap();
        let inside = runs.path().join("tonight");
        assert!(within_folder(&inside, runs.path()), "not made yet");
        std::fs::create_dir_all(&inside).unwrap();
        assert!(within_folder(&inside, runs.path()));
        assert!(!within_folder(
            &runs.path().join("..").join("x"),
            runs.path()
        ));
        assert!(!within_folder(Path::new("relative/runs"), runs.path()));
        let elsewhere = tempfile::tempdir().unwrap();
        assert!(!within_folder(elsewhere.path(), runs.path()));
        #[cfg(unix)]
        {
            // A link inside the runs folder that leads out of it.
            std::os::unix::fs::symlink(elsewhere.path(), runs.path().join("away")).unwrap();
            assert!(!within_folder(&runs.path().join("away"), runs.path()));
            assert!(!within_folder(
                &runs.path().join("away").join("new"),
                runs.path()
            ));
        }
    }

    #[tokio::test]
    async fn without_management_an_editor_runs_only_in_the_runs_folder() {
        let state = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        // A real folder for the runs: the test context's stack root,
        // "/tmp/psf-guard-test", has no drive on Windows, so nothing there
        // counts as inside it.
        let exports = tempfile::tempdir().unwrap();
        let mut context = (*state.all_databases().remove(0)).clone();
        context.export_dir = Some(exports.path().to_path_buf());
        let ctx = DbContext(Arc::new(context));
        assert!(!state.database_management_allowed());
        // Reserve PixInsight, so an accepted start joins the line rather
        // than looking for PixInsight.
        let _pixinsight = state.wbpp_queue.try_reserve_pixinsight().unwrap();
        let runs = runs_folder(None, ctx.export_dir.as_deref(), &ctx.stack_root);
        let start = |work_root: Option<String>| {
            let state = Arc::clone(&state);
            let ctx = DbContext(ctx.0.clone());
            async move {
                start_wbpp_run(
                    State(state),
                    ctx,
                    Json(StartWbppRunRequest {
                        work_root,
                        ..request(3, "IC 447")
                    }),
                )
                .await
            }
        };
        assert!(start(None)
            .await
            .unwrap()
            .0
            .data
            .unwrap()
            .queue_id
            .is_some());
        let tonight = runs.join("tonight").display().to_string();
        assert!(start(Some(tonight)).await.is_ok());
        let elsewhere = tempfile::tempdir().unwrap();
        let refused = start(Some(elsewhere.path().display().to_string())).await;
        assert!(
            matches!(refused, Err(AppError::Forbidden(_))),
            "a run folder outside the runs folder needs the gate"
        );

        // With the gate, a run folder anywhere is the operator's choice.
        state.set_allow_database_management(true);
        assert!(start(Some(elsewhere.path().display().to_string()))
            .await
            .is_ok());
    }

    #[test]
    fn the_run_root_prefers_the_request_then_settings_then_export_then_cache() {
        let cache = Path::new("/var/cache/psf-guard/db-1");
        let export = Path::new("/mnt/nas/exports");
        assert_eq!(
            run_root(
                Some(" /data/runs "),
                Some("/srv/runs"),
                Some(export),
                cache,
                "db-1"
            ),
            PathBuf::from("/data/runs/db-1")
        );
        assert_eq!(
            run_root(None, Some("/srv/runs"), Some(export), cache, "db-1"),
            PathBuf::from("/srv/runs/db-1")
        );
        assert_eq!(
            run_root(Some(""), None, Some(export), cache, "db-1"),
            PathBuf::from("/mnt/nas/exports/wbpp")
        );
        assert_eq!(
            run_root(None, None, None, cache, "db-1"),
            PathBuf::from("/var/cache/psf-guard/db-1/wbpp")
        );
    }

    #[test]
    fn copy_claim_blocks_repeat_requests_and_new_runs() {
        let store = RwLock::new(WbppRunStore::default());
        finish(&store, "complete", None);
        std::thread::scope(|scope| {
            let attempts: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| begin_publish(&store, Path::new("/process"), "Iris").is_ok())
                })
                .collect();
            assert_eq!(
                attempts
                    .into_iter()
                    .map(|attempt| attempt.join().unwrap())
                    .filter(|claimed| *claimed)
                    .count(),
                1
            );
        });
        assert!(!try_begin(
            &store,
            "next".into(),
            WbppOptions::default(),
            Stop::new()
        ));
        update(&store, |p| {
            p.publish.as_mut().unwrap().state = "complete".into()
        });
        assert!(try_begin(
            &store,
            "next".into(),
            WbppOptions::default(),
            Stop::new()
        ));
    }

    #[test]
    fn copy_reports_each_file_and_final_counts() {
        let run = tempfile::tempdir().unwrap();
        let source = run.path().join("master");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("a.xisf"), b"first").unwrap();
        std::fs::write(source.join("b.xisf"), b"second").unwrap();
        let process = tempfile::tempdir().unwrap();
        let mut reports = Vec::new();
        let outcome = publish_masters_with_progress(run.path(), process.path(), "Iris", |p| {
            reports.push(p.clone())
        })
        .unwrap();
        assert_eq!(outcome.copied, 2);
        assert_eq!(outcome.total, 2);
        assert_eq!(reports[0].state, "running");
        assert!(reports
            .iter()
            .any(|p| p.current_file.as_deref() == Some("a.xisf") && p.copied == 0));
        assert!(reports
            .iter()
            .any(|p| p.current_file.as_deref() == Some("b.xisf") && p.copied == 1));
        assert_eq!(reports.last().unwrap().state, "complete");
        assert!(reports.last().unwrap().current_file.is_none());
        assert_eq!(
            std::fs::read(process.path().join("Iris/master/b.xisf")).unwrap(),
            b"second"
        );
    }

    #[test]
    fn masters_are_saved_once_and_never_over_a_different_file() {
        let run = tempfile::tempdir().unwrap();
        let out = run.path().join("wbpp-out");
        std::fs::create_dir_all(out.join("master")).unwrap();
        std::fs::write(out.join("master/masterLight_L.xisf"), vec![1u8; 300]).unwrap();
        std::fs::write(out.join("master/masterBias.xisf"), vec![2u8; 200]).unwrap();
        let process = tempfile::tempdir().unwrap();

        let first = publish_masters(&out, process.path(), " 2026-iris-v1 ").unwrap();
        assert_eq!(first.state, "complete");
        assert_eq!(first.copied, 2);
        assert_eq!(
            first.directory,
            process
                .path()
                .join("2026-iris-v1")
                .join("master")
                .display()
                .to_string()
        );
        assert!(process
            .path()
            .join("2026-iris-v1")
            .join("master")
            .join("masterLight_L.xisf")
            .is_file());
        assert!(!process
            .path()
            .join("2026-iris-v1")
            .join("master")
            .join("masterLight_L.xisf.part")
            .exists());

        // The same masters again are recognised; a changed one is left alone.
        std::fs::write(out.join("master/masterBias.xisf"), vec![3u8; 250]).unwrap();
        let second = publish_masters(&out, process.path(), "2026-iris-v1").unwrap();
        assert_eq!(second.copied, 0);
        assert_eq!(second.skipped_existing, 1);
        assert_eq!(second.conflicts, vec!["masterBias.xisf"]);
        assert_eq!(
            std::fs::metadata(
                process
                    .path()
                    .join("2026-iris-v1")
                    .join("master")
                    .join("masterBias.xisf")
            )
            .unwrap()
            .len(),
            200
        );

        assert!(publish_masters(&out, process.path(), "..").is_err());
        assert!(publish_masters(&out, process.path(), "   ").is_err());
        assert_eq!(publish_folder_name("Iris / v2").unwrap(), "Iris _ v2");
        let empty = tempfile::tempdir().unwrap();
        assert!(publish_masters(empty.path(), process.path(), "x").is_err());
    }

    #[test]
    fn run_folders_are_named_by_scope_and_time() {
        let name = run_dir_name("NGC 7023 / Iris");
        assert!(name.starts_with("NGC_7023_"), "{name}");
        assert!(name.contains("Iris-"), "{name}");
        assert!(!name.contains('/') && !name.contains(' '), "{name}");
    }
}
