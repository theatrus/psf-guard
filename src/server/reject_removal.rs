//! The HTTP side of reject removal (docs/design/reject-removal.md): preview
//! and apply a removal, list removals, restore them, and empty the trash.
//!
//! Preview, Remove, Restore and Empty trash read the image folders and move
//! or delete files, which on a network share takes minutes, so they are a
//! tracked job (see `docs/design/background-jobs.md`): one runner per
//! catalog, at [`Priority::Interactive`], since a person starts each. A
//! route queues the run and answers `202` with the status; `GET
//! …/rejects/removal` says where it stands, and `POST …/rejects/removal/stop`
//! stops it between frames. Listing removals and purging their saved rows
//! touch only the database, and answer directly.
//!
//! Every route that changes anything needs an editor. Files move only
//! between an image folder and its own trash, so the database-management
//! gate, which guards client-named paths, does not apply.

use std::{collections::BTreeSet, sync::Arc, time::Instant};

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::commands::reject_removal::{
    self, ApplyReport, PlanOptions, PurgeReport, RemovalPlan, RemovedBatch, RemovedEntry,
    RemovedFrame, RestoreReport, RestoreSelection, Scope, Stage, StaleRemovalPlan, Stopped,
    TrashReport, Work,
};
use crate::concurrency::{Priority, Stop};
use crate::server::{
    api::ApiResponse,
    database_context::{open_scheduler_connection, DatabaseContext},
    extract::DbContext,
    handlers::AppError,
    job_line::catalog::{CatalogJob, CatalogWork, Step},
    state::AppState,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewRequest {
    #[serde(default)]
    pub project_id: Option<i64>,
    #[serde(default)]
    pub target_id: Option<i64>,
    #[serde(default = "default_min_age")]
    pub min_age_days: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyRequest {
    #[serde(default)]
    pub project_id: Option<i64>,
    #[serde(default)]
    pub target_id: Option<i64>,
    pub min_age_days: u32,
    /// The preview's digest; Apply refuses when the rejects changed since.
    pub digest: String,
    #[serde(default = "default_retention")]
    pub retention_days: u32,
}

fn default_min_age() -> u32 {
    reject_removal::DEFAULT_MIN_AGE_DAYS
}

fn default_retention() -> u32 {
    reject_removal::DEFAULT_RETENTION_DAYS
}

#[derive(Debug, Deserialize)]
pub struct RemovedQuery {
    pub batch: Option<String>,
}

#[derive(serde::Serialize)]
pub struct RemovedView {
    pub batches: Vec<RemovedBatch>,
    /// The frames of the requested batch; empty when no batch was requested.
    pub frames: Vec<RemovedEntry>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// What a run of the removal job does.
#[derive(Debug, Clone)]
enum Action {
    Preview(PreviewRequest),
    Remove(ApplyRequest),
    Restore(RestoreSelection),
    EmptyTrash,
}

impl Action {
    fn name(&self) -> &'static str {
        match self {
            Self::Preview(_) => "preview",
            Self::Remove(_) => "remove",
            Self::Restore(_) => "restore",
            Self::EmptyTrash => "empty_trash",
        }
    }

    /// What background work waits for while this runs.
    fn waited_for(&self) -> &'static str {
        match self {
            Self::Preview(_) => "a reject removal preview",
            Self::Remove(_) => "a reject removal",
            Self::Restore(_) => "restoring removed rejects",
            Self::EmptyTrash => "emptying the reject trash",
        }
    }
}

/// What the last Remove, Restore or Empty trash did, and its run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RemovalOutcome {
    pub run: u64,
    pub finished_at: i64,
    /// `remove`, `restore` or `empty_trash`.
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<ApplyReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored: Option<RestoreReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emptied: Option<TrashReport>,
}

/// Where a catalog's reject removal job stands.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RemovalJobStatus {
    /// `idle` before the first run, `running`, then `done`, `stopped` or
    /// `failed`.
    pub state: String,
    /// What the run is, as pages and the header name it, such as
    /// `Removing rejects`.
    pub title: String,
    /// The run's stage and count, such as `Moving to the trash 37 of 203`.
    pub label: String,
    /// 0–100 for the run's stage, or `None` before its size is known.
    pub percent: Option<f64>,
    /// `preview`, `remove`, `restore` or `empty_trash`: what runs, or ran
    /// last.
    pub action: Option<String>,
    /// `indexing`, `checking`, `moving`, `restoring` or `deleting`, while it
    /// runs.
    pub stage: Option<String>,
    pub done: usize,
    pub total: usize,
    /// Counts up with each run, so a page knows which result is its own.
    pub run: u64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    /// The latest preview. A removal that applied leaves none.
    pub plan: Option<RemovalPlan>,
    /// A removal found the rejects changed since its preview, applied
    /// nothing, and left the new preview in `plan`.
    pub changed: bool,
    /// What the last Remove, Restore or Empty trash did.
    pub outcome: Option<RemovalOutcome>,
    pub error: Option<String>,
    /// `interactive` while it runs: a person starts every run.
    pub priority: Option<String>,
}

/// Reject removal, run on one thread per catalog.
pub(crate) struct RejectRemoval;

type Job = CatalogJob<RejectRemoval>;

/// The catalog's removal run. Only one run per catalog is active at a time.
#[derive(Default)]
pub(crate) struct Queue {
    /// The requested run, until the runner thread starts it.
    pending: Option<PendingRun>,
    /// Whether a run is pending or running. A second request fails with a
    /// conflict until the run ends.
    run_active: bool,
}

/// A run that a person requested.
pub(crate) struct PendingRun {
    action: Action,
    ctx: Arc<DatabaseContext>,
    protected: BTreeSet<String>,
    /// The server's async runtime, which runs the refreshes that a change
    /// starts.
    runtime: tokio::runtime::Handle,
}

impl CatalogWork for RejectRemoval {
    type Queue = Queue;
    type Status = RemovalJobStatus;
    type Item = PendingRun;
    const RUNNER: &'static str = "reject-removal";
    /// Each run registers its own shutdown hold, labelled with its action.
    const SHUTDOWN_WAITS_FOR: Option<&'static str> = None;

    fn next(_: &AppState, _: &str, queue: &mut Queue, _: Instant) -> Step<PendingRun> {
        queue.pending.take().map_or(Step::Exit, Step::Run)
    }

    fn run(state: &Arc<AppState>, _: &str, job: &Job, pending: PendingRun, stop: &Stop) {
        let _runtime = pending.runtime.enter();
        run(
            state,
            &pending.ctx,
            job,
            stop,
            &pending.action,
            &pending.protected,
        );
    }

    fn runner_failed(queue: &mut Queue, status: &mut RemovalJobStatus) {
        queue.pending = None;
        queue.run_active = false;
        if status.state == "running" {
            status.state = "failed".into();
            status.stage = None;
            status.error = Some("the run ended unexpectedly".into());
        }
    }
}

fn job(state: &AppState, path: &str) -> Arc<Job> {
    state.catalog_jobs.reject_removal.job(path)
}

fn progress(job: &Job, stage: Stage, done: usize, total: usize) {
    job.update(|status| {
        status.stage = Some(stage.name().into());
        status.done = done;
        status.total = total;
    });
}

/// Where the reject removal job stands for the catalog at `path`.
pub fn status(state: &AppState, path: &str) -> RemovalJobStatus {
    let mut status = job(state, path).status();
    if status.state.is_empty() {
        status.state = "idle".into();
    }
    status.title = title(status.action.as_deref()).into();
    status.label = label(status.stage.as_deref(), status.done, status.total);
    status.percent = crate::server::job_text::percent(status.done, status.total);
    status
}

/// What a run is, such as `Removing rejects`.
pub fn title(action: Option<&str>) -> &'static str {
    match action {
        Some("remove") => "Removing rejects",
        Some("restore") => "Restoring rejects",
        Some("empty_trash") => "Emptying the reject trash",
        _ => "Previewing reject removal",
    }
}

/// A run's stage and count, such as `Moving to the trash 37 of 203`.
pub fn label(stage: Option<&str>, done: usize, total: usize) -> String {
    let verb = match stage {
        Some("indexing") => "Indexing folders",
        Some("checking") => "Checking files",
        Some("moving") => "Moving to the trash",
        Some("restoring") => "Restoring",
        Some("deleting") => "Deleting",
        _ => "Starting",
    };
    crate::server::job_text::stage_count(verb, done, total)
}

/// Stop the run between frames: the frames done stay done.
pub fn stop(state: &AppState, path: &str) -> RemovalJobStatus {
    let job = job(state, path);
    job.stop(|queue| {
        // A run that is pending, and not started by the runner thread, is
        // removed so it never starts. The stop of a running run, or of one
        // that is finishing, is requested as well.
        if queue.pending.take().is_some() {
            queue.run_active = false;
            job.update(|status| {
                status.state = "stopped".into();
                status.stage = None;
                status.priority = None;
                status.finished_at = Some(now());
            });
        }
    });
    status(state, path)
}

/// Frames a collaboration report names: none without Director.
async fn protected(state: &AppState) -> Result<BTreeSet<String>, AppError> {
    match state.director.clone() {
        Some(service) => service.reported_image_guids().await.map_err(|error| {
            AppError::InternalError(format!("reading collaboration reports: {error}"))
        }),
        None => Ok(BTreeSet::new()),
    }
}

/// Start a run on the catalog's runner, or refuse while one runs.
fn start(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    action: Action,
    protected: BTreeSet<String>,
) -> Result<RemovalJobStatus, AppError> {
    let job = job(state, &ctx.database_path);
    state
        .catalog_jobs
        .reject_removal
        .request(state, &ctx.database_path, |queue| {
            if queue.run_active {
                return Err(AppError::Conflict(
                    "Reject removal is already running on this catalog. Wait for it, or stop it."
                        .into(),
                ));
            }
            queue.run_active = true;
            job.update(|status| {
                status.run += 1;
                status.state = "running".into();
                status.action = Some(action.name().into());
                status.stage = None;
                status.done = 0;
                status.total = 0;
                status.started_at = Some(now());
                status.finished_at = None;
                status.changed = false;
                status.error = None;
                status.priority = Some(Priority::Interactive.label().into());
            });
            queue.pending = Some(PendingRun {
                action,
                ctx: Arc::clone(ctx),
                protected,
                runtime: tokio::runtime::Handle::current(),
            });
            Ok(())
        })?;
    Ok(job.status())
}

/// How a run ended short of its result.
enum Ended {
    Stopped,
    Failed(String),
}

fn failed(error: impl std::fmt::Display) -> Ended {
    Ended::Failed(error.to_string())
}

/// A core error: a stop, or a failure in words.
fn ended(error: anyhow::Error) -> Ended {
    if error.downcast_ref::<Stopped>().is_some() {
        Ended::Stopped
    } else {
        Ended::Failed(format!("{error:#}"))
    }
}

fn run(
    state: &Arc<AppState>,
    ctx: &DatabaseContext,
    job: &Job,
    stop: &Stop,
    action: &Action,
    protected: &BTreeSet<String>,
) {
    let began = std::time::Instant::now();
    let result = {
        // A person started this: background work steps aside, and says what
        // for. A server stop waits for the frame in hand, then the run ends.
        let _person = state.begin_named_interactive_job(action.waited_for());
        let _hold = state.shutdown.hold(action.waited_for());
        let progress = |stage: Stage, done: usize, total: usize| progress(job, stage, done, total);
        let stop = || stop.requested() || state.shutdown.is_stopping();
        let work = Work {
            progress: &progress,
            stop: &stop,
        };
        match action {
            Action::Preview(request) => preview_run(ctx, job, request, protected, &work),
            Action::Remove(request) => remove_run(state, ctx, job, request, protected, &work),
            Action::Restore(selection) => restore_run(state, ctx, job, selection, &work),
            Action::EmptyTrash => empty_run(ctx, job, &work),
        }
    };
    // The runner is free as the status says the run ended: a page that sees
    // the end may start the next run at once.
    let mut queue = job.queue();
    queue.run_active = false;
    job.update(|status| {
        status.finished_at = Some(now());
        status.stage = None;
        status.priority = None;
        match &result {
            Ok(true) => status.state = "done".into(),
            Ok(false) | Err(Ended::Stopped) => status.state = "stopped".into(),
            Err(Ended::Failed(error)) => {
                status.state = "failed".into();
                status.error = Some(error.clone());
            }
        }
    });
    let status = job.status();
    drop(queue);
    tracing::info!(
        db = %ctx.id,
        "reject removal: {} {} in {:.2}s{}",
        action.name(),
        status.state,
        began.elapsed().as_secs_f64(),
        status
            .error
            .as_deref()
            .map(|error| format!(": {error}"))
            .unwrap_or_default()
    );
}

fn plan_options<'a>(
    project_id: Option<i64>,
    target_id: Option<i64>,
    min_age_days: u32,
    protected: &'a BTreeSet<String>,
) -> PlanOptions<'a> {
    PlanOptions {
        scope: Scope {
            project_id,
            target_id,
        },
        min_age_days,
        now: now(),
        protected_guids: protected,
    }
}

/// What a removal of this scope would take, and why each reject stays.
/// Returns whether it finished.
fn preview_run(
    ctx: &DatabaseContext,
    job: &Job,
    request: &PreviewRequest,
    protected: &BTreeSet<String>,
    work: &Work<'_>,
) -> Result<bool, Ended> {
    let conn = open_scheduler_connection(&ctx.database_path).map_err(failed)?;
    let plan = reject_removal::plan_with(
        &conn,
        &ctx.image_dirs,
        &plan_options(
            request.project_id,
            request.target_id,
            request.min_age_days,
            protected,
        ),
        work,
    )
    .map_err(ended)?;
    job.update(|status| status.plan = Some(plan));
    Ok(true)
}

/// Remove what the preview showed. When the rejects changed since, remove
/// nothing and leave the new preview, saying so.
fn remove_run(
    state: &Arc<AppState>,
    ctx: &DatabaseContext,
    job: &Job,
    request: &ApplyRequest,
    protected: &BTreeSet<String>,
    work: &Work<'_>,
) -> Result<bool, Ended> {
    let options = plan_options(
        request.project_id,
        request.target_id,
        request.min_age_days,
        protected,
    );
    let applied = {
        let _guard = ctx.organization_mutex.lock().map_err(failed)?;
        let conn = open_scheduler_connection(&ctx.database_path).map_err(failed)?;
        let applied = reject_removal::apply_with(
            &conn,
            &ctx.image_dirs,
            &options,
            &request.digest,
            request.retention_days,
            work,
        );
        if let Ok(report) = &applied {
            after_change(ctx, &conn, &report.removed);
        }
        applied
    };
    let report = match applied {
        Ok(report) => report,
        Err(error) if error.downcast_ref::<StaleRemovalPlan>().is_some() => {
            let conn = open_scheduler_connection(&ctx.database_path).map_err(failed)?;
            let fresh =
                reject_removal::plan_with(&conn, &ctx.image_dirs, &options, work).map_err(ended)?;
            job.update(|status| {
                status.plan = Some(fresh);
                status.changed = true;
            });
            return Ok(true);
        }
        Err(error) => return Err(ended(error)),
    };
    tracing::info!(db = %ctx.id, batch = %report.batch_id, removed = report.removed.len(),
        failed = report.failed.len(), stopped = report.stopped, "Removed rejects");
    if !report.removed.is_empty() {
        state.auto_stacks.touch_database(
            &ctx.id,
            crate::server::stack_preview::automatic::RefreshReason::Sync,
        );
    }
    let finished = !report.stopped;
    let run = job.status().run;
    job.update(|status| {
        status.plan = None;
        status.outcome = Some(RemovalOutcome {
            run,
            finished_at: now(),
            action: "remove".into(),
            removed: Some(report),
            ..Default::default()
        });
    });
    Ok(finished)
}

/// What the server holds about frames that left or came back: their cache
/// files and pixel evidence, the file lookups, and the navigation counts.
fn after_change(context: &DatabaseContext, conn: &rusqlite::Connection, frames: &[RemovedFrame]) {
    if frames.is_empty() {
        return;
    }
    reject_removal::forget_image_caches(&context.cache_dir_path, frames);
    for frame in frames {
        if let Ok(id) = i32::try_from(frame.image_id) {
            crate::server::spatial_scan::invalidate_image_source(
                &context.spatial_metrics,
                &context.cache_dir_path,
                id,
            );
        }
    }
    context.file_check_cache.write().unwrap().clear();
    let _ = context.force_directory_tree_refresh();
    if let Err(error) = context.refresh_organization_navigation(conn) {
        tracing::warn!(db = %context.id, "Reject removal succeeded but navigation refresh failed: {error}");
    }
}

/// Put removed frames back while their files are still in the trash.
fn restore_run(
    state: &Arc<AppState>,
    ctx: &DatabaseContext,
    job: &Job,
    selection: &RestoreSelection,
    work: &Work<'_>,
) -> Result<bool, Ended> {
    let report = {
        let _guard = ctx.organization_mutex.lock().map_err(failed)?;
        let conn = open_scheduler_connection(&ctx.database_path).map_err(failed)?;
        let report = reject_removal::restore_with(&conn, selection, work).map_err(ended)?;
        // A restored frame can come back under a new row Id; whatever the
        // cache holds under that Id belongs to no one.
        let restored: Vec<RemovedFrame> = report
            .restored
            .iter()
            .map(|frame| RemovedFrame {
                image_id: frame.image_id,
                guid: frame.guid.clone(),
                project_id: frame.project_id,
                target_id: frame.target_id,
            })
            .collect();
        after_change(ctx, &conn, &restored);
        report
    };
    if !report.restored.is_empty() {
        state.auto_stacks.touch_database(
            &ctx.id,
            crate::server::stack_preview::automatic::RefreshReason::Sync,
        );
    }
    let finished = !report.stopped;
    let run = job.status().run;
    job.update(|status| {
        status.outcome = Some(RemovalOutcome {
            run,
            finished_at: now(),
            action: "restore".into(),
            restored: Some(report),
            ..Default::default()
        });
    });
    Ok(finished)
}

/// Delete the files of removals past their retention.
fn empty_run(ctx: &DatabaseContext, job: &Job, work: &Work<'_>) -> Result<bool, Ended> {
    let conn = open_scheduler_connection(&ctx.database_path).map_err(failed)?;
    let report = reject_removal::empty_trash_with(&conn, now(), work).map_err(ended)?;
    let finished = !report.stopped;
    let run = job.status().run;
    job.update(|status| {
        status.outcome = Some(RemovalOutcome {
            run,
            finished_at: now(),
            action: "empty_trash".into(),
            emptied: Some(report),
            ..Default::default()
        });
    });
    Ok(finished)
}

type Started = Result<(StatusCode, Json<ApiResponse<RemovalJobStatus>>), AppError>;

fn accepted(status: RemovalJobStatus) -> Started {
    Ok((StatusCode::ACCEPTED, Json(ApiResponse::success(status))))
}

/// `GET /api/db/{db}/rejects/removal` — where the catalog's reject removal
/// job stands, with the latest preview and outcome. Never does the work.
pub async fn get_status(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Json<ApiResponse<RemovalJobStatus>> {
    Json(ApiResponse::success(status(&state, &ctx.database_path)))
}

/// `POST /api/db/{db}/rejects/removal/preview` — work out what a removal of
/// this scope would take: `202` and the status, which the page polls. It
/// changes nothing but the rejects' recorded dates.
pub async fn preview(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<PreviewRequest>,
) -> Started {
    let protected = protected(&state).await?;
    accepted(start(&state, &ctx.0, Action::Preview(request), protected)?)
}

/// `POST /api/db/{db}/rejects/removal/apply` — remove what the preview
/// showed: `202` and the status. When the rejects changed since, the run
/// removes nothing, sets `changed` and leaves the new preview.
pub async fn apply(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<ApplyRequest>,
) -> Started {
    let protected = protected(&state).await?;
    accepted(start(&state, &ctx.0, Action::Remove(request), protected)?)
}

/// `POST /api/db/{db}/rejects/removal/stop` — stop the run between frames.
pub async fn stop_removal(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Json<ApiResponse<RemovalJobStatus>> {
    Json(ApiResponse::success(stop(&state, &ctx.database_path)))
}

/// Every removal batch, and the frames of one when pending.
pub async fn removed(
    ctx: DbContext,
    Query(query): Query<RemovedQuery>,
) -> Result<Json<ApiResponse<RemovedView>>, AppError> {
    let path = ctx.database_path.clone();
    let view = tokio::task::spawn_blocking(move || {
        let conn = open_scheduler_connection(&path).map_err(AppError::db)?;
        let batches = reject_removal::batches(&conn).map_err(AppError::db)?;
        let frames = match &query.batch {
            Some(batch) => reject_removal::removed(&conn, Some(batch)).map_err(AppError::db)?,
            None => Vec::new(),
        };
        Ok::<_, AppError>(RemovedView { batches, frames })
    })
    .await
    .map_err(|error| AppError::InternalError(format!("reject removal: {error}")))??;
    Ok(Json(ApiResponse::success(view)))
}

/// `POST /api/db/{db}/rejects/removed/restore` — put removed frames back
/// while their files are still in the trash: `202` and the status.
pub async fn restore(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(selection): Json<RestoreSelection>,
) -> Started {
    if selection.batch_id.is_none() && selection.guids.is_empty() {
        return Err(AppError::BadRequest(
            "Choose a batch or frames to restore.".into(),
        ));
    }
    accepted(start(
        &state,
        &ctx.0,
        Action::Restore(selection),
        BTreeSet::new(),
    )?)
}

/// `POST /api/db/{db}/rejects/trash/empty` — delete the files of removals
/// past their retention: `202` and the status.
pub async fn empty_trash(State(state): State<Arc<AppState>>, ctx: DbContext) -> Started {
    accepted(start(&state, &ctx.0, Action::EmptyTrash, BTreeSet::new())?)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PurgeRequest {
    #[serde(default)]
    pub batch_id: Option<String>,
}

/// Forget the saved rows of removals whose files are gone; markers stay.
/// Only database rows change, so it answers directly.
pub async fn purge(
    ctx: DbContext,
    request: Option<Json<PurgeRequest>>,
) -> Result<Json<ApiResponse<PurgeReport>>, AppError> {
    let request = request.map(|Json(request)| request).unwrap_or_default();
    let path = ctx.database_path.clone();
    let report = tokio::task::spawn_blocking(move || {
        let conn = open_scheduler_connection(&path).map_err(AppError::db)?;
        reject_removal::purge(&conn, request.batch_id.as_deref(), now())
            .map_err(|error| AppError::InternalError(format!("{error:#}")))
    })
    .await
    .map_err(|error| AppError::InternalError(format!("reject removal: {error}")))??;
    Ok(Json(ApiResponse::success(report)))
}
