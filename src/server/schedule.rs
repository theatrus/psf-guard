//! Work the server does on a timer: automatic imports, Target Scheduler
//! syncs, backups, the job journal, the refresh scheduler and the other
//! periodic checks.
//!
//! [`every`] runs each on its timer until the server stops. Each run is a
//! task of its own: a run that panics is logged, and the next runs on time,
//! where a panic used to end the loop for good. The timer wakes as soon as
//! the server stops, and holds the server state weakly, so a state the
//! desktop app replaced is not kept alive by its timers.
//!
//! The work a run starts belongs on the job machinery: a run decides what
//! is due and asks a catalog job or a line for it.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use crate::server::state::AppState;

/// When a timed task runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Every {
    /// The wait before the first run.
    pub first: Duration,
    /// The wait between the end of one run and the start of the next.
    pub then: Duration,
}

impl Every {
    /// Every `period`, the first run one period after the start.
    pub const fn period(period: Duration) -> Self {
        Self {
            first: period,
            then: period,
        }
    }

    /// The first run after `first`, then every `then`.
    pub const fn after(first: Duration, then: Duration) -> Self {
        Self { first, then }
    }
}

/// Run `run` on the `when` timer until the server stops or its state is
/// dropped. `name` identifies the timer in the log, as "automatic import".
pub fn every<F, Fut>(
    state: &Arc<AppState>,
    name: &'static str,
    when: Every,
    mut run: F,
) -> tokio::task::JoinHandle<()>
where
    F: FnMut(Arc<AppState>) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let shutdown = Arc::clone(&state.shutdown);
    let state = Arc::downgrade(state);
    tokio::spawn(async move {
        let mut wait = when.first;
        loop {
            if !shutdown.sleep(wait).await {
                return;
            }
            wait = when.then;
            let Some(live) = state.upgrade() else {
                return;
            };
            if let Err(error) = tokio::spawn(run(live)).await
                && error.is_panic()
            {
                tracing::error!(
                    "The {name} timer's run panicked; it runs again in {:?}",
                    when.then
                );
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn state() -> Arc<AppState> {
        Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ))
    }

    #[tokio::test]
    async fn a_run_that_panics_does_not_end_the_timer() {
        let state = state();
        let runs = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&runs);
        let timer = every(
            &state,
            "test",
            Every::period(Duration::from_millis(5)),
            move |_| {
                let run = counted.fetch_add(1, Ordering::SeqCst);
                async move {
                    if run == 0 {
                        panic!("the first run fails");
                    }
                }
            },
        );
        while runs.load(Ordering::SeqCst) < 3 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(!timer.is_finished());
        timer.abort();
    }

    #[tokio::test]
    async fn the_timer_ends_when_the_server_stops_or_its_state_is_gone() {
        let state = state();
        let timer = every(
            &state,
            "test",
            Every::period(Duration::from_secs(3600)),
            |_| async {},
        );
        state.shutdown.begin();
        tokio::time::timeout(Duration::from_secs(1), timer)
            .await
            .expect("a server stop wakes the timer")
            .unwrap();

        let state = self::state();
        let timer = every(
            &state,
            "test",
            Every::period(Duration::from_millis(5)),
            |_| async {},
        );
        drop(state);
        tokio::time::timeout(Duration::from_secs(1), timer)
            .await
            .expect("the timer holds the state weakly")
            .unwrap();
    }
}
