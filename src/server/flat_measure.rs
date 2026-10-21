//! The flat measuring job: flat responses, comparisons and light evidence
//! (see [`crate::calibration::flat_groups::job`]) as a tracked background
//! job of its own, one per catalog.
//!
//! - **Scoped.** Each trigger queues only what it touched: the rigs whose
//!   calibration frames an import, sync, upload, forget, merge or override
//!   changed ([`Scope::flats`]), or, for new lights alone, light evidence
//!   ([`Scope::lights`]). Within a rig the pass reads only sessions without
//!   a current response, pairs not yet compared and imaging sessions whose
//!   evidence key is new, so a catalog that is up to date reads no pixels.
//! - **Debounced.** Requests merge, and the pass starts once they pause for
//!   [`FLAT_MEASURE_SETTLE`], and once the catalog's grading is done (its
//!   grades decide which flats are in use).
//! - **Retried.** A rig a pass could not finish (flats away, lights the
//!   folders do not show yet, an error such as a busy catalog) is tried
//!   again after [`RETRY_FIRST`], doubling up to [`RETRY_MAX`], at most
//!   [`RETRY_LIMIT`] times; a new change to the rig starts again.
//! - **Pause** holds every rig's work until Resume or the next start. A
//!   pass paused midway puts its rigs back in line, and changes that come
//!   meanwhile wait with them (see [`crate::server::job_line::catalog`]).
//! - **Prioritised.** A pass runs at [`Priority::Background`]. While a page
//!   shows a rig's flats ([`watch`]) that rig goes first and runs at
//!   [`Priority::Viewed`]: workers borrowed from the background share,
//!   leased again for each step, and no interactive guard. Either way it
//!   waits for interactive work a person started before each step
//!   ([`AppState::wait_for_interactive_work`]), and says what it waits for.
//!   No flat path holds the interactive guard: nobody starts a pass. It
//!   does not wait for `RESUME_QUIET` after a person's work, as automatic
//!   stack refreshes do: a page is showing what it measures.
//! - **Time mode.** A rig whose flats group by time is never measured.
//! - **Tracked.** [`status`] feeds the header's job line and the page.

use crate::calibration::flat_groups::{
    self,
    job::{PassOutcome, Progress, Rigs, Scope, Workers},
};
use crate::calibration::rules::RuleBook;
use crate::concurrency::{ComputePool, Priority, Stop};
use crate::server::api::ApiResponse;
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;
use crate::server::job_line::catalog::{CatalogJob, CatalogWork, Step};
use crate::server::job_text::{joined, plural};
use crate::server::state::AppState;
use axum::extract::State;
use axum::Json;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long requests must pause before a pass starts, so a burst of
/// imports or uploads shares one pass.
pub const FLAT_MEASURE_SETTLE: Duration = Duration::from_secs(5);

/// How long a page's interest in a rig lasts after it last said so. The
/// page says so more often than this while it shows the rig.
pub const WATCH_TTL: Duration = Duration::from_secs(45);

/// How often a page showing a rig asks for a check of it while the job
/// rests: a pass that finds nothing stale reads nothing.
pub const WATCH_RECHECK: Duration = Duration::from_secs(5 * 60);

/// The wait before a rig a pass could not finish is tried again; it
/// doubles with each try, up to [`RETRY_MAX`].
pub const RETRY_FIRST: Duration = Duration::from_secs(60);

/// The longest wait between tries.
pub const RETRY_MAX: Duration = Duration::from_secs(30 * 60);

/// Tries before a rig waits for the next change to it, or the next start.
pub const RETRY_LIMIT: u32 = 6;

/// How often a waiting pass looks again.
const POLL: Duration = Duration::from_millis(250);

/// The longest nap of the runner while it waits for a retry.
const NAP_MAX: Duration = Duration::from_secs(1);

/// How long a pass waits for the catalog's write lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// The retry key for work on every rig.
const EVERY_RIG: &str = "*";

/// Where a catalog's flat measuring stands, for the job line and the page.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct FlatMeasureStatus {
    /// `idle`; `queued` while requests settle, grading runs or a retry
    /// waits; `running`; or `paused` while a person paused the job.
    pub state: String,
    /// The pass's step in words, such as `Measuring flats · 12 of 40
    /// sessions`, as pages and the header show it.
    pub label: String,
    /// Whether a pass found work and is doing it: what the job line shows.
    /// A pass with nothing to read never turns active.
    pub active: bool,
    /// The rig being measured.
    pub rig_uuid: Option<String>,
    pub rig_name: Option<String>,
    /// [`flat_groups::job::STAGE_MEASURING`] and the stages after it.
    pub stage: Option<String>,
    pub done: usize,
    pub total: usize,
    /// Rigs whose work waits or runs; `*` for every rig.
    pub rigs: Vec<String>,
    /// Rigs the running pass checks after this one; some may turn out to
    /// have nothing to measure.
    pub rigs_after: usize,
    /// While a pass runs, the rigs asked for since it began, or whose retry
    /// is due by now, which the next pass takes; `*` for every rig.
    pub next_pass: Vec<String>,
    /// When the next retry is due, Unix seconds.
    pub retry_at: Option<i64>,
    /// `background` or `viewed`, while running.
    pub priority: Option<String>,
    /// What a person started that the pass waits for, while it does: the
    /// job line lists it as waiting.
    pub waiting_for: Option<String>,
    pub workers: usize,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    /// What the last finished pass did.
    pub last: Option<PassOutcome>,
    pub last_error: Option<String>,
}

/// Requests waiting for a pass.
#[derive(Debug, Default)]
pub(crate) struct Queue {
    pending: Scope,
    /// When the last request came.
    last: Option<Instant>,
    /// The scope of the pass running now.
    running: Option<Scope>,
    /// Rigs ([`EVERY_RIG`] for all) to try again, and when.
    due: BTreeMap<String, Instant>,
    /// Tries so far, per rig.
    attempts: BTreeMap<String, u32>,
    /// Rigs a page shows, and when it last said so.
    watches: HashMap<String, Instant>,
    /// When a page's look last queued a check of a rig.
    looks: HashMap<String, Instant>,
    /// The catalog's rigs, and when they were read: a page may only name
    /// those.
    known_rigs: Option<RigList>,
}

/// What the waiting requests allow now.
#[derive(Debug, PartialEq)]
enum Ready {
    /// Nothing waits.
    Idle,
    /// Requests are still arriving, or a retry is not due: look again
    /// after this.
    Settle(Duration),
    /// Run this.
    Run(Scope),
}

/// Where a trigger comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// A change to the catalog or a start: the tries of the rigs whose
    /// flats it names start again.
    Change,
    /// A page showing a rig asks for a check: it respects the retry wait.
    Look,
}

fn scope_of(key: &str) -> Scope {
    if key == EVERY_RIG {
        Scope::everything()
    } else {
        Scope::flats(Rigs::of([key]))
    }
}

/// The wait before try `attempt` (1 for the first retry).
fn backoff(attempt: u32) -> Duration {
    let doubled = RETRY_FIRST.saturating_mul(1 << attempt.saturating_sub(1).min(16));
    doubled.min(RETRY_MAX)
}

impl Queue {
    fn add(&mut self, scope: Scope, now: Instant, trigger: Trigger) {
        if trigger == Trigger::Change {
            // A change to rigs' flats starts their tries again. New lights
            // alone do not.
            match &scope.flats {
                Rigs::All => {
                    self.attempts.clear();
                    self.due.clear();
                }
                Rigs::Some(rigs) => {
                    for rig in rigs {
                        self.attempts.remove(rig);
                        self.due.remove(rig);
                    }
                }
                Rigs::None => {}
            }
        }
        self.pending.merge(scope);
        self.last = Some(now);
    }

    /// Try `key` (a rig, or [`EVERY_RIG`]) again later. False once it has
    /// had its [`RETRY_LIMIT`] tries.
    fn retry(&mut self, key: &str, now: Instant) -> bool {
        let attempt = self.attempts.get(key).copied().unwrap_or(0) + 1;
        self.attempts.insert(key.to_string(), attempt);
        if attempt > RETRY_LIMIT {
            self.due.remove(key);
            return false;
        }
        self.due.insert(key.to_string(), now + backoff(attempt));
        true
    }

    /// A pass finished `key` (a rig, or every rig): its tries are over.
    fn finished(&mut self, key: &str) {
        if key == EVERY_RIG {
            self.attempts.clear();
        } else {
            self.attempts.remove(key);
        }
    }

    /// The merged requests once they have paused for
    /// [`FLAT_MEASURE_SETTLE`], with every retry that is due.
    fn take_ready(&mut self, now: Instant) -> Ready {
        let due: Vec<String> = self
            .due
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(key, _)| key.clone())
            .collect();
        for key in due {
            self.due.remove(&key);
            self.pending.merge(scope_of(&key));
        }
        if self.pending.is_empty() {
            return match self.due.values().min() {
                Some(next) => Ready::Settle(next.saturating_duration_since(now)),
                None => Ready::Idle,
            };
        }
        let quiet = self.last.map_or(FLAT_MEASURE_SETTLE, |last| {
            now.saturating_duration_since(last)
        });
        if quiet < FLAT_MEASURE_SETTLE {
            return Ready::Settle(FLAT_MEASURE_SETTLE - quiet);
        }
        self.last = None;
        Ready::Run(std::mem::take(&mut self.pending))
    }
}

/// Flat measuring, run on one thread per catalog.
pub(crate) struct FlatMeasure;

type Job = CatalogJob<FlatMeasure>;

impl CatalogWork for FlatMeasure {
    type Queue = Queue;
    type Status = FlatMeasureStatus;
    type Item = Scope;
    const RUNNER: &'static str = "flat-measure";
    const SHUTDOWN_WAITS_FOR: Option<&'static str> = Some("flat measuring");

    /// A pass waits while grading runs, because grades decide which flats
    /// are in use.
    fn next(state: &AppState, path: &str, queue: &mut Queue, now: Instant) -> Step<Scope> {
        let grading = crate::server::calibration_quality::busy(state, path);
        let step = next_step(queue, now, grading);
        if let Step::Run(scope) = &step {
            queue.running = Some(scope.clone());
        }
        step
    }

    fn run(state: &Arc<AppState>, path: &str, job: &Job, scope: Scope, stop: &Stop) {
        run_logged(state, path, job, scope, stop);
    }

    fn runner_failed(queue: &mut Queue, status: &mut FlatMeasureStatus) {
        queue.running = None;
        status.active = false;
        status.stage = None;
        status.priority = None;
        status.waiting_for = None;
        status.last_error = Some("flat measuring stopped unexpectedly".into());
    }
}

/// One job per catalog file.
fn job(state: &AppState, path: &str) -> Arc<Job> {
    state.catalog_jobs.flat_measure.job(path)
}

fn rig_list(scope: &Scope) -> Vec<String> {
    let mut rigs: Vec<String> = Vec::new();
    for set in [&scope.flats, &scope.lights] {
        match set {
            Rigs::None => {}
            Rigs::All => return vec![EVERY_RIG.into()],
            Rigs::Some(some) => rigs.extend(some.iter().cloned()),
        }
    }
    rigs.sort();
    rigs.dedup();
    rigs
}

fn unix_at(at: Instant) -> i64 {
    let ahead = at.saturating_duration_since(Instant::now());
    chrono::Utc::now().timestamp() + ahead.as_secs() as i64
}

/// Where flat measuring stands for the catalog at `path`.
pub fn status(state: &AppState, path: &str) -> FlatMeasureStatus {
    let job = job(state, path);
    let mut status = job.status();
    let queue = job.queue();
    // What the next pass takes: requests since this one began, and retries
    // due by now. A retry not yet due waits for its time, not this pass.
    let now = Instant::now();
    let mut next = queue.pending.clone();
    for (key, _) in queue.due.iter().filter(|(_, at)| **at <= now) {
        next.merge(scope_of(key));
    }
    status.next_pass = if queue.running.is_some() {
        rig_list(&next)
    } else {
        Vec::new()
    };
    let mut waiting = queue.pending.clone();
    for key in queue.due.keys() {
        waiting.merge(scope_of(key));
    }
    if let Some(running) = &queue.running {
        waiting.merge(running.clone());
    }
    status.rigs = rig_list(&waiting);
    status.retry_at = queue.due.values().min().map(|at| unix_at(*at));
    status.state = if queue.running.is_some() {
        "running"
    } else if queue.paused() {
        "paused"
    } else if !status.rigs.is_empty() {
        "queued"
    } else {
        "idle"
    }
    .into();
    if queue.running.is_none() {
        status.active = false;
    }
    status.label = label(status.stage.as_deref(), status.done, status.total);
    status
}

/// "Measuring flats · 12 of 40 sessions": the flat measuring pass's step.
pub fn label(stage: Option<&str>, done: usize, total: usize) -> String {
    use crate::calibration::flat_groups::job::{STAGE_COMPARING, STAGE_EVIDENCE};
    let (verb, unit) = match stage {
        Some(STAGE_COMPARING) => ("Comparing flats", "pair"),
        Some(STAGE_EVIDENCE) => ("Reading lights shot during a change", "session"),
        _ => ("Measuring flats", "session"),
    };
    if total == 0 {
        return verb.to_string();
    }
    format!(
        "{verb} · {} of {total} {}",
        done.min(total),
        plural(total, unit, &format!("{unit}s"))
    )
}

/// `1 more rig to check · 2 rigs queued`: the rigs the pass checks after
/// the current one (some may have nothing to measure), and the rigs the
/// next pass takes.
pub fn rigs_left(rigs_after: usize, next_pass: &[String]) -> Option<String> {
    let rigs = |count: usize| plural(count, "rig", "rigs");
    let after =
        (rigs_after > 0).then(|| format!("{rigs_after} more {} to check", rigs(rigs_after)));
    let next = if next_pass.iter().any(|rig| rig == "*") {
        Some("every rig queued".to_string())
    } else if !next_pass.is_empty() {
        Some(format!(
            "{} {} queued",
            next_pass.len(),
            rigs(next_pass.len())
        ))
    } else {
        None
    };
    let text = joined([
        after.as_deref().unwrap_or(""),
        next.as_deref().unwrap_or(""),
    ]);
    (!text.is_empty()).then_some(text)
}

/// Whether a pass runs over the catalog at `path`.
pub fn running(state: &AppState, path: &str) -> bool {
    job(state, path).queue().running.is_some()
}

/// Queue flat measuring for the catalog at `path`: `scope` names the rigs
/// whose flats or lights changed. Requests merge; the pass starts once they
/// pause for [`FLAT_MEASURE_SETTLE`] and grading is done. While the job is
/// paused, the requests wait for Resume.
pub fn queue(state: &Arc<AppState>, path: &str, scope: Scope) {
    enqueue(state, path, scope, Trigger::Change);
}

fn enqueue(state: &Arc<AppState>, path: &str, scope: Scope, trigger: Trigger) {
    if scope.is_empty() || state.shutdown.is_stopping() {
        return;
    }
    // The runner thread sleeps until requests stop arriving. Shutdown waits
    // only while a pass runs.
    state
        .catalog_jobs
        .flat_measure
        .request(state, path, |queue| {
            queue.add(scope, Instant::now(), trigger)
        });
}

/// [`queue`] for every catalog the server holds.
pub fn queue_all(state: &Arc<AppState>, scope: Scope) {
    for ctx in state.all_databases() {
        queue(state, &ctx.database_path, scope.clone());
    }
}

/// The runner's choice, given the queue and whether grading is busy: a pass
/// waits for grading, which decides the flats in use, without pushing back
/// its settle.
fn next_step(queue: &mut Queue, now: Instant, grading: bool) -> Step<Scope> {
    match queue.take_ready(now) {
        Ready::Idle => Step::Exit,
        Ready::Settle(wait) => Step::Sleep(wait.clamp(POLL, NAP_MAX)),
        Ready::Run(scope) if grading => {
            queue.pending.merge(scope);
            Step::Sleep(POLL)
        }
        Ready::Run(scope) => Step::Run(scope),
    }
}

fn run_logged(state: &Arc<AppState>, path: &str, job: &Job, scope: Scope, stop: &Stop) {
    job.update(|status| {
        *status = FlatMeasureStatus {
            started_at: Some(chrono::Utc::now().timestamp()),
            last: status.last.take(),
            ..Default::default()
        };
    });
    let result = run(state, path, job, &scope, stop);
    // Only a pause stops a pass.
    let changed = settle_outcome(job, &scope, &result, Instant::now(), stop.requested());
    match &result {
        Ok(outcome) => {
            if outcome.frames_read > 0 || outcome.compared > 0 {
                tracing::info!(
                    "Measured flats in {path}: {} session(s), {} comparison(s), {} light \
                     session(s), {} frame(s) read",
                    outcome.measured,
                    outcome.compared,
                    outcome.evidence,
                    outcome.frames_read
                );
            }
        }
        Err(error) if stop.requested() => tracing::info!(
            "Measuring flats in {path} ended at a pause: {error:#}. It runs again at Resume."
        ),
        Err(error) => tracing::info!(
            "Could not finish measuring flats in {path}: {error:#}. It is tried again shortly."
        ),
    }
    job.update(|status| {
        status.active = false;
        status.rig_uuid = None;
        status.rig_name = None;
        status.rigs_after = 0;
        status.stage = None;
        status.priority = None;
        status.waiting_for = None;
        status.finished_at = Some(chrono::Utc::now().timestamp());
        match result {
            Ok(outcome) => {
                status.last = Some(outcome);
                status.last_error = None;
            }
            Err(error) => status.last_error = Some(format!("{error:#}")),
        }
    });
    if changed {
        // What each project's lights take may have changed.
        crate::server::calibration_health::request(state, path);
    }
}

/// Put what a pass left behind back in line: a paused pass's whole scope
/// for Resume, and rigs it could not finish (or, on an error, its whole
/// scope) as retries. Returns whether the pass changed anything health
/// reads.
fn settle_outcome(
    job: &Job,
    scope: &Scope,
    result: &anyhow::Result<PassOutcome>,
    now: Instant,
    paused: bool,
) -> bool {
    let mut queue = job.queue();
    queue.running = None;
    let keys = || -> Vec<String> {
        let rigs = rig_list(scope);
        if rigs.is_empty() {
            vec![EVERY_RIG.into()]
        } else {
            rigs
        }
    };
    match result {
        Ok(outcome) if outcome.stopped && paused => {
            // What it measured is stored: at Resume the pass reads only what
            // is left.
            queue.pending.merge(scope.clone());
            outcome.measured + outcome.compared + outcome.evidence > 0
        }
        Ok(outcome) => {
            for key in keys() {
                if key != EVERY_RIG && !outcome.retry.contains(&key) {
                    queue.finished(&key);
                }
            }
            if rig_list(scope) == [EVERY_RIG] && outcome.retry.is_empty() {
                queue.finished(EVERY_RIG);
            }
            for rig in &outcome.retry {
                queue.retry(rig, now);
            }
            outcome.measured + outcome.compared + outcome.evidence > 0
        }
        Err(_) if paused => {
            queue.pending.merge(scope.clone());
            true
        }
        Err(_) => {
            for key in keys() {
                queue.retry(&key, now);
            }
            true
        }
    }
}

/// One pass over the rigs `scope` names, on the server's catalog context.
fn run(
    state: &AppState,
    path: &str,
    job: &Job,
    scope: &Scope,
    stop: &Stop,
) -> anyhow::Result<PassOutcome> {
    let Some(ctx) = state
        .all_databases()
        .into_iter()
        .find(|ctx| ctx.database_path == path)
    else {
        return Ok(Default::default());
    };
    let dir = ctx
        .cache()
        .ensure_category_dir(flat_groups::CACHE_CATEGORY)?;
    flat_groups::register_response_dir(Path::new(path), dir.clone());
    // The image folders are walked only when a light is looked for.
    let tree = OnceLock::new();
    let find_light = |name: &str| {
        tree.get_or_init(|| ctx.get_directory_tree().ok())
            .as_ref()
            .and_then(|tree| tree.find_file_first(name).cloned())
    };
    run_with(state, path, job, scope, stop, &dir, &find_light)
}

/// [`run`] with the response folder and the light lookup given: the
/// catalog is planned once, and the rigs are measured in turn, those a
/// page shows first.
fn run_with(
    state: &AppState,
    path: &str,
    job: &Job,
    scope: &Scope,
    stop: &Stop,
    dir: &Path,
    find_light: &dyn Fn(&str) -> Option<PathBuf>,
) -> anyhow::Result<PassOutcome> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    if conn.is_readonly(rusqlite::MAIN_DB)? || !flat_groups::store::tables_exist(&conn) {
        return Ok(Default::default());
    }
    let names: HashMap<String, String> = flat_rigs(&conn)?.into_iter().collect();
    let workers = PriorityWorkers::new(state, "", largest_flat_pixels(&conn), job);
    let keep_going = || !state.shutdown.is_stopping() && !stop.requested();
    let yield_now = || workers.yield_now();
    let goes_first = |rig: &str| watched_in(job, rig);
    let mut progress = |at: &Progress<'_>| {
        workers.set_rig(at.rig);
        job.update(|status| {
            if status.rig_uuid.as_deref() != Some(at.rig) {
                status.rig_uuid = Some(at.rig.to_string());
                status.rig_name = names.get(at.rig).cloned();
            }
            status.stage = Some(at.stage.to_string());
            status.done = at.done;
            status.total = at.total;
            status.rigs_after = at.rigs_after;
            // Work found: the job line shows it from here on.
            status.active |= at.total > 0 && at.done < at.total;
        });
    };
    flat_groups::job::run(flat_groups::job::Pass {
        conn: &conn,
        dir,
        scope: scope.clone(),
        keep_going: &keep_going,
        yield_now: &yield_now,
        progress: &mut progress,
        goes_first: &goes_first,
        find_light,
        pool: Some(&workers),
    })
}

/// Rigs with raw flats, and their names.
fn flat_rigs(conn: &Connection) -> anyhow::Result<Vec<(String, String)>> {
    let mut statement = conn.prepare(
        "SELECT DISTINCT f.rig_uuid, COALESCE(r.name, f.rig_uuid)
         FROM psf_guard_calibration_frame f
         LEFT JOIN psf_guard_rig r ON r.rig_uuid = f.rig_uuid
         WHERE f.kind = 'flat' AND f.is_master = 0
         ORDER BY f.rig_uuid",
    )?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The samples in the library's largest raw flat, every channel counted:
/// what one worker holds at once, a flat or a light of the same sensor.
fn largest_flat_pixels(conn: &Connection) -> Option<usize> {
    conn.query_row(
        "SELECT MAX(width * height * COALESCE(channels, 1)) FROM psf_guard_calibration_frame
         WHERE kind = 'flat' AND is_master = 0",
        [],
        |row| row.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
    .and_then(|pixels| usize::try_from(pixels).ok())
    .filter(|pixels| *pixels > 0)
}

// ---------------------------------------------------------------------------
// Interest: a page showing a rig lifts its pass to the viewed tier.

/// How long a catalog's list of rigs is trusted before it is read again.
const RIG_LIST_TTL: Duration = Duration::from_secs(60);

/// A catalog's rigs, and when they were read.
type RigList = (Instant, BTreeSet<String>);

/// Whether the catalog at `path` records this rig. Read on a connection of
/// its own, at most every [`RIG_LIST_TTL`], with the queue unlocked.
fn catalog_has_rig(job: &Job, path: &str, rig: &str) -> bool {
    let now = Instant::now();
    if let Some((read, rigs)) = &job.queue().known_rigs
        && now.saturating_duration_since(*read) < RIG_LIST_TTL
    {
        return rigs.contains(rig);
    }
    let rigs: BTreeSet<String> = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .and_then(|conn| {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        let mut statement = conn.prepare("SELECT rig_uuid FROM psf_guard_rig")?;
        let rigs: BTreeSet<String> = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rigs)
    })
    .unwrap_or_default();
    let known = rigs.contains(rig);
    job.queue().known_rigs = Some((now, rigs));
    known
}

/// A page shows the flats of `rig` in the catalog at `path`. For
/// [`WATCH_TTL`] that rig goes first and runs in the viewed tier; and, at
/// most every [`WATCH_RECHECK`], a check of it is queued, unless the job is
/// paused, a retry waits or a pass runs. A rig the catalog does not
/// record is ignored. Reads the catalog's rigs: call it off the request
/// path.
pub fn watch(state: &Arc<AppState>, path: &str, rig: &str) {
    let job = job(state, path);
    if !catalog_has_rig(&job, path, rig) {
        return;
    }
    let now = Instant::now();
    {
        let mut queue = job.queue();
        watch_at(&mut queue, rig, now);
        queue
            .looks
            .retain(|_, at| now.saturating_duration_since(*at) < WATCH_RECHECK);
        if queue.looks.contains_key(rig) {
            return;
        }
        let paused = queue.paused();
        let waiting = queue.due.contains_key(rig) || queue.due.contains_key(EVERY_RIG);
        if paused || waiting || queue.running.is_some() {
            return;
        }
        // Only a check queued counts against the next.
        queue.looks.insert(rig.to_string(), now);
    }
    enqueue(state, path, Scope::flats(Rigs::of([rig])), Trigger::Look);
}

fn watch_at(queue: &mut Queue, rig: &str, at: Instant) {
    queue
        .watches
        .retain(|_, seen| at.saturating_duration_since(*seen) < WATCH_TTL);
    queue.watches.insert(rig.to_string(), at);
}

/// Whether a page shows this rig's flats now.
pub fn watched(state: &AppState, path: &str, rig: &str) -> bool {
    watched_in(&job(state, path), rig)
}

/// Whether a page shows this rig's flats now, in `job`'s catalog.
fn watched_in(job: &Job, rig: &str) -> bool {
    let now = Instant::now();
    job.queue()
        .watches
        .get(rig)
        .is_some_and(|seen| now.saturating_duration_since(*seen) < WATCH_TTL)
}

/// The workers a pass runs on. Each step leases them again at the tier the
/// rig's watchers ask for (viewed while a page shows it, background
/// otherwise) and gives them back when it ends, so a person's work started
/// meanwhile plans with its whole share; and each read first waits for
/// interactive work a person started.
struct PriorityWorkers<'a> {
    state: &'a AppState,
    rig: Mutex<String>,
    frame_pixels: Option<usize>,
    job: &'a Job,
}

impl<'a> PriorityWorkers<'a> {
    fn new(state: &'a AppState, rig: &str, frame_pixels: Option<usize>, job: &'a Job) -> Self {
        Self {
            state,
            rig: Mutex::new(rig.to_string()),
            frame_pixels,
            job,
        }
    }

    fn set_rig(&self, rig: &str) {
        let mut current = self.rig.lock().unwrap_or_else(|p| p.into_inner());
        if *current != rig {
            *current = rig.to_string();
        }
    }

    /// The tier the rig's watchers ask for now.
    fn priority(&self) -> Priority {
        let rig = self.rig.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if watched_in(self.job, &rig) {
            Priority::Viewed
        } else {
            Priority::Background
        }
    }

    /// Before each read: wait for interactive work a person started,
    /// through the shared helper, which names what the pass waits for once
    /// the wait outlasts a passing preview. Both tiers do; neither holds
    /// anyone else up.
    fn yield_now(&self) {
        self.state.wait_for_interactive_work(POLL, |waiting_for| {
            self.job
                .update(|status| status.waiting_for = waiting_for.map(str::to_string));
        });
    }
}

impl Workers for PriorityWorkers<'_> {
    fn run(&self, work: &mut (dyn FnMut() + Send)) {
        let priority = self.priority();
        let budget = crate::concurrency::plan_workers(
            None,
            &self.state.worker_policy(),
            priority,
            self.frame_pixels,
        );
        // Leased for this step alone.
        let lease = self.state.lease_workers(priority, budget.workers);
        self.job.update(|status| {
            status.priority = Some(priority.label().into());
            status.workers = lease.workers;
        });
        match ComputePool::take("calibration-flat-groups", lease.workers) {
            Ok(pool) => pool.install(work),
            // No pool to be had: the calling thread does it.
            Err(_) => work(),
        }
    }
}

// ---------------------------------------------------------------------------
// Triggers: which rigs a change touched.

/// The rigs of these calibration frames, looked up by their uuids: what a
/// forget, an override or a merge names. Read before the frames go.
pub fn rigs_of_frames(conn: &Connection, frame_uuids: &[String]) -> Rigs {
    if frame_uuids.is_empty() {
        return Rigs::None;
    }
    let Ok(mut statement) =
        conn.prepare("SELECT rig_uuid FROM psf_guard_calibration_frame WHERE frame_uuid = ?1")
    else {
        return Rigs::None;
    };
    let mut rigs = BTreeSet::new();
    for uuid in frame_uuids {
        if let Ok(rig) = statement.query_row([uuid], |row| row.get::<_, String>(0)) {
            rigs.insert(rig);
        }
    }
    Rigs::of(rigs)
}

/// Per rig, a digest of its raw calibration frames as flat measuring sees
/// them: which frames, their files, kinds, times, and what keeps them out.
/// Read on a connection of its own ([`rig_state_at`]), never the request
/// connection: it scans the whole library.
pub type RigState = BTreeMap<String, String>;

/// The catalog's [`RigState`], or `None` when it cannot be read.
pub fn rig_state(conn: &Connection) -> Option<RigState> {
    if !crate::calibration::schema_exists(conn) {
        return Some(RigState::new());
    }
    let sql = |grade: &str, join: &str| {
        format!(
            "SELECT f.rig_uuid, f.frame_uuid, f.source_fingerprint, f.kind, f.captured_at,
                    f.stray_light, {grade}
             FROM psf_guard_calibration_frame f {join}
             WHERE f.is_master = 0
             ORDER BY f.rig_uuid, f.frame_uuid"
        )
    };
    // Without the grade table, a catalog that has not been graded yet.
    let mut statement = conn
        .prepare(&sql(
            "q.grade, q.override_choice",
            "LEFT JOIN psf_guard_calibration_frame_quality q ON q.frame_uuid = f.frame_uuid",
        ))
        .or_else(|_| conn.prepare(&sql("NULL, NULL", "")))
        .ok()?;
    let mut hashers: BTreeMap<String, Sha256> = BTreeMap::new();
    let rows = statement
        .query_map([], |row| {
            let rig: String = row.get(0)?;
            let mut text = String::new();
            for index in 1..8 {
                let value: rusqlite::types::Value = row.get(index)?;
                text.push_str(&format!("{value:?}\u{1f}"));
            }
            Ok((rig, text))
        })
        .ok()?;
    for row in rows {
        let (rig, text) = row.ok()?;
        hashers.entry(rig).or_default().update(text.as_bytes());
    }
    Some(
        hashers
            .into_iter()
            .map(|(rig, hasher)| {
                let digest: String = hasher
                    .finalize()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                (rig, digest)
            })
            .collect(),
    )
}

/// The [`RigState`] of the catalog at `path`, read on its own connection.
pub fn rig_state_at(path: &str) -> Option<RigState> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    conn.busy_timeout(BUSY_TIMEOUT).ok()?;
    rig_state(&conn)
}

/// The rigs whose frames differ between two states: added, gone or
/// changed. Every rig when either could not be read.
pub fn changed_rigs(before: Option<&RigState>, after: Option<&RigState>) -> Rigs {
    let (Some(before), Some(after)) = (before, after) else {
        return Rigs::All;
    };
    Rigs::of(
        before
            .keys()
            .chain(after.keys())
            .filter(|rig| before.get(*rig) != after.get(*rig))
            .cloned(),
    )
}

/// Queue the flats of the rigs a change to the catalog at `path` touched,
/// from its state before the change (read with [`rig_state_at`]), and its
/// lights when `lights` say new ones came. Call it off the request path.
pub fn queue_change(state: &Arc<AppState>, path: &str, before: Option<&RigState>, lights: bool) {
    let mut scope = Scope::flats(changed_rigs(before, rig_state_at(path).as_ref()));
    if !scope.flats.is_empty() {
        // New frames record their sites as they come; any whose header
        // could not be read then is read by the frame-sites job.
        crate::server::frame_sites::queue(state, path);
    }
    if lights {
        scope.merge(Scope::lights(Rigs::All));
    }
    queue(state, path, scope);
}

// ---------------------------------------------------------------------------
// Settings.

/// What a change of settings asks of flat measuring: the rigs whose
/// grouping limits or night zone changed and whose flats are grouped by
/// measurement now, or every rig when the server-wide ones changed. Their
/// book regroups from the stored measurements on its own; a pass only reads what the new
/// grouping lacks, as the sessions of a rig switched from time to measured,
/// and finds nothing to read for a rig that already had it.
pub fn rigs_to_measure_after(before: &RuleBook, after: &RuleBook) -> Rigs {
    let global = |book: &RuleBook| flat_groups::limits_of(&book.global());
    // Where nights split decides the sessions: a new split is planned
    // again, renaming what was measured and measuring runs it joins.
    let zone = |book: &RuleBook| book.global().night_offset_seconds;
    if global(after).measured && (global(before) != global(after) || zone(before) != zone(after)) {
        return Rigs::All;
    }
    let rigs: BTreeSet<&str> = before.rig_uuids().chain(after.rig_uuids()).collect();
    Rigs::of(rigs.into_iter().filter(|rig| {
        let (was_rules, now_rules) = (before.for_rig(rig), after.for_rig(rig));
        let was = flat_groups::limits_of(&was_rules);
        let now = flat_groups::limits_of(&now_rules);
        now.measured
            && (was != now || was_rules.night_offset_seconds != now_rules.night_offset_seconds)
    }))
}

/// Settings changed: queue measuring, in every catalog, for the rigs whose
/// grouping changed. When grades changed too, every rig: the grading pass
/// that follows may change which flats are in use. Reads nothing here.
pub fn settings_changed(state: &Arc<AppState>, before: &RuleBook, after: &RuleBook) {
    let rigs = if before.grades_differ(after) {
        Rigs::All
    } else {
        rigs_to_measure_after(before, after)
    };
    queue_all(state, Scope::flats(rigs));
}

/// Frames of these rigs found their sites after startup, so their nights
/// may split elsewhere: grade the catalog again, since grading screens
/// flats by session, and queue the rigs' measuring, which waits for grading
/// and plans their sessions again. Reads nothing here.
pub fn sites_found(state: &Arc<AppState>, path: &str, rigs: Vec<String>) {
    let rigs = Rigs::of(rigs);
    if rigs.is_empty() {
        return;
    }
    crate::server::calibration_quality::request(state, path);
    queue(state, path, Scope::flats(rigs));
}

// ---------------------------------------------------------------------------
// Routes.

/// `GET /api/db/{db}/calibrations/flat-measure` — where flat measuring
/// stands. The header and the page share this one poll.
pub async fn get_status(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Json<ApiResponse<FlatMeasureStatus>> {
    Json(ApiResponse::success(status(&state, &ctx.database_path)))
}

#[derive(Debug, Deserialize)]
pub struct WatchRequest {
    pub rig: String,
}

/// `POST /api/db/{db}/calibrations/flat-measure/watch` — a page shows this
/// rig's flats. A hint, not a change: it lifts the rig's pass to the viewed
/// tier for [`WATCH_TTL`] and, rate-limited, queues a check that reads
/// nothing when nothing is stale. It takes the gate of the other display
/// calculations (`requires_write`), so a viewer who may compute may send it.
pub async fn post_watch(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<WatchRequest>,
) -> Result<Json<ApiResponse<FlatMeasureStatus>>, AppError> {
    let rig = request.rig.trim();
    if rig.is_empty() || rig.len() > 64 {
        return Err(AppError::BadRequest("name one rig".into()));
    }
    let (path, rig) = (ctx.database_path.clone(), rig.to_string());
    // It may read the catalog's rigs: off the request path.
    let watching = Arc::clone(&state);
    let path = tokio::task::spawn_blocking(move || {
        watch(&watching, &path, &rig);
        path
    })
    .await
    .map_err(|error| AppError::InternalError(format!("noting the rig in view: {error}")))?;
    Ok(Json(ApiResponse::success(status(&state, &path))))
}

#[cfg(test)]
mod tests;
