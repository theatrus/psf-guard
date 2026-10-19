//! The calibration page's Duplicates filter and its repair: exposures the
//! library records more than once, and merging them.
//!
//! Both are tracked jobs (see `docs/design/background-jobs.md`), one runner
//! per catalog. A check or repair a person asks for runs at
//! [`Priority::Interactive`]; the check a page starts on its own when it
//! opens on no plan, or a stale one, runs at [`Priority::Background`] and
//! waits for a person's work, until a person asks for it.
//!
//! - **Check** (`POST …/duplicates/check`) reads the rows on its own
//!   connection, then looks at the files of the rows that could be
//!   duplicates on its share's threads: "Checking files 812 of 2,140", then
//!   "Comparing copies". It leaves the plan, with its token and the
//!   catalog's revision, in the status. Stop ends it at once. It writes
//!   nothing, so a read-only account may run it.
//! - **Repair** (`POST …/duplicates/repair {token}`) checks again, applies
//!   only if the plan still has the previewed token, and merges in one
//!   transaction: "Merging 37 of 203". Stop between exposures rolls the
//!   whole repair back. When the plan has changed it applies nothing, and
//!   says what changed beside the new plan. It holds the catalog's repair
//!   lock, so the command line cannot repair the catalog meanwhile.
//! - **Status** (`GET …/duplicates`) answers at once, from memory, and
//!   says whether the plan is `stale`: whether the catalog's calibration
//!   rows have changed since it was made (a sync, an import, a repair from
//!   the command line), read from one aggregate query.
//!
//! Neither waits for the stack worker. A master a build records while a
//! repair merges is guarded where it is recorded: one whose rows went or
//! moved is not recorded (`record_master`), and one recorded before the
//! repair is removed with the others. Files are read only inside the
//! database's folders, as the browser reads them, and never moved or
//! deleted.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::calibration::{
    self, CheckStage, Checking, DuplicateReport, FileAccess, RepairError, RepairLockError,
    RepairOutcome, TwinGroup,
};
use crate::concurrency::{plan_workers, Priority, Stop};
use crate::server::api::ApiResponse;
use crate::server::calibration_browser::ReadableRoots;
use crate::server::database_context::DatabaseContext;
use crate::server::extract::DbContext;
use crate::server::handlers::{require_database_management_allowed, AppError};
use crate::server::job_line::catalog::{CatalogJob, CatalogJobs, CatalogWork, Step};
use crate::server::state::AppState;

/// The stages a status names, for the page's words.
pub const STAGE_FILES: &str = "checking_files";
pub const STAGE_COPIES: &str = "comparing_copies";
pub const STAGE_MERGING: &str = "merging";

/// How long a repair waits for the catalog's write lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(30);

/// Most exposures a change lists by name; the counts say the rest.
const CHANGE_NAMES: usize = 20;

/// Where a catalog's duplicate check or repair stands.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DuplicateJobStatus {
    /// `idle` before the first run, `running`, then `done`, `stopped` or
    /// `failed`.
    pub state: String,
    /// `check` or `repair`: what runs, or ran last.
    pub action: Option<String>,
    /// [`STAGE_FILES`], [`STAGE_COPIES`] or [`STAGE_MERGING`], while it runs.
    pub stage: Option<String>,
    pub done: usize,
    pub total: usize,
    /// Counts up with each run, so a page knows which result is its own.
    pub run: u64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    /// The latest plan, with the token a repair takes. A repair that
    /// applied leaves none, until the next check.
    pub report: Option<DuplicateReport>,
    /// When that plan was made, Unix seconds.
    pub checked_at: Option<i64>,
    /// What the last repair that applied did, and its run.
    pub repaired: Option<Repaired>,
    /// A repair that found the plan changed since its preview: what
    /// changed. `report` is the new plan.
    pub changed: Option<PlanChange>,
    pub error: Option<String>,
    /// `interactive` or `background`, while it runs.
    pub priority: Option<String>,
    /// What a person started that a background check waits for, while it
    /// does.
    pub waiting_for: Option<String>,
    /// Whether the catalog's calibration rows changed since the plan was
    /// made. Worked out by the status route.
    pub stale: bool,
    /// The catalog's revision when the plan was made.
    #[serde(skip)]
    revision: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Repaired {
    pub run: u64,
    pub finished_at: i64,
    pub outcome: RepairOutcome,
}

/// How the plan a repair found differs from the one previewed.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct PlanChange {
    pub groups_before: usize,
    pub groups_after: usize,
    pub rows_before: usize,
    pub rows_after: usize,
    /// Exposures the new plan repairs that the preview did not, by file
    /// name, at most [`CHANGE_NAMES`].
    pub added: Vec<String>,
    pub added_count: usize,
    /// Exposures the preview repaired that the new plan leaves.
    pub dropped: Vec<String>,
    pub dropped_count: usize,
    /// Exposures in both whose rows or files differ.
    pub altered: Vec<String>,
    pub altered_count: usize,
}

/// The duplicate check and repair, run on one thread per catalog.
pub(crate) struct Duplicates;

type Job = CatalogJob<Duplicates>;

/// The catalog's duplicate run. Only one run per catalog is active at a time.
#[derive(Default)]
pub(crate) struct Queue {
    /// The requested run, until the runner thread starts it.
    pending: Option<(Action, Arc<DatabaseContext>)>,
    /// Whether a run is pending or running. A second request fails with a
    /// conflict until the run ends.
    run_active: bool,
    /// Whether a person requested the run, which then runs at `Interactive`. A
    /// background check a person joins turns interactive at its next step.
    interactive: bool,
    /// Holds a run at its next step until released or stopped, so a test
    /// can stop it there.
    #[cfg(test)]
    hold: bool,
}

impl CatalogWork for Duplicates {
    type Queue = Queue;
    type Status = DuplicateJobStatus;
    type Item = (Action, Arc<DatabaseContext>);
    const RUNNER: &'static str = "calibration-duplicates";
    /// Shutdown does not wait for a check, which stops at once. A repair
    /// registers its own shutdown hold around its write.
    const SHUTDOWN_WAITS_FOR: Option<&'static str> = None;

    fn next(_: &AppState, _: &str, queue: &mut Queue, _: Instant) -> Step<Self::Item> {
        queue.pending.take().map_or(Step::Exit, Step::Run)
    }

    fn run(state: &Arc<AppState>, _: &str, job: &Job, (action, ctx): Self::Item, stop: &Stop) {
        run(state, &ctx, &CurrentRun { job, stop }, &action);
    }

    fn runner_failed(queue: &mut Queue, status: &mut DuplicateJobStatus) {
        queue.pending = None;
        queue.run_active = false;
        if status.state == "running" {
            status.state = "failed".into();
            status.stage = None;
            status.error = Some("the run ended unexpectedly".into());
        }
    }
}

static JOBS: CatalogJobs<Duplicates> = CatalogJobs::new();

fn job(path: &str) -> Arc<Job> {
    JOBS.job(path)
}

/// The running check or repair, and its stop flag.
struct CurrentRun<'a> {
    job: &'a Job,
    stop: &'a Stop,
}

impl CurrentRun<'_> {
    fn status(&self) -> DuplicateJobStatus {
        self.job.status()
    }

    fn update(&self, change: impl FnOnce(&mut DuplicateJobStatus)) {
        self.job.update(change);
    }

    fn interactive(&self) -> bool {
        self.job.queue().interactive
    }

    fn progress(&self, stage: &str, done: usize, total: usize) {
        self.update(|status| {
            if status.stage.as_deref() != Some(stage) {
                status.stage = Some(stage.to_string());
            }
            // Threads finish out of order; the count only goes up.
            if status.total != total || done > status.done {
                status.done = done;
            }
            status.total = total;
        });
        #[cfg(test)]
        while self.job.queue().hold && !self.stop.requested() {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Where the duplicate check or repair stands for the catalog at `path`.
pub fn status(path: &str) -> DuplicateJobStatus {
    let mut status = job(path).status();
    if status.state.is_empty() {
        status.state = "idle".into();
    }
    status
}

/// Stop: a check ends at its next file, a repair at its next exposure,
/// rolled back whole.
pub fn stop(path: &str) -> DuplicateJobStatus {
    let job = job(path);
    if !job.stop_running() {
        // The run is pending and the runner thread has not started it:
        // remove it, so it never starts.
        let mut queue = job.queue();
        if queue.pending.take().is_some() {
            queue.run_active = false;
            job.update(|status| {
                status.state = "stopped".into();
                status.stage = None;
                status.priority = None;
                status.finished_at = Some(now());
            });
        }
    }
    status(path)
}

#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// `automatic` when a page starts it on its own.
    Check {
        automatic: bool,
    },
    Repair {
        token: String,
    },
}

impl Action {
    fn name(&self) -> &'static str {
        match self {
            Self::Check { .. } => "check",
            Self::Repair { .. } => "repair",
        }
    }

    fn automatic(&self) -> bool {
        matches!(self, Self::Check { automatic: true })
    }
}

/// What the job line says background work waits for.
fn waited_for(action: &str) -> String {
    format!("a duplicate {action}")
}

/// A cheap stamp of the catalog's calibration rows: how many, the newest
/// id and the newest change. A sync, an import, a forget, a merge or a
/// repair from the command line moves it; grading and flat measuring,
/// which write other tables, do not.
fn catalog_revision(conn: &Connection) -> rusqlite::Result<String> {
    if !calibration::schema_exists(conn) {
        return Ok("none".into());
    }
    conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(id), 0), COALESCE(MAX(updated_at), 0)
         FROM psf_guard_calibration_frame",
        [],
        |row| {
            Ok(format!(
                "{}:{}:{}",
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?
            ))
        },
    )
}

fn open_read_only(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}

/// Start a run on the catalog's runner thread. A check requested while a
/// check runs joins that check. Any other request while a run is active
/// fails with a conflict.
fn start(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    action: Action,
) -> Result<DuplicateJobStatus, AppError> {
    let job = job(&ctx.database_path);
    JOBS.request(state, &ctx.database_path, |queue| {
        if queue.run_active {
            let running = job.status();
            if matches!(action, Action::Check { .. }) && running.action.as_deref() == Some("check")
            {
                // A person joining a background check lifts it.
                if !action.automatic() {
                    queue.interactive = true;
                }
                return Ok(());
            }
            return Err(AppError::Conflict(format!(
                "A duplicate {} is running on this catalog. Wait for it, or stop it.",
                running.action.as_deref().unwrap_or("check")
            )));
        }
        queue.interactive = !action.automatic();
        queue.run_active = true;
        job.update(|status| {
            status.run += 1;
            status.state = "running".into();
            status.action = Some(action.name().into());
            status.stage = Some(STAGE_FILES.into());
            status.done = 0;
            status.total = 0;
            status.started_at = Some(now());
            status.finished_at = None;
            status.changed = None;
            status.error = None;
            status.waiting_for = None;
            status.priority = Some(
                if action.automatic() {
                    Priority::Background
                } else {
                    Priority::Interactive
                }
                .label()
                .into(),
            );
        });
        queue.pending = Some((action, Arc::clone(ctx)));
        Ok(())
    })?;
    Ok(job.status())
}

/// How a run ended short of its result.
enum Ended {
    Stopped,
    Failed(String),
}

impl From<rusqlite::Error> for Ended {
    fn from(error: rusqlite::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

impl From<anyhow::Error> for Ended {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed(format!("{error:#}"))
    }
}

fn run(state: &Arc<AppState>, ctx: &DatabaseContext, job: &CurrentRun, action: &Action) {
    let began = std::time::Instant::now();
    // Let go of any guard before the status says the run is done.
    let ended = match action {
        Action::Check { .. } => check(state, ctx, job),
        Action::Repair { token } => {
            // A person started this: background work waits for it, and
            // says so.
            let _person = state.begin_named_interactive_job(waited_for("repair"));
            repair(state, ctx, job, token)
        }
    };
    let elapsed = began.elapsed();
    // The runner is free as the status says the run ended, not after: a
    // page that sees the end may start the next run at once.
    let mut queue = job.job.queue();
    queue.run_active = false;
    job.update(|status| {
        status.finished_at = Some(now());
        status.stage = None;
        status.priority = None;
        status.waiting_for = None;
        match &ended {
            Ok(()) => status.state = "done".into(),
            Err(Ended::Stopped) => status.state = "stopped".into(),
            Err(Ended::Failed(error)) => {
                status.state = "failed".into();
                status.error = Some(error.clone());
            }
        }
    });
    let status = job.status();
    drop(queue);
    tracing::info!(
        "calibration duplicates: {} of {} {} in {:.2}s{}",
        action.name(),
        ctx.database_path,
        status.state,
        elapsed.as_secs_f64(),
        status
            .error
            .as_deref()
            .map(|error| format!(": {error}"))
            .unwrap_or_default()
    );
}

/// How often a background check looks again while a person's work runs.
const WAIT_NAP: Duration = Duration::from_millis(250);

/// A background check waits while a person's work runs, saying what for,
/// until it may go on, a person asks for it (it turns interactive), Stop,
/// or the server stops.
fn wait_for_people(state: &AppState, job: &CurrentRun) -> Result<(), Ended> {
    while let Some(name) = state.interactive_job_name() {
        if job.interactive() {
            break;
        }
        if job.stop.requested() || state.shutdown.is_stopping() {
            return Err(Ended::Stopped);
        }
        job.update(|status| {
            if status.waiting_for.as_deref() != Some(name.as_str()) {
                status.waiting_for = Some(name.clone());
            }
        });
        std::thread::sleep(WAIT_NAP);
    }
    job.update(|status| status.waiting_for = None);
    Ok(())
}

/// Plan from the catalog as it is now: the rows on a connection of the
/// job's own, then the files on the threads of its priority's share.
/// Returns the plan and the catalog's revision it was made from.
fn plan(
    state: &AppState,
    ctx: &DatabaseContext,
    job: &CurrentRun,
) -> Result<(DuplicateReport, String), Ended> {
    if !job.interactive() {
        wait_for_people(state, job)?;
    }
    let priority = if job.interactive() {
        Priority::Interactive
    } else {
        Priority::Background
    };
    job.update(|status| status.priority = Some(priority.label().into()));
    // A check that a person requested holds the guard, so background work steps
    // aside for it. A repair holds its own as well.
    let _person = (priority == Priority::Interactive)
        .then(|| state.begin_named_interactive_job(waited_for("check")));
    let (snapshot, revision) = {
        let conn = open_read_only(&ctx.database_path)?;
        let revision = catalog_revision(&conn)?;
        (calibration::duplicate_snapshot(&conn)?, revision)
    };
    let roots = ReadableRoots::of(ctx);
    let files = FileAccess::resolving(move |path| roots.holds(path).then(|| path.to_path_buf()));
    let budget = plan_workers(None, &state.worker_policy(), priority, None);
    let lease = state.lease_workers(priority, budget.workers);
    let progress = |stage: CheckStage, done: usize, total: usize| {
        let stage = match stage {
            CheckStage::Files => STAGE_FILES,
            CheckStage::Copies => STAGE_COPIES,
        };
        job.progress(stage, done, total);
    };
    let checking = Checking {
        workers: lease.workers,
        progress: &progress,
        stop: job.stop.as_flag(),
    };
    let report = calibration::plan_duplicate_repair_checked(&snapshot, &files, &checking)
        .map_err(|_| Ended::Stopped)?;
    Ok((report, revision))
}

/// Keep `report` as the latest plan, made from `revision`.
fn keep_plan(job: &CurrentRun, report: DuplicateReport, revision: String) {
    job.update(|status| {
        status.report = Some(report);
        status.revision = Some(revision);
        status.checked_at = Some(now());
    });
}

fn check(state: &AppState, ctx: &DatabaseContext, job: &CurrentRun) -> Result<(), Ended> {
    let (report, revision) = plan(state, ctx, job)?;
    keep_plan(job, report, revision);
    Ok(())
}

fn repair(
    state: &Arc<AppState>,
    ctx: &DatabaseContext,
    job: &CurrentRun,
    token: &str,
) -> Result<(), Ended> {
    let _lock = calibration::repair_lock(std::path::Path::new(&ctx.database_path)).map_err(
        |error| match error {
            RepairLockError::Busy => Ended::Failed(
                "Another repair of this catalog is running, from the command line or another server. Try again when it has finished."
                    .into(),
            ),
            error => Ended::Failed(error.to_string()),
        },
    )?;
    let previewed = job.status().report;
    let (report, revision) = plan(state, ctx, job)?;
    if report.token != token {
        let change = plan_change(previewed.as_ref(), &report);
        job.update(|status| status.changed = Some(change));
        keep_plan(job, report, revision);
        return Ok(());
    }
    let mut conn = Connection::open_with_flags(
        &ctx.database_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    job.progress(STAGE_MERGING, 0, report.groups.len());
    let outcome = {
        // A server stop waits for the commit or the rollback.
        let _hold = state.shutdown.hold("a duplicate repair");
        calibration::apply_duplicate_repair_with(
            &mut conn,
            &report,
            &|done, total| job.progress(STAGE_MERGING, done, total),
            job.stop.as_flag(),
        )
    };
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(RepairError::Stopped) => return Err(Ended::Stopped),
        Err(RepairError::Stale) => {
            // The rows changed between this run's check and its write:
            // check again, and say what changed.
            let (fresh, revision) = plan(state, ctx, job)?;
            let change = plan_change(Some(&report), &fresh);
            job.update(|status| status.changed = Some(change));
            keep_plan(job, fresh, revision);
            return Ok(());
        }
        Err(RepairError::Failed(error)) => return Err(Ended::Failed(format!("{error:#}"))),
    };
    if outcome.rows_removed > 0 {
        after_repair(
            state,
            ctx,
            report
                .groups
                .iter()
                .map(|group| group.rig_uuid.clone())
                .collect(),
        );
    }
    let run = job.status().run;
    job.update(|status| {
        status.report = None;
        status.revision = None;
        status.checked_at = None;
        status.repaired = Some(Repaired {
            run,
            finished_at: now(),
            outcome: outcome.clone(),
        });
    });
    Ok(())
}

/// What the plan a repair found changes from the plan previewed, by the
/// row each exposure keeps.
fn plan_change(before: Option<&DuplicateReport>, after: &DuplicateReport) -> PlanChange {
    let by_keep = |report: &DuplicateReport| {
        report
            .groups
            .iter()
            .map(|group| (group.keep.clone(), group.clone()))
            .collect::<HashMap<String, TwinGroup>>()
    };
    let empty = HashMap::new();
    let old = before.map(by_keep).unwrap_or_default();
    let old = if before.is_some() { &old } else { &empty };
    let new = by_keep(after);
    let shape = |group: &TwinGroup| {
        (
            group.path.clone(),
            group
                .members
                .iter()
                .map(|member| (member.frame_uuid.clone(), member.exists, member.keep))
                .collect::<Vec<_>>(),
        )
    };
    let names = |groups: Vec<&TwinGroup>| {
        let count = groups.len();
        let mut names = groups
            .into_iter()
            .map(|group| group.file_name.clone())
            .collect::<Vec<_>>();
        names.sort();
        names.truncate(CHANGE_NAMES);
        (names, count)
    };
    let (added, added_count) = names(
        after
            .groups
            .iter()
            .filter(|group| !old.contains_key(&group.keep))
            .collect(),
    );
    let (dropped, dropped_count) = names(
        before
            .map(|report| {
                report
                    .groups
                    .iter()
                    .filter(|group| !new.contains_key(&group.keep))
                    .collect()
            })
            .unwrap_or_default(),
    );
    let (altered, altered_count) = names(
        after
            .groups
            .iter()
            .filter(|group| {
                old.get(&group.keep)
                    .is_some_and(|previous| shape(previous) != shape(group))
            })
            .collect(),
    );
    PlanChange {
        groups_before: before.map_or(0, |report| report.groups.len()),
        groups_after: after.groups.len(),
        rows_before: before.map_or(0, |report| report.rows_removed),
        rows_after: after.rows_removed,
        added,
        added_count,
        dropped,
        dropped_count,
        altered,
        altered_count,
    }
}

/// What follows a repair that merged rows: the sessions a merged row was
/// measured in are measured again from the rows kept, for the rigs the plan
/// named; and the merged rows were peers the others were graded by, and
/// what each project's lights get may have changed.
fn after_repair(state: &Arc<AppState>, ctx: &DatabaseContext, rigs: Vec<String>) {
    let rigs = crate::calibration::flat_groups::job::Rigs::of(rigs);
    crate::server::flat_measure::queue(
        state,
        &ctx.database_path,
        crate::calibration::flat_groups::job::Scope::flats(rigs),
    );
    crate::server::calibration_quality::request(state, &ctx.database_path);
    crate::server::calibration_health::request(state, &ctx.database_path);
}

/// GET /api/db/{db}/calibrations/duplicates — where the check or repair
/// stands, with the latest plan. Never does the work itself.
pub async fn get_duplicates(ctx: DbContext) -> Json<ApiResponse<DuplicateJobStatus>> {
    let mut status = status(&ctx.database_path);
    // Whether the plan still describes the catalog: one aggregate query on
    // a connection of its own, and none while a run is going.
    if status.state != "running"
        && let Some(made_from) = status.revision.clone()
    {
        let path = ctx.database_path.clone();
        let now = tokio::task::spawn_blocking(move || {
            open_read_only(&path).and_then(|conn| catalog_revision(&conn))
        })
        .await;
        status.stale = matches!(now, Ok(Ok(now)) if now != made_from);
    }
    Json(ApiResponse::success(status))
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct CheckQuery {
    /// A page started it on its own, because it had no plan or a stale
    /// one: it runs at `Background`.
    #[serde(default)]
    pub automatic: bool,
}

/// POST /api/db/{db}/calibrations/duplicates/check — check the library
/// for duplicates: `202` and the status, which the page polls. It writes
/// nothing, so any account that may read the catalog may run it.
pub async fn check_duplicates(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Query(query): Query<CheckQuery>,
) -> Result<(StatusCode, Json<ApiResponse<DuplicateJobStatus>>), AppError> {
    let status = start(
        &state,
        &ctx.0,
        Action::Check {
            automatic: query.automatic,
        },
    )?;
    Ok((StatusCode::ACCEPTED, Json(ApiResponse::success(status))))
}

#[derive(Debug, serde::Deserialize)]
pub struct RepairRequest {
    /// The token of the plan that was previewed.
    pub token: String,
}

/// POST /api/db/{db}/calibrations/duplicates/repair — apply the previewed
/// plan: `202` and the status. The run applies nothing when the plan has
/// changed, and says what changed.
pub async fn repair_duplicates(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<RepairRequest>,
) -> Result<(StatusCode, Json<ApiResponse<DuplicateJobStatus>>), AppError> {
    require_database_management_allowed(&state)?;
    let status = start(
        &state,
        &ctx.0,
        Action::Repair {
            token: request.token,
        },
    )?;
    Ok((StatusCode::ACCEPTED, Json(ApiResponse::success(status))))
}

/// POST /api/db/{db}/calibrations/duplicates/stop — stop the check or
/// repair running.
pub async fn stop_duplicates(ctx: DbContext) -> Json<ApiResponse<DuplicateJobStatus>> {
    Json(ApiResponse::success(stop(&ctx.database_path)))
}

#[cfg(test)]
mod tests;
