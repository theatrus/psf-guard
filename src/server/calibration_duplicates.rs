//! The calibration page's Duplicates filter and its repair: exposures the
//! library records more than once, and merging them.
//!
//! Both are tracked jobs (see `docs/design/background-jobs.md`), one runner
//! per catalog, at [`Priority::Interactive`]: a person starts each.
//!
//! - **Check** (`POST …/duplicates/check`) reads the rows on its own
//!   connection, then looks at the files of the rows that could be
//!   duplicates on the interactive share's threads: "Checking files 812 of
//!   2,140", then "Comparing copies". It leaves the plan, with its token, in
//!   the status. Stop ends it at once.
//! - **Repair** (`POST …/duplicates/repair {token}`) checks again, applies
//!   only if the plan still has the previewed token, and merges in one
//!   transaction: "Merging 37 of 203". Stop between exposures rolls the
//!   whole repair back. When the plan has changed it applies nothing, and
//!   says what changed beside the new plan. It holds the catalog's repair
//!   lock, so the command line cannot repair the catalog meanwhile.
//! - **Status** (`GET …/duplicates`) answers at once, from memory.
//!
//! Neither waits for the stack worker. A master a build records while a
//! repair merges is guarded where it is recorded: one whose rows went or
//! moved is not recorded (`record_master`), and one recorded before the
//! repair is removed with the others. Files are read only inside the
//! database's folders, as the browser reads them, and never moved or
//! deleted.

use axum::{extract::State, http::StatusCode, Json};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::Duration;

use crate::calibration::{
    self, CheckStage, Checking, DuplicateReport, FileAccess, RepairError, RepairLockError,
    RepairOutcome, TwinGroup,
};
use crate::concurrency::{plan_workers, Priority};
use crate::server::api::ApiResponse;
use crate::server::calibration_browser::ReadableRoots;
use crate::server::database_context::DatabaseContext;
use crate::server::extract::DbContext;
use crate::server::handlers::{require_database_management_allowed, AppError};
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

#[derive(Default)]
struct Job {
    status: RwLock<DuplicateJobStatus>,
    stop: AtomicBool,
    /// Whether the catalog's one runner is at work.
    busy: Mutex<bool>,
    /// Holds a run at its next step until released or stopped, so a test
    /// can stop it there.
    #[cfg(test)]
    hold: AtomicBool,
}

impl Job {
    fn status(&self) -> DuplicateJobStatus {
        self.status
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn update(&self, change: impl FnOnce(&mut DuplicateJobStatus)) {
        change(
            &mut self
                .status
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
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
        while self.hold.load(Ordering::SeqCst) && !self.stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

static JOBS: LazyLock<Mutex<HashMap<String, Arc<Job>>>> = LazyLock::new(Default::default);

fn job(path: &str) -> Arc<Job> {
    let mut jobs = JOBS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    Arc::clone(jobs.entry(path.to_string()).or_default())
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
    if *job
        .busy
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
    {
        job.stop.store(true, Ordering::SeqCst);
    }
    status(path)
}

#[derive(Debug, Clone)]
enum Action {
    Check,
    Repair { token: String },
}

impl Action {
    fn name(&self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Repair { .. } => "repair",
        }
    }

    /// What the job line says the background work waits for.
    fn waited_for(&self) -> &'static str {
        match self {
            Self::Check => "a duplicate check",
            Self::Repair { .. } => "a duplicate repair",
        }
    }
}

/// Frees the runner if a run ends by a panic, or never starts. A run that
/// ends as it should frees the runner itself, as it says so.
struct RunnerClaim(Option<Arc<Job>>);

impl RunnerClaim {
    /// The run ended as it should, and freed the runner itself.
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for RunnerClaim {
    fn drop(&mut self) {
        if let Some(job) = self.0.take() {
            *job.busy
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
            job.update(|status| {
                if status.state == "running" {
                    status.state = "failed".into();
                    status.stage = None;
                    status.error = Some("the run ended unexpectedly".into());
                }
            });
        }
    }
}

/// Start a run on the catalog's runner. A check asked for while a check
/// runs joins it; anything else asked for while one runs is refused.
fn start(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    action: Action,
) -> Result<DuplicateJobStatus, AppError> {
    let job = job(&ctx.database_path);
    {
        let mut busy = job
            .busy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *busy {
            let running = job.status();
            if matches!(action, Action::Check) && running.action.as_deref() == Some("check") {
                return Ok(running);
            }
            return Err(AppError::Conflict(format!(
                "A duplicate {} is running on this catalog. Wait for it, or stop it.",
                running.action.as_deref().unwrap_or("check")
            )));
        }
        *busy = true;
    }
    job.stop.store(false, Ordering::SeqCst);
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
    });
    let claim = RunnerClaim(Some(Arc::clone(&job)));
    let (state_for_run, ctx_for_run, job_for_run) =
        (Arc::clone(state), Arc::clone(ctx), Arc::clone(&job));
    let spawned = std::thread::Builder::new()
        .name("calibration-duplicates".into())
        .spawn(move || {
            let claim = claim;
            run(&state_for_run, &ctx_for_run, &job_for_run, &action);
            claim.disarm();
        });
    if let Err(error) = spawned {
        // The closure, and with it the claim, was dropped: the runner is
        // free and the status failed.
        tracing::warn!("could not start a duplicate check or repair: {error}");
    }
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

fn run(state: &Arc<AppState>, ctx: &DatabaseContext, job: &Job, action: &Action) {
    let began = std::time::Instant::now();
    let ended = {
        // A person started this: background work waits for it, and says
        // so. Let go before the status says it is done.
        let _person = state.begin_named_interactive_job(action.waited_for());
        match action {
            Action::Check => check(state, ctx, job),
            Action::Repair { token } => repair(state, ctx, job, token),
        }
    };
    let elapsed = began.elapsed();
    // The runner is free as the status says the run ended, not after: a
    // page that sees the end may start the next run at once.
    let mut busy = job
        .busy
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *busy = false;
    job.update(|status| {
        status.finished_at = Some(now());
        status.stage = None;
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
    drop(busy);
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

/// Plan from the catalog as it is now: the rows on a connection of the
/// job's own, then the files on the interactive share's threads.
fn plan(state: &AppState, ctx: &DatabaseContext, job: &Job) -> Result<DuplicateReport, Ended> {
    let snapshot = {
        let conn = Connection::open_with_flags(
            &ctx.database_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        calibration::duplicate_snapshot(&conn)?
    };
    let roots = ReadableRoots::of(ctx);
    let files = FileAccess::resolving(move |path| roots.holds(path).then(|| path.to_path_buf()));
    let budget = plan_workers(None, &state.worker_policy(), Priority::Interactive, None);
    let lease = state.lease_workers(Priority::Interactive, budget.workers);
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
        stop: &job.stop,
    };
    calibration::plan_duplicate_repair_checked(&snapshot, &files, &checking)
        .map_err(|_| Ended::Stopped)
}

fn check(state: &AppState, ctx: &DatabaseContext, job: &Job) -> Result<(), Ended> {
    let report = plan(state, ctx, job)?;
    job.update(|status| {
        status.report = Some(report);
        status.checked_at = Some(now());
    });
    Ok(())
}

fn repair(
    state: &Arc<AppState>,
    ctx: &DatabaseContext,
    job: &Job,
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
    let report = plan(state, ctx, job)?;
    if report.token != token {
        let change = plan_change(previewed.as_ref(), &report);
        job.update(|status| {
            status.changed = Some(change);
            status.report = Some(report);
            status.checked_at = Some(now());
        });
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
            &job.stop,
        )
    };
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(RepairError::Stopped) => return Err(Ended::Stopped),
        Err(RepairError::Stale) => {
            // The rows changed between this run's check and its write:
            // check again, and say what changed.
            let fresh = plan(state, ctx, job)?;
            let change = plan_change(Some(&report), &fresh);
            job.update(|status| {
                status.changed = Some(change);
                status.report = Some(fresh);
                status.checked_at = Some(now());
            });
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
    Json(ApiResponse::success(status(&ctx.database_path)))
}

/// POST /api/db/{db}/calibrations/duplicates/check — check the library
/// for duplicates: `202` and the status, which the page polls.
pub async fn check_duplicates(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Result<(StatusCode, Json<ApiResponse<DuplicateJobStatus>>), AppError> {
    let status = start(&state, &ctx.0, Action::Check)?;
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
