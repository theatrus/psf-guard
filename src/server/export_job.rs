//! Singleton per-DB background export job.
//!
//! Modeled on `import_job`: one job per database at a time, a serializable
//! progress snapshot the frontend polls (~1s), and `try_begin` / `finish`
//! guards so a panic can never wedge the singleton.
//!
//! Export stages: `planning` (catalog walk + calibration join) → `placing`
//! (reflink-or-copy of every planned file) → `scripts` (WBPP runner) →
//! `complete` / `error`.

use crate::commands::export::ExportSummary;
use serde::Serialize;
use std::sync::{Arc, RwLock};

/// Progress of the (singleton per-DB) export job.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExportJobProgress {
    pub running: bool,
    /// `planning`, `placing`, `scripts`, `complete`, or `error`.
    pub stage: String,
    /// Where this export lands, for display. Set at begin.
    pub destination: String,
    /// What was asked for, for display ("project Bubble", "target M 74 Ha").
    pub scope: String,
    /// Placement progress once the plan is known.
    pub total_files: usize,
    pub placed_files: usize,
    /// Set once placement finishes.
    pub outcome: Option<ExportSummary>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct ExportJobStore {
    pub progress: ExportJobProgress,
}

pub type SharedExportJob = Arc<RwLock<ExportJobStore>>;

/// Claim the singleton. Returns false when a job is already running.
pub fn try_begin(store: &RwLock<ExportJobStore>, destination: String, scope: String) -> bool {
    let mut s = store.write().unwrap();
    if s.progress.running {
        return false;
    }
    s.progress = ExportJobProgress {
        running: true,
        stage: "planning".to_string(),
        destination,
        scope,
        started_at: Some(chrono::Utc::now().timestamp()),
        ..Default::default()
    };
    true
}

pub fn set_stage(store: &RwLock<ExportJobStore>, stage: &str) {
    let mut s = store.write().unwrap();
    s.progress.stage = stage.to_string();
}

pub fn set_placement_totals(store: &RwLock<ExportJobStore>, total: usize, placed: usize) {
    let mut s = store.write().unwrap();
    s.progress.total_files = total;
    s.progress.placed_files = placed;
}

/// Publish the completed export and release the per-database singleton.
pub fn complete_export(store: &RwLock<ExportJobStore>, outcome: ExportSummary) {
    let mut s = store.write().unwrap();
    s.progress.outcome = Some(outcome);
    s.progress.running = false;
    s.progress.stage = "complete".to_string();
    s.progress.finished_at = Some(chrono::Utc::now().timestamp());
}

/// Finalize the job. `error = None` marks success.
pub fn finish(store: &RwLock<ExportJobStore>, error: Option<String>) {
    let mut s = store.write().unwrap();
    s.progress.running = false;
    s.progress.stage = if error.is_some() {
        "error".to_string()
    } else {
        "complete".to_string()
    };
    s.progress.error = error;
    s.progress.finished_at = Some(chrono::Utc::now().timestamp());
}

pub fn progress_snapshot(store: &RwLock<ExportJobStore>) -> ExportJobProgress {
    store.read().unwrap().progress.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_is_a_singleton_and_resets_prior_state() {
        let store = RwLock::new(ExportJobStore::default());
        assert!(try_begin(&store, "/exports".into(), "project A".into()));
        assert!(!try_begin(&store, "/exports".into(), "project B".into()));
        finish(&store, Some("boom".into()));
        assert_eq!(progress_snapshot(&store).stage, "error");
        assert!(try_begin(&store, "/exports".into(), "project B".into()));
        let progress = progress_snapshot(&store);
        assert_eq!(progress.scope, "project B");
        assert!(progress.error.is_none());
        assert!(progress.outcome.is_none());
    }

    #[test]
    fn completion_publishes_the_summary() {
        let store = RwLock::new(ExportJobStore::default());
        assert!(try_begin(&store, "/exports".into(), "target T".into()));
        set_stage(&store, "placing");
        set_placement_totals(&store, 10, 4);
        complete_export(&store, ExportSummary::default());
        let progress = progress_snapshot(&store);
        assert!(!progress.running);
        assert_eq!(progress.stage, "complete");
        assert_eq!(progress.total_files, 10);
        assert!(progress.outcome.is_some());
    }
}
