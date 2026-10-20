//! The import into one database: a preview, or a run that writes.
//!
//! Each database keeps one import record, which its page polls about once a
//! second: the progress of the newest import or import preview.
//!
//! - A run that writes queues on [`CatalogJobs`], one runner per database.
//!   It holds the database's import lock while it reads headers and writes
//!   its one transaction. Stop rolls the transaction back.
//! - A preview runs at once, as a person's preview
//!   ([`crate::server::previews`]). A newer preview, or a run that writes,
//!   replaces it. A preview never replaces a run that writes: it is refused
//!   while one runs.
//!
//! Each run claims the record ([`Claim`]), so progress a replaced run
//! writes afterwards is dropped.
//!
//! Import stages: `scanning` (header reads, parallel), then `importing` (one
//! SQL transaction on a connection of its own, so the shared request
//! connection never waits behind a long import), then `complete` or
//! `error`.
//!
//! [`CatalogJobs`]: crate::server::job_line::catalog::CatalogJobs

use crate::commands::import::{ImportOptions, ImportOutcome};
use crate::concurrency::Stop;
use crate::server::database_context::DatabaseContext;
use crate::server::job_line::catalog::{CatalogJob, CatalogWork, Step};
use crate::server::state::AppState;
use serde::Serialize;
use std::sync::{Arc, RwLock};
use std::time::Instant;

/// Progress of the database's newest import or import preview.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportJobProgress {
    pub running: bool,
    /// Whether the run is a preview, which writes nothing.
    #[serde(default)]
    pub preview: bool,
    /// `scanning`, `importing`, `complete`, or `error`.
    pub stage: String,
    /// Directories being imported (for display).
    pub image_dirs: Vec<String>,
    /// Header-scan progress.
    pub total_files: usize,
    pub scanned_files: usize,
    /// Set once the import transaction finishes.
    pub outcome: Option<ImportOutcome>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
    /// `manual` for the Import button and API, `automatic` for a scheduled
    /// or on-open run.
    #[serde(default)]
    pub trigger: String,
    /// Files the automatic prefilter dropped as already cataloged, without
    /// reading their headers. Counted into the outcome's `skipped_existing`
    /// too; kept apart so the UI can say how cheap the run was.
    #[serde(default)]
    pub prefiltered: usize,
}

#[derive(Debug, Default)]
pub struct ImportJobStore {
    pub progress: ImportJobProgress,
    /// The number of the newest claim.
    claim: u64,
}

pub type SharedImportJob = Arc<RwLock<ImportJobStore>>;

/// A run's hold on the import record. Progress written under a claim that a
/// newer run replaced is dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claim(u64);

/// Claim the record for a run. `None` while a run that writes holds it; a
/// preview under way gives way to the new run.
pub fn try_begin(
    store: &RwLock<ImportJobStore>,
    image_dirs: Vec<String>,
    preview: bool,
) -> Option<Claim> {
    claim(&mut store.write().unwrap(), image_dirs, preview)
}

/// Claim the record for a preview and call `start` in one step, under the
/// record's lock. Two previews asked for at once then claim the record and
/// start their runs in the same order, so the newest one runs.
pub fn try_begin_preview<R>(
    store: &RwLock<ImportJobStore>,
    image_dirs: Vec<String>,
    start: impl FnOnce() -> R,
) -> Option<(Claim, R)> {
    let mut s = store.write().unwrap();
    let claim = claim(&mut s, image_dirs, true)?;
    Some((claim, start()))
}

fn claim(s: &mut ImportJobStore, image_dirs: Vec<String>, preview: bool) -> Option<Claim> {
    if s.progress.running && !s.progress.preview {
        return None;
    }
    s.claim += 1;
    s.progress = ImportJobProgress {
        running: true,
        preview,
        stage: "scanning".to_string(),
        image_dirs,
        started_at: Some(chrono::Utc::now().timestamp()),
        ..Default::default()
    };
    Some(Claim(s.claim))
}

/// Change the progress, unless a newer run replaced the claim.
fn update(
    store: &RwLock<ImportJobStore>,
    claim: Claim,
    change: impl FnOnce(&mut ImportJobProgress),
) {
    let mut s = store.write().unwrap();
    if s.claim == claim.0 {
        change(&mut s.progress);
    }
}

/// Whether no newer run replaced the claim.
pub fn is_current(store: &RwLock<ImportJobStore>, claim: Claim) -> bool {
    store.read().unwrap().claim == claim.0
}

pub fn set_trigger(store: &RwLock<ImportJobStore>, claim: Claim, trigger: &str) {
    update(store, claim, |progress| {
        progress.trigger = trigger.to_string()
    });
}

pub fn set_prefiltered(store: &RwLock<ImportJobStore>, claim: Claim, prefiltered: usize) {
    update(store, claim, |progress| progress.prefiltered = prefiltered);
}

pub fn set_stage(store: &RwLock<ImportJobStore>, claim: Claim, stage: &str) {
    update(store, claim, |progress| progress.stage = stage.to_string());
}

pub fn set_scan_totals(store: &RwLock<ImportJobStore>, claim: Claim, total: usize, scanned: usize) {
    update(store, claim, |progress| {
        progress.total_files = total;
        progress.scanned_files = scanned;
    });
}

/// One more header read.
pub fn count_scanned(store: &RwLock<ImportJobStore>, claim: Claim) {
    update(store, claim, |progress| progress.scanned_files += 1);
}

/// Publish the completed import and release the record. Optional quality
/// work is queued separately after this returns.
pub fn complete_import(store: &RwLock<ImportJobStore>, claim: Claim, outcome: ImportOutcome) {
    update(store, claim, |progress| {
        progress.outcome = Some(outcome);
        progress.running = false;
        progress.stage = "complete".to_string();
        progress.finished_at = Some(chrono::Utc::now().timestamp());
    });
}

/// Finalize the run. `error = None` marks success.
pub fn finish(store: &RwLock<ImportJobStore>, claim: Claim, error: Option<String>) {
    update(store, claim, |progress| {
        progress.running = false;
        progress.stage = if error.is_some() {
            "error".to_string()
        } else {
            "complete".to_string()
        };
        progress.error = error;
        progress.finished_at = Some(chrono::Utc::now().timestamp());
    });
}

pub fn progress_snapshot(store: &RwLock<ImportJobStore>) -> ImportJobProgress {
    store.read().unwrap().progress.clone()
}

/// A run that writes, until its runner takes it.
pub(crate) struct ImportRun {
    pub ctx: Arc<DatabaseContext>,
    pub dirs: Vec<String>,
    pub options: ImportOptions,
    /// Queue a quality scan of the targets it touched.
    pub backfill: bool,
    pub fill_metadata: bool,
    pub claim: Claim,
    /// A scheduled or on-open run, not a person's.
    pub automatic: bool,
}

/// Imports that write, one runner per database.
pub(crate) struct Imports;

#[derive(Default)]
pub(crate) struct Queue {
    pub(crate) pending: Option<ImportRun>,
    /// The record and claim of the run under way, so a failed runner ends
    /// it.
    current: Option<(SharedImportJob, Claim)>,
}

impl CatalogWork for Imports {
    type Queue = Queue;
    type Status = ();
    type Item = ImportRun;
    const RUNNER: &'static str = "import";
    /// An import writing its transaction finishes before the server stops;
    /// one still reading headers stops at the next file.
    const SHUTDOWN_WAITS_FOR: Option<&'static str> = Some("import");

    fn next(_: &AppState, _: &str, queue: &mut Queue, _: Instant) -> Step<ImportRun> {
        match queue.pending.take() {
            Some(run) => {
                queue.current = Some((Arc::clone(&run.ctx.import_job), run.claim));
                Step::Run(run)
            }
            None => {
                queue.current = None;
                Step::Exit
            }
        }
    }

    fn run(state: &Arc<AppState>, _: &str, _: &CatalogJob<Self>, run: ImportRun, stop: &Stop) {
        crate::server::handlers::run_import(state, run, stop);
    }

    fn has_work(queue: &Queue) -> bool {
        queue.pending.is_some()
    }

    fn runner_failed(queue: &mut Queue, _: &mut ()) {
        let failed = queue.current.take().or_else(|| {
            queue
                .pending
                .take()
                .map(|run| (Arc::clone(&run.ctx.import_job), run.claim))
        });
        if let Some((store, claim)) = failed {
            finish(
                &store,
                claim,
                Some("The import stopped unexpectedly.".into()),
            );
        }
    }
}

/// Stop the database's import: a preview, or a run that writes, which
/// rolls back. False when none runs.
pub fn stop(state: &AppState, ctx: &DatabaseContext) -> bool {
    let progress = progress_snapshot(&ctx.import_job);
    if !progress.running {
        return false;
    }
    if progress.preview {
        return state
            .previews
            .stop(crate::server::previews::PreviewKind::Import, &ctx.id);
    }
    match state.catalog_jobs.imports.existing(&ctx.id) {
        Some(job) => {
            job.stop(|queue| {
                // A run its runner has not taken yet never starts.
                if let Some(run) = queue.pending.take() {
                    finish(
                        &run.ctx.import_job,
                        run.claim,
                        Some("The import stopped before it started.".into()),
                    );
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
    fn singleton_guard() {
        let store = RwLock::new(ImportJobStore::default());
        let claim = try_begin(&store, vec!["/a".into()], false).unwrap();
        assert!(
            try_begin(&store, vec!["/b".into()], false).is_none(),
            "second job refused"
        );
        assert!(
            try_begin(&store, vec!["/b".into()], true).is_none(),
            "and a preview too"
        );
        finish(&store, claim, None);
        assert!(!progress_snapshot(&store).running);
        assert_eq!(progress_snapshot(&store).stage, "complete");
        assert!(
            try_begin(&store, vec!["/c".into()], false).is_some(),
            "reusable after finish"
        );
    }

    #[test]
    fn error_finish_reports_stage() {
        let store = RwLock::new(ImportJobStore::default());
        let claim = try_begin(&store, vec![], false).unwrap();
        finish(&store, claim, Some("boom".into()));
        let p = progress_snapshot(&store);
        assert_eq!(p.stage, "error");
        assert_eq!(p.error.as_deref(), Some("boom"));
        assert!(p.finished_at.is_some());
    }

    #[test]
    fn complete_import_publishes_outcome_and_releases_singleton() {
        let store = RwLock::new(ImportJobStore::default());
        let claim = try_begin(&store, vec!["/images".into()], false).unwrap();

        complete_import(&store, claim, ImportOutcome::default());
        let imported = progress_snapshot(&store);
        assert!(!imported.running, "header import is already complete");
        assert_eq!(imported.stage, "complete");
        assert!(try_begin(&store, vec!["/more".into()], false).is_some());
    }

    #[test]
    fn begin_resets_previous_run() {
        let store = RwLock::new(ImportJobStore::default());
        let claim = try_begin(&store, vec![], false).unwrap();
        set_scan_totals(&store, claim, 10, 10);
        finish(&store, claim, Some("boom".into()));
        assert!(try_begin(&store, vec![], false).is_some());
        let p = progress_snapshot(&store);
        assert_eq!(p.total_files, 0);
        assert!(p.error.is_none());
        assert_eq!(p.stage, "scanning");
    }

    #[test]
    fn a_new_run_replaces_a_preview_and_the_preview_writes_nothing_after() {
        let store = RwLock::new(ImportJobStore::default());
        let preview = try_begin(&store, vec!["/a".into()], true).unwrap();
        set_scan_totals(&store, preview, 10, 2);
        // A newer preview replaces it, and so would a run that writes.
        let newer = try_begin(&store, vec!["/a".into()], true).unwrap();
        assert!(!is_current(&store, preview) && is_current(&store, newer));
        count_scanned(&store, preview);
        complete_import(&store, preview, ImportOutcome::default());
        let progress = progress_snapshot(&store);
        assert!(progress.running && progress.preview);
        assert_eq!(
            progress.scanned_files, 0,
            "the replaced preview's count is dropped"
        );
        let import = try_begin(&store, vec!["/a".into()], false).unwrap();
        finish(&store, newer, Some("stopped".into()));
        let progress = progress_snapshot(&store);
        assert!(progress.running && !progress.preview && progress.error.is_none());
        finish(&store, import, None);
    }
}
