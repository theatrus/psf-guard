//! Singleton per-DB background FITS import job, plus the post-import quality
//! backfill chain.
//!
//! Modeled on `spatial_scan`: one job per database at a time, a serializable
//! progress snapshot the frontend polls (~1s), and `try_begin` /
//! `finish` guards so a panic can never wedge the singleton.
//!
//! Stages: `scanning` (header reads, parallel) → `importing` (one SQL
//! transaction via a **dedicated** connection, so the shared request
//! connection never blocks behind a long import) → `backfill` (chained
//! quality scans over the created targets, reusing the existing singleton
//! scan) → `complete` / `error`.

use crate::commands::import::ImportOutcome;
use serde::Serialize;
use std::sync::{Arc, RwLock};

/// Progress of the (singleton per-DB) import job.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportJobProgress {
    pub running: bool,
    /// `scanning`, `importing`, `backfill`, `complete`, or `error`.
    pub stage: String,
    /// Directories being imported (for display).
    pub image_dirs: Vec<String>,
    /// Header-scan progress.
    pub total_files: usize,
    pub scanned_files: usize,
    /// Set once the import transaction finishes.
    pub outcome: Option<ImportOutcome>,
    /// Post-import quality backfill progress (live imports only).
    pub backfill_total: usize,
    pub backfill_done: usize,
    pub backfill_current_target: Option<i32>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct ImportJobStore {
    pub progress: ImportJobProgress,
}

pub type SharedImportJob = Arc<RwLock<ImportJobStore>>;

/// Claim the singleton. Returns false when a job is already running.
pub fn try_begin(store: &RwLock<ImportJobStore>, image_dirs: Vec<String>) -> bool {
    let mut s = store.write().unwrap();
    if s.progress.running {
        return false;
    }
    s.progress = ImportJobProgress {
        running: true,
        stage: "scanning".to_string(),
        image_dirs,
        started_at: Some(chrono::Utc::now().timestamp()),
        ..Default::default()
    };
    true
}

pub fn set_stage(store: &RwLock<ImportJobStore>, stage: &str) {
    let mut s = store.write().unwrap();
    s.progress.stage = stage.to_string();
}

pub fn set_scan_totals(store: &RwLock<ImportJobStore>, total: usize, scanned: usize) {
    let mut s = store.write().unwrap();
    s.progress.total_files = total;
    s.progress.scanned_files = scanned;
}

pub fn set_outcome(store: &RwLock<ImportJobStore>, outcome: ImportOutcome) {
    let mut s = store.write().unwrap();
    s.progress.outcome = Some(outcome);
}

pub fn begin_backfill(store: &RwLock<ImportJobStore>, total: usize) {
    let mut s = store.write().unwrap();
    s.progress.stage = "backfill".to_string();
    s.progress.backfill_total = total;
    s.progress.backfill_done = 0;
}

pub fn backfill_target(store: &RwLock<ImportJobStore>, target_id: i32) {
    let mut s = store.write().unwrap();
    s.progress.backfill_current_target = Some(target_id);
}

pub fn backfill_target_done(store: &RwLock<ImportJobStore>) {
    let mut s = store.write().unwrap();
    s.progress.backfill_done += 1;
    s.progress.backfill_current_target = None;
}

/// Finalize the job. `error = None` marks success.
pub fn finish(store: &RwLock<ImportJobStore>, error: Option<String>) {
    let mut s = store.write().unwrap();
    s.progress.running = false;
    s.progress.stage = if error.is_some() {
        "error".to_string()
    } else {
        "complete".to_string()
    };
    s.progress.error = error;
    s.progress.finished_at = Some(chrono::Utc::now().timestamp());
    s.progress.backfill_current_target = None;
}

pub fn progress_snapshot(store: &RwLock<ImportJobStore>) -> ImportJobProgress {
    store.read().unwrap().progress.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singleton_guard() {
        let store = RwLock::new(ImportJobStore::default());
        assert!(try_begin(&store, vec!["/a".into()]));
        assert!(!try_begin(&store, vec!["/b".into()]), "second job refused");
        finish(&store, None);
        assert!(!progress_snapshot(&store).running);
        assert_eq!(progress_snapshot(&store).stage, "complete");
        assert!(
            try_begin(&store, vec!["/c".into()]),
            "reusable after finish"
        );
    }

    #[test]
    fn error_finish_reports_stage() {
        let store = RwLock::new(ImportJobStore::default());
        assert!(try_begin(&store, vec![]));
        finish(&store, Some("boom".into()));
        let p = progress_snapshot(&store);
        assert_eq!(p.stage, "error");
        assert_eq!(p.error.as_deref(), Some("boom"));
        assert!(p.finished_at.is_some());
    }

    #[test]
    fn begin_resets_previous_run() {
        let store = RwLock::new(ImportJobStore::default());
        assert!(try_begin(&store, vec![]));
        set_scan_totals(&store, 10, 10);
        finish(&store, Some("boom".into()));
        assert!(try_begin(&store, vec![]));
        let p = progress_snapshot(&store);
        assert_eq!(p.total_files, 0);
        assert!(p.error.is_none());
        assert_eq!(p.stage, "scanning");
    }
}
