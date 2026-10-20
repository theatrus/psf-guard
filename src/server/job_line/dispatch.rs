//! A line's dispatchers: one async task per lane, started when work first
//! joins the line. Each dispatcher runs the next entry on this server when
//! the lane is free, and settles entries that were stopped while queued.

use std::sync::Arc;

use super::{Dispatch, JobLine, Lane, Worker};
use crate::concurrency::Priority;
use crate::server::state::AppState;

/// How an entry runs, decided when it starts: a person can adopt
/// automatic work while it is queued.
pub struct Turn {
    pub priority: Priority,
    /// Whether background work pauses while the entry runs, and shows the
    /// entry as the reason. Set for a person's work on this server's cores.
    pub pauses_background_work: bool,
}

/// How the dispatchers of a line on this server run its entries.
pub struct DispatchRules {
    /// The line's name in the log: `stack`, `WBPP`.
    pub name: &'static str,
    /// The line, read from the server state.
    pub line: fn(&AppState) -> &JobLine,
    /// The lanes that get a dispatcher.
    pub lanes: &'static [Lane],
    /// How the entry with the given id runs.
    pub turn: fn(&AppState, &str) -> Turn,
    /// Whether an automatic entry may start now. While this returns false,
    /// such as while a person's work runs elsewhere on the server, the
    /// dispatcher passes over automatic entries.
    pub automatic_may_start: fn(&AppState) -> bool,
}

impl JobLine {
    /// Start the dispatcher of each lane in `rules` that has none running.
    pub fn ensure_dispatching(&self, state: &Arc<AppState>, rules: &'static DispatchRules) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("No runtime to dispatch the {} line on", rules.name);
            return;
        };
        let mut dispatchers = self
            .dispatchers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for &lane in rules.lanes {
            if dispatchers
                .get(&lane)
                .is_some_and(|dispatcher| !dispatcher.is_finished())
            {
                continue;
            }
            dispatchers.insert(
                lane,
                runtime.spawn(dispatch(Arc::clone(state), rules, lane)),
            );
        }
    }
}

/// A lane's dispatcher: runs each entry in order on this server, and settles
/// each entry that was stopped while queued.
async fn dispatch(state: Arc<AppState>, rules: &'static DispatchRules, lane: Lane) {
    loop {
        let next = (rules.line)(&state)
            .next_when(lane, || (rules.automatic_may_start)(&state))
            .await;
        let state = Arc::clone(&state);
        match next {
            Dispatch::Run {
                id,
                stop,
                mut task,
                lease,
            } => {
                if let Some(work) = task.take_future(&state, &stop) {
                    // Awaited on the runtime, which drops it when the
                    // server stops, as it drops other async work.
                    tokio::spawn(async move {
                        work.await;
                        drop(lease);
                    });
                    continue;
                }
                let turn = (rules.turn)(&state, &id);
                let person = turn
                    .pauses_background_work
                    .then(|| state.begin_named_interactive_job(task.what()));
                tokio::task::spawn_blocking(move || {
                    let worker = Worker::this_server(&state, &stop, turn.priority);
                    task.run(&state, &worker);
                    drop(person);
                    drop(lease);
                });
            }
            Dispatch::Stopped { task, .. } => {
                tokio::task::spawn_blocking(move || task.stopped(&state));
            }
        }
    }
}
