//! Work a person drives in real time, where the newest request wins.
//!
//! A preview of an export, an import or a sync is cheap next to a stack
//! build, and a person waits for each one. Previews never wait in a line: a
//! request starts at once, as a person's work, and a newer request for the
//! same subject stops the one before it. The older run sees its stop and
//! ends early, and its result is dropped ([`LatestRun::is_latest`]).
//!
//! [`LatestRequests`] tracks the run of each subject, so Stop reaches it and
//! the jobs route lists it.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::concurrency::Stop;
use crate::server::state::{AppState, InteractiveJobGuard};

/// The runs of one kind of preview, by subject. Clones share the record.
pub struct LatestRequests<K>(Arc<Record<K>>);

impl<K> Clone for LatestRequests<K> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<K> Default for LatestRequests<K> {
    fn default() -> Self {
        Self(Arc::new(Record {
            runs: Mutex::new(HashMap::new()),
            next: AtomicU64::new(0),
        }))
    }
}

struct Record<K> {
    runs: Mutex<HashMap<K, Running>>,
    /// Numbers each run, so a run that ends after a newer one started
    /// leaves the newer one's record alone.
    next: AtomicU64,
}

struct Running {
    number: u64,
    stop: Stop,
    label: String,
    started_at: i64,
}

/// A run the jobs route lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningPreview<K> {
    pub key: K,
    /// What the run does, such as `Previewing sync into Rig`.
    pub label: String,
    /// When it started, in Unix seconds.
    pub started_at: i64,
}

impl<K: Clone + Eq + Hash> LatestRequests<K> {
    fn runs(&self) -> MutexGuard<'_, HashMap<K, Running>> {
        self.0
            .runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Start a run for `key` now, as a person's work named `label`. A run
    /// still under way for the same key is asked to stop.
    pub fn start(&self, state: &AppState, key: K, label: impl Into<String>) -> LatestRun<K> {
        let label = label.into();
        let person = state.begin_named_interactive_job(label.clone());
        self.start_run(key, label, Some(person))
    }

    /// [`Self::start`] for a request that no one on this server made, such
    /// as a sync another server pushes: background work keeps running.
    pub fn start_unattended(&self, key: K, label: impl Into<String>) -> LatestRun<K> {
        self.start_run(key, label.into(), None)
    }

    fn start_run(
        &self,
        key: K,
        label: String,
        person: Option<InteractiveJobGuard>,
    ) -> LatestRun<K> {
        let number = self.0.next.fetch_add(1, Ordering::Relaxed);
        let stop = Stop::new();
        let previous = self.runs().insert(
            key.clone(),
            Running {
                number,
                stop: stop.clone(),
                label: label.clone(),
                started_at: chrono::Utc::now().timestamp(),
            },
        );
        if let Some(previous) = previous {
            previous.stop.request();
        }
        LatestRun {
            requests: self.clone(),
            key,
            number,
            stop,
            _person: person,
        }
    }

    /// Ask the run for `key` to stop. False when none runs.
    pub fn stop(&self, key: &K) -> bool {
        match self.runs().get(key) {
            Some(running) => {
                running.stop.request();
                true
            }
            None => false,
        }
    }

    /// The runs under way, oldest first.
    pub fn running(&self) -> Vec<RunningPreview<K>> {
        let mut running: Vec<_> = self
            .runs()
            .iter()
            .map(|(key, running)| {
                (
                    running.number,
                    RunningPreview {
                        key: key.clone(),
                        label: running.label.clone(),
                        started_at: running.started_at,
                    },
                )
            })
            .collect();
        running.sort_by_key(|(number, _)| *number);
        running.into_iter().map(|(_, preview)| preview).collect()
    }
}

/// One run. It counts as a person's work until it is dropped, and dropping
/// it removes its record unless a newer run of the same key replaced it.
pub struct LatestRun<K: Clone + Eq + Hash> {
    requests: LatestRequests<K>,
    key: K,
    number: u64,
    stop: Stop,
    /// Held while a person waits for the run.
    _person: Option<InteractiveJobGuard>,
}

impl<K: Clone + Eq + Hash> LatestRun<K> {
    /// The run's stop: set by Stop, or when a newer request for the same
    /// key starts.
    pub fn stop(&self) -> &Stop {
        &self.stop
    }

    /// Whether no newer request for the same key started. A run that is no
    /// longer the latest drops its result.
    pub fn is_latest(&self) -> bool {
        self.requests
            .runs()
            .get(&self.key)
            .is_some_and(|running| running.number == self.number)
    }
}

impl<K: Clone + Eq + Hash> Drop for LatestRun<K> {
    fn drop(&mut self) {
        let mut runs = self.requests.runs();
        if runs
            .get(&self.key)
            .is_some_and(|running| running.number == self.number)
        {
            runs.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        AppState::new_for_test(rusqlite::Connection::open_in_memory().unwrap())
    }

    #[test]
    fn a_newer_request_stops_the_one_before_and_takes_its_place() {
        let state = state();
        let previews = LatestRequests::<String>::default();
        let first = previews.start(&state, "rig".into(), "Previewing sync into Rig");
        assert!(first.is_latest());
        assert!(
            state.interactive_job_active(),
            "a preview is a person's work"
        );

        let second = previews.start(&state, "rig".into(), "Previewing sync into Rig");
        assert!(first.stop().requested(), "the older run is asked to stop");
        assert!(!first.is_latest(), "and drops its result");
        assert!(second.is_latest() && !second.stop().requested());

        // The older run ending leaves the newer one listed.
        drop(first);
        assert_eq!(previews.running().len(), 1);
        drop(second);
        assert!(previews.running().is_empty());
        assert!(!state.interactive_job_active());
    }

    #[test]
    fn requests_for_other_subjects_run_side_by_side() {
        let state = state();
        let previews = LatestRequests::<String>::default();
        let rig = previews.start(&state, "rig".into(), "Previewing sync into Rig");
        let home = previews.start(&state, "home".into(), "Previewing sync into Home");
        assert!(rig.is_latest() && home.is_latest());
        assert!(!rig.stop().requested() && !home.stop().requested());
        let labels: Vec<_> = previews
            .running()
            .into_iter()
            .map(|run| run.label)
            .collect();
        assert_eq!(
            labels,
            ["Previewing sync into Rig", "Previewing sync into Home"]
        );
    }

    #[test]
    fn stop_reaches_the_running_preview() {
        let state = state();
        let previews = LatestRequests::<String>::default();
        assert!(!previews.stop(&"rig".into()));
        let run = previews.start(&state, "rig".into(), "Previewing sync into Rig");
        assert!(previews.stop(&"rig".into()));
        assert!(run.stop().requested());
        // Stopped, but still the latest request: the person sees it end.
        assert!(run.is_latest());
    }
}
