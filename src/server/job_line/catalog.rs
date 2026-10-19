//! Jobs with one runner per catalog.
//!
//! Flat measuring, frame sites, calibration grading and health, the
//! duplicate check and reject removal each keep a queue per catalog: what
//! was asked for, merged, and what waits for a retry or was paused. One
//! runner thread per catalog takes what is due, one piece at a time, and
//! lets go of the catalog when nothing is left. [`CatalogJobs`] holds that
//! part once: the catalogs' jobs, starting a runner when one is needed,
//! the stop flag of the piece running, and a runner that ends without
//! letting go, as on a panic. What each job queues, when it runs, and what
//! it does stay its own ([`CatalogWork`]).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use crate::concurrency::Stop;
use crate::server::state::AppState;

/// What a catalog's runner does next.
#[derive(Debug, PartialEq, Eq)]
pub enum Step<Item> {
    /// Nothing waits: the runner lets go of the catalog.
    Exit,
    /// Something waits, but not yet: look again after this long.
    Nap(Duration),
    /// Run this now.
    Run(Item),
}

/// One kind of job with a runner per catalog.
pub trait CatalogWork: Sized + Send + Sync + 'static {
    /// What waits: requests merged, retries, what Stop paused.
    type Queue: Default + Send;
    /// What the job reports, beside what its queue says.
    type Status: Default + Clone + Send + Sync;
    /// One piece of work the runner takes.
    type Item: Send;
    /// The runner thread's name, as `flat-measure`.
    const RUNNER: &'static str;
    /// What a server stop waits for while a piece runs, as `flat
    /// measuring`; `None` for a job that holds the stop itself, around
    /// what must not be cut short.
    const STOP_WAITS_FOR: Option<&'static str>;

    /// What the runner does next, read with the queue locked. `state` and
    /// `catalog` are there for a job that waits on another, as flat
    /// measuring waits for grading.
    fn next(
        state: &AppState,
        catalog: &str,
        queue: &mut Self::Queue,
        now: Instant,
    ) -> Step<Self::Item>;

    /// Run one piece, holding the server's stop until it ends when
    /// [`Self::STOP_WAITS_FOR`] says so. `stop` is this piece's: the job's
    /// Stop sets it.
    fn run(
        state: &Arc<AppState>,
        catalog: &str,
        job: &CatalogJob<Self>,
        item: Self::Item,
        stop: &Stop,
    );

    /// Whether the queue has anything for a runner to do, now or later. A
    /// request that leaves nothing starts no runner.
    fn waiting(_queue: &Self::Queue) -> bool {
        true
    }

    /// The runner ended without letting go, as on a panic, or could not
    /// start: settle what the queue and the status say.
    fn runner_lost(queue: &mut Self::Queue, status: &mut Self::Status);
}

/// One catalog's job.
pub struct CatalogJob<W: CatalogWork> {
    queue: Mutex<Queued<W::Queue>>,
    status: RwLock<W::Status>,
}

struct Queued<Q> {
    queue: Q,
    /// A runner serves the catalog.
    runner: bool,
    /// The stop of the piece running, if one is.
    running: Option<Stop>,
}

impl<Q: Default> Default for Queued<Q> {
    fn default() -> Self {
        Self {
            queue: Q::default(),
            runner: false,
            running: None,
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

/// The catalog's queue, held locked.
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
    /// Whether a piece is running now.
    pub fn running(&self) -> bool {
        self.0.running.is_some()
    }

    /// Whether a runner serves the catalog, running or waiting.
    pub fn has_runner(&self) -> bool {
        self.0.runner
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

    /// What the job reports now.
    pub fn status(&self) -> W::Status {
        self.status
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Change what the job reports.
    pub fn update(&self, edit: impl FnOnce(&mut W::Status)) {
        edit(
            &mut self
                .status
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
    }

    /// The runner ended without letting go, or never started.
    fn lost(&self) {
        let mut queued = self.lock();
        queued.runner = false;
        queued.running = None;
        let mut status = self
            .status
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        W::runner_lost(&mut queued.queue, &mut status);
    }

    /// Ask the piece running, if any, to stop at its next step. Returns
    /// whether one was running.
    pub fn stop_running(&self) -> bool {
        let queued = self.lock();
        if let Some(stop) = &queued.running {
            stop.request();
            true
        } else {
            false
        }
    }
}

/// Every catalog's job of one kind.
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

    /// The job of the catalog at `catalog`.
    pub fn job(&self, catalog: &str) -> Arc<CatalogJob<W>> {
        Arc::clone(
            self.jobs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .entry(catalog.to_string())
                .or_default(),
        )
    }

    /// Change the catalog's queue with `edit`, then start its runner unless
    /// one serves it already, which sees the change, or nothing waits. Returns what `edit`
    /// returned. Nothing starts while the server stops.
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
            let start =
                !queued.runner && !state.shutdown.is_stopping() && W::waiting(&queued.queue);
            if start {
                queued.runner = true;
            }
            (result, start)
        };
        if start {
            let (state, catalog, runner_job) =
                (Arc::clone(state), catalog.to_string(), Arc::clone(&job));
            let started = std::thread::Builder::new()
                .name(W::RUNNER.into())
                .spawn(move || serve(&state, &catalog, &runner_job));
            if let Err(error) = started {
                tracing::warn!("Could not start {}: {error}", W::RUNNER);
                job.lost();
            }
        }
        result
    }

    /// Every catalog that has a job of this kind.
    pub fn catalogs(&self) -> Vec<String> {
        self.jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keys()
            .cloned()
            .collect()
    }
}

/// Settles the runner's claim if it ends without letting go, as on a panic,
/// so the next request starts another.
struct RunnerClaim<'a, W: CatalogWork> {
    job: &'a CatalogJob<W>,
    released: bool,
}

impl<W: CatalogWork> Drop for RunnerClaim<'_, W> {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.job.lost();
    }
}

/// A catalog's one runner: it takes what is due, naps while nothing is, and
/// lets go of the catalog when nothing waits or the server stops.
fn serve<W: CatalogWork>(state: &Arc<AppState>, catalog: &str, job: &CatalogJob<W>) {
    let mut claim = RunnerClaim {
        job,
        released: false,
    };
    loop {
        let (item, stop) = {
            let mut queued = job.lock();
            if state.shutdown.is_stopping() {
                queued.runner = false;
                claim.released = true;
                return;
            }
            match W::next(state, catalog, &mut queued.queue, Instant::now()) {
                Step::Exit => {
                    queued.runner = false;
                    claim.released = true;
                    return;
                }
                Step::Nap(wait) => {
                    drop(queued);
                    std::thread::sleep(wait);
                    continue;
                }
                Step::Run(item) => {
                    let stop = Stop::new();
                    queued.running = Some(stop.clone());
                    (item, stop)
                }
            }
        };
        {
            let _work =
                W::STOP_WAITS_FOR.map(|what| state.shutdown.hold(format!("{what} ({catalog})")));
            W::run(state, catalog, job, item, &stop);
        }
        job.lock().running = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static RUNS: AtomicUsize = AtomicUsize::new(0);

    /// Counts what it is asked for, and runs each count once.
    struct Counting;

    impl CatalogWork for Counting {
        type Queue = Vec<u32>;
        type Status = Vec<u32>;
        type Item = u32;
        const RUNNER: &'static str = "counting";
        const STOP_WAITS_FOR: Option<&'static str> = Some("counting");

        fn next(_: &AppState, _: &str, queue: &mut Vec<u32>, _: Instant) -> Step<u32> {
            queue.pop().map_or(Step::Exit, Step::Run)
        }

        fn run(_: &Arc<AppState>, _: &str, job: &CatalogJob<Self>, item: u32, stop: &Stop) {
            assert!(!stop.requested());
            RUNS.fetch_add(1, Ordering::SeqCst);
            job.update(|done| done.push(item));
        }

        fn runner_lost(queue: &mut Vec<u32>, _: &mut Vec<u32>) {
            queue.clear();
        }
    }

    static COUNTING: CatalogJobs<Counting> = CatalogJobs::new();

    #[test]
    fn a_catalog_runner_takes_what_is_due_and_lets_go() {
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
        assert!(!job.stop_running(), "nothing runs");
        assert_eq!(COUNTING.catalogs(), ["catalog.sqlite"]);
    }
}
