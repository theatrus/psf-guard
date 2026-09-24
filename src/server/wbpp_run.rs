//! Running WBPP on a project or a target from inside PSF Guard.
//!
//! The run is the referenced WBPP export made real: PSF Guard plans the
//! frames, writes `run-wbpp.js` and its launchers into a work folder below
//! the cache, and starts PixInsight on it headless, with a virtual display
//! when the server has none. One run per database at a time, as the export
//! and import jobs are. Progress comes from WBPP's own log, which the run
//! reads every couple of seconds; the results are whatever WBPP wrote below
//! the work folder, listed and downloadable when it exits.
//!
//! Starting a run launches a program on the server, so it sits behind the
//! database-management gate, as does naming where PixInsight is.

use axum::{
    extract::{Path as AxumPath, State},
    http::header,
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::commands::export::wbpp::{
    WbppFiles, WbppOptions, WbppRun, WbppScriptSpec, OUTPUT_DIRECTORY,
};
use crate::commands::export::{plan_export, sanitize_component, ExportLayout, ExportOptions};
use crate::db_registry::{DbRegistry, PixInsightSettings};
use crate::pixinsight::{
    self, detect, display_plan, Detection, DisplayPlan, OutputFile, PixInsightInstall,
};
use crate::server::{
    api::ApiResponse,
    database_context::open_scheduler_connection_with_flags,
    extract::DbContext,
    handlers::{require_database_management_allowed, require_registry_path, AppError},
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
    /// The masters' save below the database's process directory: what was
    /// asked for at the start, and how it went.
    pub publish: Option<PublishOutcome>,
}

/// What happened to a run's masters when saved to the process directory.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PublishOutcome {
    /// `running`, `complete`, or `error`.
    pub state: String,
    /// The folder the masters went to.
    pub directory: String,
    pub copied: usize,
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
    cancel: Option<Arc<AtomicBool>>,
}

pub type SharedWbppRun = Arc<RwLock<WbppRunStore>>;

fn try_begin(
    store: &RwLock<WbppRunStore>,
    scope: String,
    options: WbppOptions,
) -> Option<Arc<AtomicBool>> {
    let mut s = store.write().unwrap();
    if s.progress.running {
        return None;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    s.progress = WbppRunProgress {
        running: true,
        stage: "planning".to_string(),
        scope,
        options: Some(options),
        started_at: Some(chrono::Utc::now().timestamp()),
        ..Default::default()
    };
    s.cancel = Some(cancel.clone());
    Some(cancel)
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
    /// database's export directory when it has one, else the cache.
    pub runs_dir: Option<String>,
    /// Bytes free where that folder is (or would be), when known.
    pub runs_dir_free_bytes: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdatePixInsightSettingsRequest {
    /// Empty or absent means look in the standard places.
    #[serde(default)]
    pub binary: Option<String>,
    /// Empty or absent means the export directory or the cache.
    #[serde(default)]
    pub runs_dir: Option<String>,
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn configured(state: &AppState) -> Result<PixInsightSettings, AppError> {
    let Ok(path) = require_registry_path(state) else {
        return Ok(PixInsightSettings::default());
    };
    let registry = DbRegistry::load_or_init(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    let settings = registry.pixinsight.unwrap_or_default();
    Ok(PixInsightSettings {
        binary: clean(settings.binary),
        runs_dir: clean(settings.runs_dir),
    })
}

fn settings_response(settings: PixInsightSettings) -> PixInsightSettingsResponse {
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
    }
}

/// Where a run's folder goes: what the request names, else the settings'
/// runs folder, else the database's export directory, else the cache.
/// Every root but the cache gets the database's slug below it, so two
/// databases' runs never share a folder.
pub fn run_root(
    requested: Option<&str>,
    settings_runs_dir: Option<&str>,
    export_dir: Option<&Path>,
    cache_dir: &Path,
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
    cache_dir.join("wbpp")
}

/// GET /api/settings/pixinsight
pub async fn get_pixinsight_settings(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<PixInsightSettingsResponse>>, AppError> {
    let settings = configured(&state)?;
    let response = tokio::task::spawn_blocking(move || settings_response(settings))
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
    let path = require_registry_path(&state)?;
    let _registry_guard = state.registry_write.lock().await;
    let mut registry = DbRegistry::load_or_init(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    let settings = PixInsightSettings {
        binary: clean(request.binary),
        runs_dir: clean(request.runs_dir),
    };
    // Absent is what the defaults already mean; storing nothing keeps the
    // registry clean for older builds reading the same file.
    registry.pixinsight = (settings != PixInsightSettings::default()).then(|| settings.clone());
    registry
        .save(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    let response = tokio::task::spawn_blocking(move || settings_response(settings))
        .await
        .map_err(|error| AppError::InternalError(format!("settings task: {error}")))?;
    Ok(Json(ApiResponse::success(response)))
}

// ----------------------------------------------------------------------------
// Runs
// ----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
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
pub fn publish_masters(
    output_dir: &Path,
    process_dir: &Path,
    folder: &str,
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
    for file in files {
        let Some(name) = file.file_name() else {
            continue;
        };
        let target = destination.join(name);
        let size = std::fs::metadata(&file).map(|meta| meta.len()).unwrap_or(0);
        if let Ok(existing) = std::fs::metadata(&target) {
            if existing.len() == size {
                outcome.skipped_existing += 1;
            } else {
                outcome.conflicts.push(name.to_string_lossy().into_owned());
            }
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
    outcome.state = if outcome.errors.is_empty() {
        "complete"
    } else {
        "error"
    }
    .to_string();
    outcome.finished_at = Some(chrono::Utc::now().timestamp());
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
    let outcome = match publish_masters(output_dir, process_dir, folder) {
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
    pub started: bool,
    pub progress: WbppRunProgress,
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
                    "Looked in {}. Name the executable under Settings → Setups → PixInsight.",
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
    format!("{}-{stamp}", sanitize_component(scope).replace(' ', "_"))
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
    require_database_management_allowed(&state)?;
    if req.project_id.is_none() && req.target_id.is_none() {
        return Err(AppError::BadRequest(
            "name a project_id or a target_id to stack".into(),
        ));
    }
    for param in &req.extra_params {
        if param.contains(',') || param.trim().is_empty() {
            return Err(AppError::BadRequest(format!(
                "'{param}' is not a WBPP parameter: one name=value, no commas"
            )));
        }
    }
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
        &ctx.cache_dir_path,
        &ctx.id,
    );
    if root.to_string_lossy().contains(',') {
        return Err(AppError::BadRequest(format!(
            "{} contains a comma, which WBPP's command line uses to separate parameters;              choose another run folder",
            root.display()
        )));
    }

    let scope = req
        .scope_label
        .clone()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| "selection".into());
    let publish_folder = match req.publish_folder.as_deref().map(str::trim) {
        Some(folder) if !folder.is_empty() => {
            if ctx.process_dir.is_none() {
                return Err(AppError::BadRequest(
                    "This database has no process directory to save masters to. Set one \
                     under Settings → Databases."
                        .into(),
                ));
            }
            Some(publish_folder_name(folder)?)
        }
        _ => None,
    };
    let store = ctx.0.wbpp_run.clone();
    let Some(cancel) = try_begin(&store, scope.clone(), req.options.clone()) else {
        return Ok(Json(ApiResponse::success(WbppRunStatusResponse {
            started: false,
            progress: progress_snapshot(&store),
        })));
    };

    let work_dir = root.join(run_dir_name(&scope));
    let output_dir = work_dir.join(OUTPUT_DIRECTORY);
    let free_bytes = pixinsight::free_bytes(&root);
    let project_id = req.project_id;
    update(&store, |progress| {
        progress.work_dir = work_dir.display().to_string();
        progress.output_dir = output_dir.display().to_string();
        progress.free_bytes_at_start = free_bytes;
        progress.project_id = project_id;
    });

    let options = ExportOptions {
        include_pending: req.include_pending,
        project_id: req.project_id,
        target_id: req.target_id,
        filter_name: req.filter_name.clone(),
        layout: ExportLayout::Wbpp,
        ..Default::default()
    };
    let job_ctx = ctx.0.clone();
    let job_store = store.clone();
    let wbpp_options = req.options.clone();
    let extra = req.extra_params.clone();

    tokio::spawn(async move {
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
        match outcome {
            Ok(stage) => finish(&job_store, stage, None),
            Err(error) => {
                tracing::warn!("WBPP run failed: {error:#}");
                finish(&job_store, "error", Some(format!("{error:#}")));
            }
        }
    });

    Ok(Json(ApiResponse::success(WbppRunStatusResponse {
        started: true,
        progress: progress_snapshot(&store),
    })))
}

/// The run itself: plan, write, launch, watch. Returns the closing stage.
#[allow(clippy::too_many_arguments)]
async fn run(
    ctx: Arc<crate::server::database_context::DatabaseContext>,
    store: SharedWbppRun,
    cancel: Arc<AtomicBool>,
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
                "no light frames to stack ({} catalog rows had missing files)",
                plan.missing.len()
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
        };
        crate::commands::export::write_wbpp_scripts(&plan, &plan_work_dir, &spec)?;
        let runner = plan_work_dir.join(crate::commands::export::wbpp::JS_RUNNER);
        let summary = (plan.items.len(), lights, plan.missing.len());
        let _ = &plan_store;
        Ok((summary, runner))
    })
    .await
    .map_err(|e| anyhow::anyhow!("planning task: {e}"))??;

    let (frames, lights, missing) = plan_summary;
    update(&store, |progress| {
        progress.frames = frames;
        progress.lights = lights;
        progress.missing_files = missing;
        progress.stage = "launching".to_string();
    });
    if cancel.load(Ordering::SeqCst) {
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
                if cancel.load(Ordering::SeqCst)
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
    let cancelled = cancel.load(Ordering::SeqCst);
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
            .unwrap_or_else(|| "WBPP's log names no error; read it in the run folder".into());
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

/// `GET /api/db/{db_id}/wbpp/runs/current` — the run's progress.
pub async fn get_wbpp_run(
    ctx: DbContext,
) -> Result<Json<ApiResponse<WbppRunStatusResponse>>, AppError> {
    let progress = progress_snapshot(&ctx.0.wbpp_run);
    Ok(Json(ApiResponse::success(WbppRunStatusResponse {
        started: progress.running,
        progress,
    })))
}

/// `DELETE /api/db/{db_id}/wbpp/runs/current` — stop the run.
pub async fn cancel_wbpp_run(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Result<Json<ApiResponse<WbppRunStatusResponse>>, AppError> {
    require_database_management_allowed(&state)?;
    let (running, cancel, pid) = {
        let s = ctx.0.wbpp_run.read().unwrap();
        (s.progress.running, s.cancel.clone(), s.progress.pid)
    };
    if running {
        if let Some(cancel) = cancel {
            cancel.store(true, Ordering::SeqCst);
        }
        if let Some(pid) = pid {
            terminate(pid);
        }
    }
    let progress = progress_snapshot(&ctx.0.wbpp_run);
    Ok(Json(ApiResponse::success(WbppRunStatusResponse {
        started: progress.running,
        progress,
    })))
}

/// `POST /api/db/{db_id}/wbpp/runs/current/publish` — save the finished
/// run's masters below the database's process directory.
pub async fn publish_wbpp_run(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<PublishRequest>,
) -> Result<Json<ApiResponse<WbppRunStatusResponse>>, AppError> {
    require_database_management_allowed(&state)?;
    let Some(process_dir) = ctx.process_dir.clone() else {
        return Err(AppError::BadRequest(
            "This database has no process directory to save masters to. Set one under \
             Settings → Databases."
                .into(),
        ));
    };
    let folder = publish_folder_name(&request.folder)?;
    let (output_dir, project_id, ready) = {
        let s = ctx.0.wbpp_run.read().unwrap();
        let p = &s.progress;
        let busy = p
            .publish
            .as_ref()
            .is_some_and(|publish| publish.state == "running");
        (
            PathBuf::from(&p.output_dir),
            p.project_id,
            !p.running && p.stage == "complete" && !busy,
        )
    };
    if !ready {
        return Err(AppError::Conflict(
            "the masters can be saved once a run has finished, and not while a save is under way"
                .into(),
        ));
    }
    update(&ctx.0.wbpp_run, |progress| {
        progress.publish = Some(PublishOutcome {
            state: "running".to_string(),
            directory: process_dir
                .join(&folder)
                .join("master")
                .display()
                .to_string(),
            ..Default::default()
        })
    });
    let publish_ctx = ctx.0.clone();
    let publish_store = ctx.0.wbpp_run.clone();
    tokio::task::spawn_blocking(move || {
        publish_and_record(
            &publish_ctx,
            &publish_store,
            &output_dir,
            &process_dir,
            &folder,
            project_id,
        )
    });
    let progress = progress_snapshot(&ctx.0.wbpp_run);
    Ok(Json(ApiResponse::success(WbppRunStatusResponse {
        started: progress.running,
        progress,
    })))
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
    if !path.is_file() {
        return Err(AppError::NotFoundMessage(format!(
            "{requested} is not a file of this run"
        )));
    }
    Ok(path)
}

/// `GET /api/db/{db_id}/wbpp/runs/current/files/{*path}` — a file the run
/// wrote: a master, a calibrated frame, a log, or the script itself.
pub async fn get_wbpp_run_file(
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
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
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
        assert!(try_begin(&store, "project A".into(), WbppOptions::default()).is_some());
        assert!(try_begin(&store, "project B".into(), WbppOptions::default()).is_none());
        finish(&store, "error", Some("boom".into()));
        assert_eq!(progress_snapshot(&store).stage, "error");
        assert!(try_begin(&store, "project B".into(), WbppOptions::default()).is_some());
        let progress = progress_snapshot(&store);
        assert_eq!(progress.scope, "project B");
        assert!(progress.error.is_none());
        assert!(progress.outputs.is_empty());
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
                .join("2026-iris-v1/master")
                .display()
                .to_string()
        );
        assert!(process
            .path()
            .join("2026-iris-v1/master/masterLight_L.xisf")
            .is_file());
        assert!(!process
            .path()
            .join("2026-iris-v1/master/masterLight_L.xisf.part")
            .exists());

        // The same masters again are recognised; a changed one is left alone.
        std::fs::write(out.join("master/masterBias.xisf"), vec![3u8; 250]).unwrap();
        let second = publish_masters(&out, process.path(), "2026-iris-v1").unwrap();
        assert_eq!(second.copied, 0);
        assert_eq!(second.skipped_existing, 1);
        assert_eq!(second.conflicts, vec!["masterBias.xisf"]);
        assert_eq!(
            std::fs::metadata(process.path().join("2026-iris-v1/master/masterBias.xisf"))
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
