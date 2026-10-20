//! The export into a database's export folder: one at a time per database,
//! on [`CatalogJobs`], with a progress record the page polls about once a
//! second. Stop ends it before the next file; the files placed stay, and a
//! later export skips them.
//!
//! [`CatalogJobs`]: crate::server::job_line::catalog::CatalogJobs
//!
//! Export stages: `planning` (catalog walk + calibration join) → `placing`
//! (reflink-or-copy of every planned file) → `scripts` (WBPP runner) →
//! `complete` / `error`.

use crate::commands::export::ExportSummary;
use crate::concurrency::Stop;
use crate::server::database_context::DatabaseContext;
use crate::server::job_line::catalog::{CatalogJob, CatalogWork, Step};
use crate::server::state::AppState;
use serde::Serialize;
use std::sync::{Arc, RwLock};
use std::time::Instant;

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

/// An export to run, until its runner takes it.
pub(crate) struct ExportRun {
    pub ctx: Arc<DatabaseContext>,
    pub dest: std::path::PathBuf,
    pub options: crate::commands::export::ExportOptions,
    pub placement: crate::commands::export::Placement,
    pub files: crate::commands::export::wbpp::WbppFiles,
    pub wbpp_options: crate::commands::export::wbpp::WbppOptions,
}

/// Exports into the server's export folders, one runner per database.
pub(crate) struct Exports;

#[derive(Default)]
pub(crate) struct Queue {
    pub(crate) pending: Option<ExportRun>,
    /// The record of the export under way, so a failed runner ends it.
    current: Option<SharedExportJob>,
}

impl CatalogWork for Exports {
    type Queue = Queue;
    type Status = ();
    type Item = ExportRun;
    const RUNNER: &'static str = "export";
    /// A stop ends the export before its next file.
    const SHUTDOWN_WAITS_FOR: Option<&'static str> = Some("export");

    fn next(_: &AppState, _: &str, queue: &mut Queue, _: Instant) -> Step<ExportRun> {
        match queue.pending.take() {
            Some(run) => {
                queue.current = Some(Arc::clone(&run.ctx.export_job));
                Step::Run(run)
            }
            None => {
                queue.current = None;
                Step::Exit
            }
        }
    }

    fn run(state: &Arc<AppState>, _: &str, _: &CatalogJob<Self>, run: ExportRun, stop: &Stop) {
        crate::server::handlers::run_server_export(state, run, stop);
    }

    fn has_work(queue: &Queue) -> bool {
        queue.pending.is_some()
    }

    fn runner_failed(queue: &mut Queue, _: &mut ()) {
        let failed = queue.current.take().or_else(|| {
            queue
                .pending
                .take()
                .map(|run| Arc::clone(&run.ctx.export_job))
        });
        if let Some(store) = failed {
            finish(&store, Some("The export stopped unexpectedly.".into()));
        }
    }
}

/// Stop the database's export before its next file. False when none runs.
pub fn stop(state: &AppState, ctx: &DatabaseContext) -> bool {
    if !progress_snapshot(&ctx.export_job).running {
        return false;
    }
    match state.catalog_jobs.exports.existing(&ctx.id) {
        Some(job) => {
            job.stop(|queue| {
                // An export its runner has not taken yet never starts.
                if let Some(run) = queue.pending.take() {
                    finish(&run.ctx.export_job, Some("Stopped.".into()));
                }
            });
            true
        }
        None => false,
    }
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
