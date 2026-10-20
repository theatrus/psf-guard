//! Background jobs that run one thread per catalog.
//!
//! Each catalog job keeps a queue per catalog: flat measuring, frame sites,
//! calibration grading and health, the duplicate check, reject removal,
//! quality scans, arrivals, the Calibration page's header reads, flat group
//! use, catalog upkeep, the stack preparation line, Target Scheduler imports
//! and syncs, and backups. The registry of them is
//! `crate::server::catalog_jobs::CatalogJobRegistry`, on `AppState`. The
//! queue stores merged requests, retries that are due later, and work that
//! Stop paused. While a catalog's queue has work, one runner thread
//! processes it one item at a time; the thread exits when the queue is
//! empty.
//!
//! [`CatalogJobs`] implements the shared parts: the job of each catalog,
//! starting the runner thread, the stop flag of the item that is running,
//! and recovery when a runner thread panics or cannot start. Each job
//! implements [`CatalogWork`] to define its queue, when an item is due, and
//! how to run it.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use crate::concurrency::Stop;
use crate::server::state::AppState;

/// The longest a sleeping runner thread waits before it looks again, so it
/// sees a server stop.
const SHUTDOWN_LOOK: Duration = Duration::from_secs(1);

/// What a catalog's runner thread does next.
#[derive(Debug, PartialEq, Eq)]
pub enum Step<Item> {
    /// The queue is empty: the runner thread exits.
    Exit,
    /// Work is queued but not due yet: check again after this long.
    Sleep(Duration),
    /// Run this item now.
    Run(Item),
}

/// Requests that run once they stop arriving: `quiet` after the last one,
/// but no later than `longest` after the first, so a steady stream still
/// runs. A job keeps one in its queue and reads it in [`CatalogWork::next`].
#[derive(Debug, Default, Clone)]
pub struct Settling {
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Settling {
    /// A request arrived at `now`.
    pub fn touch(&mut self, now: Instant) {
        self.first.get_or_insert(now);
        self.last = Some(now);
    }

    /// Whether a request waits to settle.
    pub fn is_pending(&self) -> bool {
        self.last.is_some()
    }

    /// When the requests settle: `quiet` after the last, or `longest` after
    /// the first, whichever comes sooner. `None` when nothing waits.
    pub fn due_at(&self, quiet: Duration, longest: Option<Duration>) -> Option<Instant> {
        let quiet_at = self.last? + quiet;
        Some(match (self.first, longest) {
            (Some(first), Some(longest)) => quiet_at.min(first + longest),
            _ => quiet_at,
        })
    }

    /// The runner's next step for these requests: [`Step::Exit`] when none
    /// waits, [`Step::Sleep`] until they settle, or [`Step::Run`] once they
    /// have, which clears them.
    pub fn step(&mut self, now: Instant, quiet: Duration, longest: Option<Duration>) -> Step<()> {
        let Some(due) = self.due_at(quiet, longest) else {
            return Step::Exit;
        };
        if now < due {
            return Step::Sleep(due - now);
        }
        *self = Self::default();
        Step::Run(())
    }
}

/// One kind of background job that runs one thread per catalog.
pub trait CatalogWork: Sized + Send + Sync + 'static {
    /// The pending work: merged requests, retries and work that Stop paused.
    type Queue: Default + Send;
    /// Progress and results for the status routes, apart from the queue.
    type Status: Default + Clone + Send + Sync;
    /// One unit of work for the runner thread.
    type Item: Send;
    /// The runner thread's name, such as `flat-measure`.
    const RUNNER: &'static str;
    /// The label that server shutdown shows while it waits for an item to
    /// finish, such as `flat measuring`. `None` when shutdown does not wait:
    /// the job then registers its own shutdown hold around any write that
    /// must finish.
    const SHUTDOWN_WAITS_FOR: Option<&'static str>;

    /// Choose the runner thread's next step. Called with the queue locked.
    /// `state` and `catalog` let a job check another job, as flat measuring
    /// checks whether grading is running.
    fn next(
        state: &AppState,
        catalog: &str,
        queue: &mut Self::Queue,
        now: Instant,
    ) -> Step<Self::Item>;

    /// Run one item. When [`Self::SHUTDOWN_WAITS_FOR`] is set, server
    /// shutdown waits for this call to return. [`CatalogJob::stop`]
    /// requests `stop`.
    fn run(
        state: &Arc<AppState>,
        catalog: &str,
        job: &CatalogJob<Self>,
        item: Self::Item,
        stop: &Stop,
    );

    /// Whether the queue has work, due now or later. A request that leaves
    /// the queue without work does not start a runner thread.
    fn has_work(_queue: &Self::Queue) -> bool {
        true
    }

    /// Called on the runner thread after it found the queue empty and
    /// exited, outside the queue lock; not during server shutdown. A job
    /// that hands its results to another job requests that job here, once
    /// per batch of items.
    fn after_queue_empty(_state: &Arc<AppState>, _catalog: &str) {}

    /// The runner thread panicked or could not start. Clear the running
    /// state from the queue and the status, and record the error. It runs
    /// under the queue lock: a lock it takes must never be held by a caller
    /// of [`CatalogJob::stop`] or [`CatalogJobs::request`].
    fn runner_failed(queue: &mut Self::Queue, status: &mut Self::Status);
}

/// One catalog's job.
pub struct CatalogJob<W: CatalogWork> {
    queue: Mutex<Queued<W::Queue>>,
    status: RwLock<W::Status>,
    /// Wakes a runner thread that sleeps until queued work is due, when a
    /// request changes the queue.
    wake: Condvar,
}

struct Queued<Q> {
    queue: Q,
    /// Whether a runner thread exists for the catalog.
    runner_active: bool,
    /// The stop flag of the running item, when an item is running.
    current_stop: Option<Stop>,
}

impl<Q: Default> Default for Queued<Q> {
    fn default() -> Self {
        Self {
            queue: Q::default(),
            runner_active: false,
            current_stop: None,
        }
    }
}

impl<W: CatalogWork> Default for CatalogJob<W> {
    fn default() -> Self {
        Self {
            queue: Mutex::new(Queued::default()),
            status: RwLock::new(W::Status::default()),
            wake: Condvar::new(),
        }
    }
}

/// The catalog's queue, locked.
pub struct QueueGuard<'a, Q>(MutexGuard<'a, Queued<Q>>);

impl<Q> std::ops::Deref for QueueGuard<'_, Q> {
    type Target = Q;
    fn deref(&self) -> &Q {
        &self.0.queue
    }
}

impl<Q> std::ops::DerefMut for QueueGuard<'_, Q> {
    fn deref_mut(&mut self) -> &mut Q {
        &mut self.0.queue
    }
}

impl<Q> QueueGuard<'_, Q> {
    /// Whether an item is running now.
    pub fn running(&self) -> bool {
        self.0.current_stop.is_some()
    }

    /// Whether a runner thread exists for the catalog, running an item or
    /// sleeping until one is due.
    pub fn has_runner(&self) -> bool {
        self.0.runner_active
    }
}

impl<W: CatalogWork> CatalogJob<W> {
    fn lock(&self) -> MutexGuard<'_, Queued<W::Queue>> {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The catalog's queue, locked.
    pub fn queue(&self) -> QueueGuard<'_, W::Queue> {
        QueueGuard(self.lock())
    }

    /// A copy of the job's status.
    pub fn status(&self) -> W::Status {
        self.status
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Change the job's status.
    pub fn update(&self, edit: impl FnOnce(&mut W::Status)) {
        edit(
            &mut self
                .status
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
    }

    /// Reset the runner state after the runner thread panicked or could
    /// not start, so the next request starts a new thread.
    fn runner_failed(&self) {
        let mut queued = self.lock();
        queued.runner_active = false;
        queued.current_stop = None;
        let mut status = self
            .status
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        W::runner_failed(&mut queued.queue, &mut status);
    }

    /// Stop the job: change the queue with `edit`, such as to pause or
    /// remove pending work, and request a stop of the running item, which
    /// stops at its next check. Both happen under the queue lock, so the
    /// runner thread cannot start or finish an item in between. Returns
    /// false when no item was running.
    pub fn stop(&self, edit: impl FnOnce(&mut W::Queue)) -> bool {
        self.stop_if(|queue| {
            edit(queue);
            true
        })
    }

    /// Request a stop of the running item when `test`, called with the
    /// queue under the queue lock, returns true. The test can read what
    /// runs from the queue, such as to stop only automatic work. Returns
    /// false when no stop was requested.
    pub fn stop_if(&self, test: impl FnOnce(&mut W::Queue) -> bool) -> bool {
        let mut queued = self.lock();
        if !test(&mut queued.queue) {
            return false;
        }
        if let Some(stop) = &queued.current_stop {
            stop.request();
            true
        } else {
            false
        }
    }
}

/// The jobs of one kind, one per catalog.
pub struct CatalogJobs<W: CatalogWork> {
    jobs: LazyLock<Mutex<HashMap<String, Arc<CatalogJob<W>>>>>,
}

impl<W: CatalogWork> Default for CatalogJobs<W> {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: CatalogWork> CatalogJobs<W> {
    pub const fn new() -> Self {
        Self {
            jobs: LazyLock::new(Default::default),
        }
    }

    /// The job of the catalog at path `catalog`, created when missing.
    pub fn job(&self, catalog: &str) -> Arc<CatalogJob<W>> {
        Arc::clone(
            self.jobs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .entry(catalog.to_string())
                .or_default(),
        )
    }

    /// Change the catalog's queue with `edit` and return its result. Then
    /// start a runner thread when none exists and the queue has work. An
    /// existing runner thread reads the change at its next step. No thread
    /// starts during server shutdown.
    pub fn request<R>(
        &self,
        state: &Arc<AppState>,
        catalog: &str,
        edit: impl FnOnce(&mut W::Queue) -> R,
    ) -> R {
        let job = self.job(catalog);
        let (result, start) = {
            let mut queued = job.lock();
            let result = edit(&mut queued.queue);
            // A runner that sleeps until queued work is due reads the change
            // now.
            job.wake.notify_all();
            let start = !queued.runner_active
                && !state.shutdown.is_stopping()
                && W::has_work(&queued.queue);
            if start {
                queued.runner_active = true;
            }
            (result, start)
        };
        if start {
            let (state, catalog, runner_job) =
                (Arc::clone(state), catalog.to_string(), Arc::clone(&job));
            // Work a runner starts on the async runtime, such as queueing a
            // scan, needs a runtime on its thread.
            let runtime = state.runtime();
            let started = std::thread::Builder::new()
                .name(W::RUNNER.into())
                .spawn(move || {
                    let _runtime = runtime.as_ref().map(|runtime| runtime.enter());
                    run_queue(&state, &catalog, &runner_job)
                });
            if let Err(error) = started {
                tracing::warn!("Could not start {}: {error}", W::RUNNER);
                job.runner_failed();
            }
        }
        result
    }

    /// The job of the catalog at path `catalog`, if one was created.
    pub fn existing(&self, catalog: &str) -> Option<Arc<CatalogJob<W>>> {
        self.jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(catalog)
            .cloned()
    }

    /// The paths of every catalog that has a job of this kind.
    pub fn catalogs(&self) -> Vec<String> {
        self.jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keys()
            .cloned()
            .collect()
    }
}

/// Resets the runner state when the runner thread unwinds from a panic.
struct PanicGuard<'a, W: CatalogWork> {
    job: &'a CatalogJob<W>,
    exited: bool,
}

impl<W: CatalogWork> Drop for PanicGuard<'_, W> {
    fn drop(&mut self) {
        if !self.exited {
            self.job.runner_failed();
        }
    }
}

/// The runner thread of a catalog. It runs each item when it is due,
/// sleeps while queued work is not due, and exits when the queue is empty
/// or the server shuts down.
fn run_queue<W: CatalogWork>(state: &Arc<AppState>, catalog: &str, job: &CatalogJob<W>) {
    let mut guard = PanicGuard { job, exited: false };
    loop {
        let (item, stop) = {
            let mut queued = job.lock();
            if state.shutdown.is_stopping() {
                queued.runner_active = false;
                guard.exited = true;
                return;
            }
            match W::next(state, catalog, &mut queued.queue, Instant::now()) {
                Step::Exit => {
                    queued.runner_active = false;
                    guard.exited = true;
                    drop(queued);
                    W::after_queue_empty(state, catalog);
                    return;
                }
                Step::Sleep(wait) => {
                    // A request wakes it early; the cap lets it see a
                    // server stop soon, which wakes no one.
                    let (queued, _) = job
                        .wake
                        .wait_timeout(queued, wait.min(SHUTDOWN_LOOK))
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    drop(queued);
                    continue;
                }
                Step::Run(item) => {
                    let stop = Stop::new();
                    queued.current_stop = Some(stop.clone());
                    (item, stop)
                }
            }
        };
        {
            let _shutdown_hold = W::SHUTDOWN_WAITS_FOR
                .map(|what| state.shutdown.hold(format!("{what} ({catalog})")));
            W::run(state, catalog, job, item, &stop);
        }
        job.lock().current_stop = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static RUNS: AtomicUsize = AtomicUsize::new(0);

    /// Queues numbers and records each one it runs.
    struct Counting;

    impl CatalogWork for Counting {
        type Queue = Vec<u32>;
        type Status = Vec<u32>;
        type Item = u32;
        const RUNNER: &'static str = "counting";
        const SHUTDOWN_WAITS_FOR: Option<&'static str> = Some("counting");

        fn next(_: &AppState, _: &str, queue: &mut Vec<u32>, _: Instant) -> Step<u32> {
            queue.pop().map_or(Step::Exit, Step::Run)
        }

        fn run(_: &Arc<AppState>, _: &str, job: &CatalogJob<Self>, item: u32, stop: &Stop) {
            assert!(!stop.requested());
            RUNS.fetch_add(1, Ordering::SeqCst);
            job.update(|done| done.push(item));
        }

        fn runner_failed(queue: &mut Vec<u32>, _: &mut Vec<u32>) {
            queue.clear();
        }
    }

    static COUNTING: CatalogJobs<Counting> = CatalogJobs::new();

    #[test]
    fn a_catalog_runner_runs_each_item_then_exits() {
        let state = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        for item in [1, 2, 3] {
            COUNTING.request(&state, "catalog.sqlite", |queue| queue.push(item));
        }
        let job = COUNTING.job("catalog.sqlite");
        for _ in 0..200 {
            if !job.queue().has_runner() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut done = job.status();
        done.sort_unstable();
        assert_eq!(done, [1, 2, 3]);
        assert!(!job.queue().running());
        assert!(!job.stop(|_| {}), "no item is running");
        assert_eq!(COUNTING.catalogs(), ["catalog.sqlite"]);
    }

    #[test]
    fn stop_if_requests_the_running_items_stop_only_when_the_test_passes() {
        let job = CatalogJob::<Counting>::default();
        let stop = Stop::new();
        job.lock().current_stop = Some(stop.clone());
        assert!(!job.stop_if(|queue| !queue.is_empty()));
        assert!(!stop.requested(), "the test failed: nothing is stopped");
        job.queue().push(3);
        assert!(job.stop_if(|queue| !queue.is_empty()));
        assert!(stop.requested());
    }

    struct Sleeper;

    impl CatalogWork for Sleeper {
        /// When the queued number is due.
        type Queue = Option<(Instant, u32)>;
        type Status = Vec<u32>;
        type Item = u32;
        const RUNNER: &'static str = "sleeper";
        const SHUTDOWN_WAITS_FOR: Option<&'static str> = None;

        fn next(_: &AppState, _: &str, queue: &mut Self::Queue, now: Instant) -> Step<u32> {
            match *queue {
                None => Step::Exit,
                Some((due, _)) if now < due => Step::Sleep(due - now),
                Some((_, item)) => {
                    *queue = None;
                    Step::Run(item)
                }
            }
        }

        fn run(_: &Arc<AppState>, _: &str, job: &CatalogJob<Self>, item: u32, _: &Stop) {
            job.update(|done| done.push(item));
        }

        fn runner_failed(_: &mut Self::Queue, _: &mut Vec<u32>) {}
    }

    struct RuntimeCheck;

    impl CatalogWork for RuntimeCheck {
        type Queue = bool;
        /// Whether the run found an async runtime.
        type Status = Option<bool>;
        type Item = ();
        const RUNNER: &'static str = "runtime-check";
        const SHUTDOWN_WAITS_FOR: Option<&'static str> = None;

        fn next(_: &AppState, _: &str, queue: &mut bool, _: Instant) -> Step<()> {
            if std::mem::take(queue) {
                Step::Run(())
            } else {
                Step::Exit
            }
        }

        fn run(_: &Arc<AppState>, _: &str, job: &CatalogJob<Self>, (): (), _: &Stop) {
            let found = tokio::runtime::Handle::try_current().is_ok();
            job.update(|status| *status = Some(found));
        }

        fn runner_failed(_: &mut bool, _: &mut Option<bool>) {}
    }

    #[test]
    fn a_runner_started_off_the_runtime_can_start_async_work() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let state = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        state.set_runtime(runtime.handle().clone());
        let jobs = Arc::new(CatalogJobs::<RuntimeCheck>::new());
        // A sync runner thread, say, raises the request: no runtime there.
        let (requester_state, requester_jobs) = (Arc::clone(&state), Arc::clone(&jobs));
        std::thread::spawn(move || {
            requester_jobs.request(&requester_state, "c", |queue| *queue = true)
        })
        .join()
        .unwrap();
        let job = jobs.job("c");
        let started = Instant::now();
        while job.status().is_none() {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "the runner never ran"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(job.status(), Some(true));
    }

    #[test]
    fn a_request_wakes_a_runner_that_sleeps_until_work_is_due() {
        let jobs = CatalogJobs::<Sleeper>::new();
        let state = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        // Due in an hour: the runner sleeps.
        jobs.request(&state, "c", |queue| {
            *queue = Some((Instant::now() + Duration::from_secs(3600), 1))
        });
        std::thread::sleep(Duration::from_millis(50));
        // Due now: the runner wakes and runs it at once.
        let started = Instant::now();
        jobs.request(&state, "c", |queue| *queue = Some((Instant::now(), 2)));
        let job = jobs.job("c");
        while job.status().is_empty() {
            assert!(
                started.elapsed() < Duration::from_millis(500),
                "the runner kept sleeping"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(job.status(), [2]);
    }

    #[test]
    fn requests_run_once_they_settle_or_the_longest_wait_passes() {
        let start = Instant::now();
        let quiet = Duration::from_secs(10);
        let longest = Some(Duration::from_secs(25));
        let mut settling = Settling::default();
        assert_eq!(settling.step(start, quiet, longest), Step::Exit);
        settling.touch(start);
        assert_eq!(
            settling.step(start, quiet, longest),
            Step::Sleep(Duration::from_secs(10))
        );
        // Each request pushes it out, but not past the longest wait.
        settling.touch(start + Duration::from_secs(8));
        settling.touch(start + Duration::from_secs(16));
        assert_eq!(
            settling.step(start + Duration::from_secs(20), quiet, longest),
            Step::Sleep(Duration::from_secs(5))
        );
        assert_eq!(
            settling.step(start + Duration::from_secs(25), quiet, longest),
            Step::Run(())
        );
        assert!(!settling.is_pending(), "running clears it");
    }

    #[test]
    fn stop_edits_the_queue_and_requests_the_running_items_stop() {
        let job = CatalogJob::<Counting>::default();
        job.queue().push(7);
        assert!(!job.stop(|queue| queue.clear()));
        assert!(job.queue().is_empty(), "the pending item was removed");

        // While an item runs, its stop flag is requested as well.
        let stop = Stop::new();
        job.lock().current_stop = Some(stop.clone());
        job.queue().push(8);
        assert!(job.stop(|queue| queue.clear()));
        assert!(stop.requested());
        assert!(job.queue().is_empty());
    }
}
