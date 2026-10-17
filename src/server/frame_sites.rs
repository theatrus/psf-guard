//! The frame-sites job: reads the site of each calibration frame imported
//! before the catalog kept one ([`crate::calibration::backfill_frame_sites`]),
//! so its nights split at local noon where it was shot. A tracked
//! background job, one per catalog, on the patterns of
//! [`crate::server::flat_measure`]:
//!
//! - **Queued** at start and after a change that may bring frames; requests
//!   merge and the pass starts once they pause for [`SETTLE`].
//! - **Retried** while the server runs: a pass that found the volume away
//!   (no file it tried could be read, as with an unmounted NAS) or ended on
//!   an error is tried again after [`RETRY_FIRST`], doubling up to
//!   [`RETRY_MAX`], at most [`RETRY_LIMIT`] times. A frame whose file alone
//!   is away is marked and read again a week later ([`spawn_away_watch`]
//!   queues the pass), or on import.
//! - **Handed on as it goes.** The rigs it placed are graded again and
//!   their flat measuring queued ([`super::flat_measure::sites_found`]) at
//!   most every [`crate::calibration::night::SITE_FLUSH_EVERY`] and once
//!   when the pass ends, so flat measuring does not wait for the whole pass
//!   and one that ends early has handed on what it placed.
//! - **Stop** pauses until the next change, Resume, or the next start.
//! - **Background.** Before each chunk it waits for interactive work a
//!   person started, and says what it waits for.
//! - **Tracked.** [`status`] feeds the header's job line.

use crate::calibration::{backfill_frame_sites, SiteBackfill, SiteBackfillOutcome};
use crate::server::api::ApiResponse;
use crate::server::extract::DbContext;
use crate::server::state::AppState;
use axum::extract::State;
use axum::Json;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

/// How long requests must pause before a pass starts.
pub const SETTLE: Duration = Duration::from_secs(2);

/// The wait before a pass that could not finish is tried again; it doubles
/// with each try, up to [`RETRY_MAX`].
pub const RETRY_FIRST: Duration = Duration::from_secs(60);

/// The longest wait between tries.
pub const RETRY_MAX: Duration = Duration::from_secs(30 * 60);

/// Tries before the job waits for the next change, or the next start.
pub const RETRY_LIMIT: u32 = 6;

/// How often a waiting pass looks again.
const POLL: Duration = Duration::from_millis(250);

/// The longest nap of the runner while it waits.
const NAP_MAX: Duration = Duration::from_secs(1);

/// How long a commit waits for the catalog's write lock before the pass
/// tries it again itself.
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// Where a catalog's frame-sites job stands, for the job line.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct FrameSitesStatus {
    /// `idle`; `queued` while requests settle or a retry waits; `running`;
    /// or `stopped` when Stop paused it.
    pub state: String,
    /// Whether a pass found frames to read and reads them: what the job
    /// line shows.
    pub active: bool,
    pub done: usize,
    pub total: usize,
    /// While a pass runs, frames came since it began: another pass reads
    /// them after this one.
    pub next_pass: bool,
    /// What a person started that the pass waits for, while it does.
    pub waiting_for: Option<String>,
    /// When the next retry is due, Unix seconds.
    pub retry_at: Option<i64>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    /// What the last finished pass did.
    pub last: Option<SiteBackfillOutcome>,
    pub last_error: Option<String>,
}

#[derive(Debug, Default)]
struct Queue {
    pending: bool,
    /// When the last request came.
    last: Option<Instant>,
    running: bool,
    /// When a pass that could not finish is tried again.
    due: Option<Instant>,
    attempts: u32,
    stopped: bool,
    /// Whether a thread serves this catalog.
    worker: bool,
}

/// What the runner does next.
#[derive(Debug, PartialEq)]
enum Step {
    Exit,
    Nap(Duration),
    Run,
}

fn backoff(attempt: u32) -> Duration {
    RETRY_FIRST
        .saturating_mul(1 << attempt.saturating_sub(1).min(16))
        .min(RETRY_MAX)
}

impl Queue {
    /// The hourly watch's request: frames marked away are due again. It
    /// respects Stop, and leaves a retry in flight and its tries alone.
    fn nudge(&mut self, now: Instant) {
        if self.stopped || self.running || self.due.is_some() {
            return;
        }
        self.pending = true;
        self.last = Some(now);
    }

    /// A change, a start or Resume: tries start again and Stop is undone.
    fn add(&mut self, now: Instant) {
        self.pending = true;
        self.last = Some(now);
        self.stopped = false;
        self.attempts = 0;
        self.due = None;
    }

    fn next_step(&mut self, now: Instant) -> Step {
        if let Some(due) = self.due
            && due <= now
        {
            self.due = None;
            self.pending = true;
        }
        if !self.pending {
            return match self.due {
                Some(due) => Step::Nap(due.saturating_duration_since(now).clamp(POLL, NAP_MAX)),
                None => Step::Exit,
            };
        }
        let quiet = self
            .last
            .map_or(SETTLE, |last| now.saturating_duration_since(last));
        if quiet < SETTLE {
            return Step::Nap((SETTLE - quiet).clamp(POLL, NAP_MAX));
        }
        self.pending = false;
        self.last = None;
        Step::Run
    }

    /// Try again later; false once the tries are spent.
    fn retry(&mut self, now: Instant) -> bool {
        self.attempts += 1;
        if self.attempts > RETRY_LIMIT {
            self.due = None;
            return false;
        }
        self.due = Some(now + backoff(self.attempts));
        true
    }

    fn stop(&mut self) {
        self.stopped = self.pending || self.running || self.due.is_some() || self.stopped;
        self.pending = false;
        self.due = None;
        self.last = None;
        self.attempts = 0;
    }
}

#[derive(Default)]
struct Job {
    queue: Mutex<Queue>,
    status: RwLock<FrameSitesStatus>,
    /// Set by [`cancel`]: the running pass stops at its next chunk.
    cancelled: AtomicBool,
}

impl Job {
    fn update(&self, edit: impl FnOnce(&mut FrameSitesStatus)) {
        edit(&mut self.status.write().unwrap_or_else(|p| p.into_inner()));
    }

    fn queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|p| p.into_inner())
    }
}

static JOBS: LazyLock<Mutex<HashMap<String, Arc<Job>>>> = LazyLock::new(Default::default);

fn job(path: &str) -> Arc<Job> {
    Arc::clone(
        JOBS.lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(path.to_string())
            .or_default(),
    )
}

fn unix_at(at: Instant) -> i64 {
    let ahead = at.saturating_duration_since(Instant::now());
    chrono::Utc::now().timestamp() + ahead.as_secs() as i64
}

/// Where the frame-sites job stands for the catalog at `path`.
pub fn status(path: &str) -> FrameSitesStatus {
    let job = job(path);
    let queue = job.queue();
    let mut status = job.status.read().unwrap_or_else(|p| p.into_inner()).clone();
    status.retry_at = queue.due.map(unix_at);
    status.next_pass = queue.running && queue.pending;
    status.state = if queue.running {
        "running"
    } else if queue.pending || queue.due.is_some() {
        "queued"
    } else if queue.stopped {
        "stopped"
    } else {
        "idle"
    }
    .into();
    if !queue.running {
        status.active = false;
    }
    status
}

/// Read the sites of the catalog at `path`'s frames that have none yet.
pub fn queue(state: &Arc<AppState>, path: &str) {
    request(state, path, Queue::add);
}

/// [`queue`] for the hourly watch: nothing while Stop holds, or while a
/// retry waits, whose tries it leaves alone ([`Queue::nudge`]).
pub fn nudge(state: &Arc<AppState>, path: &str) {
    request(state, path, Queue::nudge);
}

fn request(state: &Arc<AppState>, path: &str, ask: fn(&mut Queue, Instant)) {
    if state.shutdown.is_stopping() {
        return;
    }
    let job = job(path);
    let spawn = {
        let mut queue = job.queue();
        ask(&mut queue, Instant::now());
        if !queue.pending && queue.due.is_none() {
            return;
        }
        !std::mem::replace(&mut queue.worker, true)
    };
    if !spawn {
        return;
    }
    let state = Arc::clone(state);
    let path = path.to_string();
    let runner = Arc::clone(&job);
    let started = std::thread::Builder::new()
        .name("frame-sites".into())
        .spawn(move || serve(&state, &path, &runner));
    if let Err(error) = started {
        tracing::warn!("Could not start reading frame sites: {error}");
        job.queue().worker = false;
    }
}

/// Stop: the running pass ends at its next chunk, and nothing more runs
/// until a change, [`resume`], or the next start.
pub fn cancel(path: &str) -> FrameSitesStatus {
    let job = job(path);
    job.queue().stop();
    job.cancelled.store(true, Ordering::SeqCst);
    status(path)
}

/// Take up what Stop paused.
pub fn resume(state: &Arc<AppState>, path: &str) -> FrameSitesStatus {
    let stopped = std::mem::take(&mut job(path).queue().stopped);
    if stopped {
        queue(state, path);
    }
    status(path)
}

/// Lets go of the runner's claim if it ends without doing so, as on a
/// panic, so the next request starts another.
struct RunnerClaim<'a> {
    job: &'a Job,
    released: bool,
}

impl Drop for RunnerClaim<'_> {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let mut queue = self.job.queue();
        queue.worker = false;
        queue.running = false;
        drop(queue);
        self.job.update(|status| {
            status.active = false;
            status.waiting_for = None;
            status.last_error = Some("reading frame sites stopped unexpectedly".into());
        });
    }
}

fn serve(state: &Arc<AppState>, path: &str, job: &Job) {
    let mut claim = RunnerClaim {
        job,
        released: false,
    };
    loop {
        if state.shutdown.is_stopping() {
            job.queue().worker = false;
            claim.released = true;
            return;
        }
        let step = {
            let mut queue = job.queue();
            let step = queue.next_step(Instant::now());
            match step {
                Step::Exit => {
                    queue.worker = false;
                    claim.released = true;
                    return;
                }
                Step::Run => {
                    queue.running = true;
                    job.cancelled.store(false, Ordering::SeqCst);
                }
                Step::Nap(_) => {}
            }
            step
        };
        if let Step::Nap(wait) = step {
            std::thread::sleep(wait);
            continue;
        }
        let _work = state.shutdown.hold(format!("frame sites ({path})"));
        run_logged(state, path, job);
    }
}

fn run_logged(state: &Arc<AppState>, path: &str, job: &Job) {
    job.update(|status| {
        *status = FrameSitesStatus {
            started_at: Some(chrono::Utc::now().timestamp()),
            last: status.last.take(),
            ..Default::default()
        };
    });
    let result = run(state, path, job);
    let cancelled = job.cancelled.load(Ordering::SeqCst);
    let unfinished = match &result {
        Ok(outcome) => outcome.volume_away || outcome.error.is_some(),
        Err(_) => true,
    };
    {
        let mut queue = job.queue();
        queue.running = false;
        if cancelled && !queue.pending {
            queue.stopped = true;
        } else if cancelled {
            // A change came after Stop: it runs.
        } else if unfinished {
            queue.retry(Instant::now());
        } else {
            queue.attempts = 0;
        }
    }
    match &result {
        Ok(outcome) if outcome.pending > 0 => tracing::info!(
            "Read calibration frame sites in {path}: {} by longitude, {} by clock, {} with \
             neither, {} with files away{}{}",
            outcome.located,
            outcome.clocked,
            outcome.unplaced,
            outcome.unreadable,
            if outcome.volume_away {
                "; no file could be read, as if its volume were away, so it is tried again"
            } else {
                ""
            },
            outcome
                .error
                .as_ref()
                .map(|error| format!("; stopped early ({error}), tried again shortly"))
                .unwrap_or_default(),
        ),
        Ok(_) => {}
        Err(error) => tracing::info!(
            "Paused reading calibration frame sites in {path}: {error:#}. It is tried again \
             shortly."
        ),
    }
    job.update(|status| {
        status.active = false;
        status.waiting_for = None;
        status.finished_at = Some(chrono::Utc::now().timestamp());
        match result {
            Ok(outcome) => {
                status.last_error = outcome.error.clone().or_else(|| {
                    outcome
                        .volume_away
                        .then(|| "no frame file could be read; is its volume mounted?".into())
                });
                status.last = Some(outcome);
            }
            Err(error) => status.last_error = Some(format!("{error:#}")),
        }
    });
}

fn run(state: &Arc<AppState>, path: &str, job: &Job) -> anyhow::Result<SiteBackfillOutcome> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    if conn.is_readonly(rusqlite::MAIN_DB)? {
        return Ok(Default::default());
    }
    run_on(state, path, job, &conn)
}

fn run_on(
    state: &Arc<AppState>,
    path: &str,
    job: &Job,
    conn: &Connection,
) -> anyhow::Result<SiteBackfillOutcome> {
    let keep_going = || !state.shutdown.is_stopping() && !job.cancelled.load(Ordering::SeqCst);
    let yield_now = || {
        state.wait_for_interactive_work(POLL, |waiting_for| {
            job.update(|status| status.waiting_for = waiting_for.map(str::to_string));
        });
    };
    let mut progress = |done: usize, total: usize| {
        job.update(|status| {
            status.done = done;
            status.total = total;
            status.active |= total > 0 && done < total;
        });
    };
    let mut placed = |rigs: &[String]| {
        crate::server::flat_measure::sites_found(state, path, rigs.to_vec());
    };
    backfill_frame_sites(
        conn,
        SiteBackfill {
            keep_going: &keep_going,
            yield_now: &yield_now,
            progress: &mut progress,
            placed: &mut placed,
        },
    )
}

/// How often the server looks for frames marked away that are due to be
/// read again.
pub const AWAY_WATCH: Duration = Duration::from_secs(60 * 60);

/// Every [`AWAY_WATCH`], queue a pass for each catalog with frames marked
/// away a week or more ago, so they are read again without waiting for an
/// import or a restart.
pub fn spawn_away_watch(state: &Arc<AppState>) {
    let state = Arc::clone(state);
    tokio::spawn(async move {
        while state.shutdown.sleep(AWAY_WATCH).await {
            let paths: Vec<String> = state
                .all_databases()
                .into_iter()
                .map(|ctx| ctx.database_path.clone())
                .collect();
            let due: Vec<String> = tokio::task::spawn_blocking(move || {
                paths
                    .into_iter()
                    .filter(|path| {
                        Connection::open_with_flags(
                            path,
                            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                        )
                        .is_ok_and(|conn| crate::calibration::night::away_frames_due(&conn))
                    })
                    .collect()
            })
            .await
            .unwrap_or_default();
            for path in due {
                nudge(&state, &path);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Routes.

/// `GET /api/db/{db}/calibrations/frame-sites` — where the job stands.
pub async fn get_status(ctx: DbContext) -> Json<ApiResponse<FrameSitesStatus>> {
    Json(ApiResponse::success(status(&ctx.database_path)))
}

/// `POST /api/db/{db}/calibrations/frame-sites/cancel` — Stop.
pub async fn post_cancel(ctx: DbContext) -> Json<ApiResponse<FrameSitesStatus>> {
    Json(ApiResponse::success(cancel(&ctx.database_path)))
}

/// `POST /api/db/{db}/calibrations/frame-sites/resume` — take up what
/// Stop paused.
pub async fn post_resume(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Json<ApiResponse<FrameSitesStatus>> {
    Json(ApiResponse::success(resume(&state, &ctx.database_path)))
}

#[cfg(test)]
mod tests;
