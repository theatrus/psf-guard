//! The server's stop.
//!
//! A stop begins once, when Ctrl+C, a service manager's SIGTERM or the
//! desktop app asks for it. Background loops then start nothing new, long
//! jobs stop at their next safe point (between frames, after a committed
//! chunk, at a stack checkpoint), and the server waits for the work in hand
//! to finish before it exits, so no file or catalog write is cut off.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// How often a stop that is still waiting says what it waits for.
const REPORT_EVERY: Duration = Duration::from_secs(5);

#[derive(Debug, Default)]
pub struct Shutdown {
    token: CancellationToken,
    /// Work a stop waits for, by a number of its own, with what it is.
    work: Mutex<BTreeMap<u64, String>>,
    next: AtomicU64,
    /// Wakes a waiting stop when a piece of work finishes.
    finished: tokio::sync::Notify,
}

impl Shutdown {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Begin the stop. Later calls change nothing.
    pub fn begin(&self) {
        self.token.cancel();
    }

    /// Whether the stop has begun. Long work reads this between units.
    pub fn is_stopping(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Resolves once the stop begins.
    pub async fn begun(&self) {
        self.token.cancelled().await
    }

    /// Sleep for `duration` unless the stop begins first. False when it has.
    pub async fn sleep(&self, duration: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(duration) => !self.is_stopping(),
            _ = self.token.cancelled() => false,
        }
    }

    /// Mark work the stop must wait for, until the guard drops.
    pub fn hold(self: &Arc<Self>, what: impl Into<String>) -> WorkGuard {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.work.lock().unwrap().insert(id, what.into());
        WorkGuard {
            shutdown: Arc::clone(self),
            id,
        }
    }

    /// What the stop still waits for, oldest first.
    pub fn running(&self) -> Vec<String> {
        self.work.lock().unwrap().values().cloned().collect()
    }

    /// Wait until every held piece of work has finished, saying every few
    /// seconds what is still running.
    pub async fn drained(&self) {
        let started = Instant::now();
        let mut next_report = started;
        loop {
            let finished = self.finished.notified();
            tokio::pin!(finished);
            // Registered before the list is read, so work that ends between
            // the read and the wait still wakes this loop.
            finished.as_mut().enable();
            let running = self.running();
            if running.is_empty() {
                return;
            }
            let now = Instant::now();
            if now >= next_report {
                tracing::info!(
                    "🛑 Waiting for {} to finish ({}s so far): {}",
                    match running.len() {
                        1 => "1 job".to_string(),
                        count => format!("{count} jobs"),
                    },
                    started.elapsed().as_secs(),
                    running.join("; ")
                );
                next_report = now + REPORT_EVERY;
            }
            tokio::select! {
                _ = &mut finished => {}
                _ = tokio::time::sleep_until(next_report.into()) => {}
            }
        }
    }
}

/// Work a stop waits for. Dropping it says the work is done.
pub struct WorkGuard {
    shutdown: Arc<Shutdown>,
    id: u64,
}

impl WorkGuard {
    /// Whether the stop has begun.
    pub fn is_stopping(&self) -> bool {
        self.shutdown.is_stopping()
    }
}

impl Drop for WorkGuard {
    fn drop(&mut self) {
        self.shutdown.work.lock().unwrap().remove(&self.id);
        self.shutdown.finished.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::Shutdown;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn a_sleep_ends_when_the_stop_begins() {
        let shutdown = Shutdown::new();
        let sleeper = {
            let shutdown = shutdown.clone();
            tokio::spawn(async move { shutdown.sleep(Duration::from_secs(60)).await })
        };
        tokio::task::yield_now().await;
        let started = Instant::now();
        shutdown.begin();
        assert!(!sleeper.await.unwrap());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!shutdown.sleep(Duration::from_secs(60)).await);
    }

    #[tokio::test]
    async fn a_sleep_that_runs_out_says_to_go_on() {
        assert!(Shutdown::new().sleep(Duration::from_millis(1)).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_stop_waits_for_held_work_to_finish() {
        let shutdown = Shutdown::new();
        let work = shutdown.hold("a stack build");
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            // The work notices the stop and finishes its unit before it lets go.
            while !work.is_stopping() {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::thread::sleep(Duration::from_millis(50));
            finished_tx.send(()).unwrap();
            drop(work);
        });
        assert_eq!(shutdown.running(), vec!["a stack build".to_string()]);
        shutdown.begin();
        shutdown.drained().await;
        assert!(
            finished_rx.try_recv().is_ok(),
            "drained before the work let go"
        );
        assert!(shutdown.running().is_empty());
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn a_stop_with_nothing_held_drains_at_once() {
        let shutdown = Shutdown::new();
        shutdown.begin();
        tokio::time::timeout(Duration::from_secs(1), shutdown.drained())
            .await
            .unwrap();
    }
}
