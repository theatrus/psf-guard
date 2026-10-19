//! Background jobs that run one thread per catalog.
//!
//! Flat measuring, frame sites, calibration grading, calibration health, the
//! duplicate check and reject removal each keep a queue per catalog. The
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
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use crate::concurrency::Stop;
use crate::server::state::AppState;

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
    /// shutdown waits for this call to return. [`CatalogJob::stop_running`]
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

    /// The runner thread panicked or could not start. Clear the running
    /// state from the queue and the status, and record the error.
    fn runner_failed(queue: &mut Self::Queue, status: &mut Self::Status);
}

/// One catalog's job.
pub struct CatalogJob<W: CatalogWork> {
    queue: Mutex<Queued<W::Queue>>,
    status: RwLock<W::Status>,
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

    /// Request a stop of the running item, which stops at its next check.
    /// Returns false when no item is running.
    pub fn stop_running(&self) -> bool {
        let queued = self.lock();
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
            let started = std::thread::Builder::new()
                .name(W::RUNNER.into())
                .spawn(move || run_queue(&state, &catalog, &runner_job));
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
                    return;
                }
                Step::Sleep(wait) => {
                    drop(queued);
                    std::thread::sleep(wait);
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
        assert!(!job.stop_running(), "no item is running");
        assert_eq!(COUNTING.catalogs(), ["catalog.sqlite"]);
    }
}
