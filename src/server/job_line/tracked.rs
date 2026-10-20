//! The jobs a line tracks from the moment they are requested until they
//! end: each job's stop flag and, when the journal keeps the job, its
//! journal record.
//!
//! A job is tracked longer than its entry is in the line. A build is
//! tracked while it is prepared, before it has an entry, and while its
//! results are published, after its entry has left. The journal record is
//! stored as JSON, as entry specs are, so the line stays independent of
//! what each journal records.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use serde::{de::DeserializeOwned, Serialize};

use crate::concurrency::Stop;

#[derive(Default)]
pub(super) struct Tracked {
    jobs: Mutex<HashMap<String, TrackedJob>>,
}

struct TrackedJob {
    stop: Stop,
    journal: Option<serde_json::Value>,
}

impl Tracked {
    fn jobs(&self) -> MutexGuard<'_, HashMap<String, TrackedJob>> {
        self.jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn track(&self, id: &str) -> Stop {
        let stop = Stop::new();
        self.jobs().insert(
            id.to_string(),
            TrackedJob {
                stop: stop.clone(),
                journal: None,
            },
        );
        stop
    }

    pub(super) fn stop_flag(&self, id: &str) -> Option<Stop> {
        self.jobs().get(id).map(|job| job.stop.clone())
    }

    /// Replace the stop flag of a tracked job, keeping its journal record.
    pub(super) fn restore_stop(&self, id: &str, stop: Stop) {
        if let Some(job) = self.jobs().get_mut(id) {
            job.stop = stop;
        }
    }

    pub(super) fn forget(&self, id: &str) {
        self.jobs().remove(id);
    }

    pub(super) fn forget_if_current(&self, id: &str, stop: &Stop) -> bool {
        let mut jobs = self.jobs();
        if jobs.get(id).is_some_and(|job| job.stop.same(stop)) {
            jobs.remove(id);
            true
        } else {
            false
        }
    }

    pub(super) fn ids(&self) -> Vec<String> {
        self.jobs().keys().cloned().collect()
    }

    pub(super) fn clear_journal(&self, id: &str) {
        if let Some(job) = self.jobs().get_mut(id) {
            job.journal = None;
        }
    }

    pub(super) fn set_journal<T: Serialize>(&self, id: &str, record: &T) {
        let value = match serde_json::to_value(record) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(id, "A journal record could not be stored: {error}");
                return;
            }
        };
        if let Some(job) = self.jobs().get_mut(id) {
            job.journal = Some(value);
        }
    }

    pub(super) fn update_journal<T: Serialize + DeserializeOwned>(
        &self,
        id: &str,
        edit: impl FnOnce(&mut T),
    ) {
        let mut jobs = self.jobs();
        let Some(value) = jobs.get_mut(id).and_then(|job| job.journal.as_mut()) else {
            return;
        };
        let Ok(mut record) = serde_json::from_value::<T>(value.clone()) else {
            return;
        };
        edit(&mut record);
        if let Ok(updated) = serde_json::to_value(&record) {
            *value = updated;
        }
    }

    pub(super) fn journal_of<T: DeserializeOwned>(&self, id: &str) -> Option<T> {
        let value = self.jobs().get(id)?.journal.clone()?;
        serde_json::from_value(value).ok()
    }

    /// Stop tracking the job `id` and return its journal record, in one
    /// step: two callers cannot both take the same job.
    pub(super) fn take_journal<T: DeserializeOwned>(&self, id: &str) -> Option<T> {
        let job = self.jobs().remove(id)?;
        serde_json::from_value(job.journal?).ok()
    }

    pub(super) fn journals<T: DeserializeOwned>(&self) -> Vec<(String, T)> {
        self.jobs()
            .iter()
            .filter_map(|(id, job)| {
                let record = serde_json::from_value(job.journal.clone()?).ok()?;
                Some((id.clone(), record))
            })
            .collect()
    }
}
