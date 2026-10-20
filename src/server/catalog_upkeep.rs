//! Upkeep of each catalog file that PSF Guard does on its own, one runner
//! per catalog on [`CatalogJobs`]:
//!
//! - **Query indexes.** PSF Guard's indexes on the catalog, built once at
//!   server start rather than whenever a connection opens.
//! - **Readout modes.** The readout-mode names of calibration frames
//!   imported before the catalog kept them, read from their headers.
//! - **Calibrated copies.** Calibrated and registered copies already on disk
//!   paired with their lights, after each directory refresh.
//!
//! Each works on its own connection with a short busy timeout, so a capture
//! program writing the catalog wins: the work stops for now and the next
//! start or refresh continues it. The server start, and each change to the
//! registered databases, asks for the indexes and readout modes; a
//! published directory tree asks for the pairing ([`tree_published`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Instant;

use rusqlite::Connection;
use serde::Serialize;
use tokio::sync::Mutex as TokioMutex;

use crate::concurrency::{LiveSetting, Stop};
use crate::directory_tree::DirectoryTree;
use crate::frame_derivatives::FileStamp;
use crate::server::database_context::{db_open_flags, INDEX_BUILD_BUSY_TIMEOUT};
use crate::server::job_line::catalog::{CatalogJob, CatalogJobs, CatalogWork, Step};
use crate::server::state::AppState;

/// One catalog's upkeep as a job with one runner per catalog.
pub(crate) struct CatalogUpkeep;

type Job = CatalogJob<CatalogUpkeep>;

/// What waits for the catalog's runner.
#[derive(Default)]
pub(crate) struct Queue {
    indexes: bool,
    readout_modes: bool,
    /// The newest published tree to pair copies in; a newer tree replaces
    /// one still waiting.
    pairing: Option<Pairing>,
    /// The files whose pairing was checked, with what they were then, so
    /// the next pass reads only what changed.
    checked: HashMap<PathBuf, FileStamp>,
    /// The server that last asked, so a published tree can ask for pairing.
    server: Weak<AppState>,
}

/// Copies to pair in a published directory tree.
pub(crate) struct Pairing {
    tree: Arc<DirectoryTree>,
    enabled: LiveSetting,
    import_lock: Arc<TokioMutex<()>>,
}

/// The piece of upkeep the runner takes next.
pub(crate) enum Item {
    Indexes,
    ReadoutModes,
    Pairing(Pairing),
}

/// What the runner does now, for the jobs route.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpkeepStage {
    #[default]
    Idle,
    BuildingIndexes,
    ReadingReadoutModes,
    PairingCopies,
}

/// Where a catalog's upkeep stands.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpkeepStatus {
    pub stage: UpkeepStage,
    /// Frames read and the frames to read, while readout modes are read.
    pub done: usize,
    pub total: usize,
}

impl CatalogWork for CatalogUpkeep {
    type Queue = Queue;
    type Status = UpkeepStatus;
    type Item = Item;
    const RUNNER: &'static str = "catalog-upkeep";
    /// Shutdown waits for the index being built, the readout-mode chunk
    /// being committed, or the pairing pass.
    const SHUTDOWN_WAITS_FOR: Option<&'static str> = Some("catalog upkeep");

    fn next(_: &AppState, _: &str, queue: &mut Queue, _: Instant) -> Step<Item> {
        if std::mem::take(&mut queue.indexes) {
            return Step::Run(Item::Indexes);
        }
        if std::mem::take(&mut queue.readout_modes) {
            return Step::Run(Item::ReadoutModes);
        }
        match queue.pairing.take() {
            Some(pairing) => Step::Run(Item::Pairing(pairing)),
            None => Step::Exit,
        }
    }

    fn run(state: &Arc<AppState>, path: &str, job: &Job, item: Item, stop: &Stop) {
        let keep_going = || !stop.requested() && !state.shutdown.is_stopping();
        match item {
            Item::Indexes => {
                job.update(|status| status.stage = UpkeepStage::BuildingIndexes);
                build_indexes(path);
            }
            Item::ReadoutModes => {
                job.update(|status| status.stage = UpkeepStage::ReadingReadoutModes);
                read_readout_modes(path, job, keep_going);
            }
            Item::Pairing(pairing) => {
                job.update(|status| status.stage = UpkeepStage::PairingCopies);
                // The pass reads and updates the record of checked files;
                // the queue holds it between passes.
                let mut checked = std::mem::take(&mut job.queue().checked);
                pair_copies(path, &pairing, &mut checked);
                job.queue().checked = checked;
            }
        }
        job.update(|status| *status = UpkeepStatus::default());
    }

    fn has_work(queue: &Queue) -> bool {
        queue.indexes || queue.readout_modes || queue.pairing.is_some()
    }

    fn runner_failed(_: &mut Queue, status: &mut UpkeepStatus) {
        *status = UpkeepStatus::default();
    }
}

static JOBS: CatalogJobs<CatalogUpkeep> = CatalogJobs::new();

/// Build the catalog's query indexes and read the readout modes it lacks,
/// on its runner. The server start asks for every catalog, and each change
/// to the registered databases for the catalog it opened.
pub(crate) fn start(state: &Arc<AppState>, path: &str) {
    JOBS.request(state, path, |queue| {
        queue.server = Arc::downgrade(state);
        queue.indexes = true;
        queue.readout_modes = true;
    });
}

/// A directory refresh of the catalog at `path` published `tree`: pair the
/// calibrated copies in it while `enabled` is on, unless an import holds
/// `import_lock`. Only a catalog whose upkeep a server started is paired;
/// the server start starts every catalog's.
pub(crate) fn tree_published(
    path: &str,
    tree: Arc<DirectoryTree>,
    enabled: LiveSetting,
    import_lock: Arc<TokioMutex<()>>,
) {
    if !enabled.is_on() {
        return;
    }
    let Some(state) = JOBS
        .existing(path)
        .and_then(|job| job.queue().server.upgrade())
    else {
        return;
    };
    JOBS.request(&state, path, |queue| {
        queue.pairing = Some(Pairing {
            tree,
            enabled,
            import_lock,
        });
    });
}

/// Where the upkeep of the catalog at `path` stands.
pub(crate) fn status(path: &str) -> UpkeepStatus {
    JOBS.existing(path)
        .map(|job| job.status())
        .unwrap_or_default()
}

fn open(path: &str) -> anyhow::Result<Connection> {
    let conn = Connection::open_with_flags(path, db_open_flags())?;
    conn.busy_timeout(INDEX_BUILD_BUSY_TIMEOUT)?;
    Ok(conn)
}

/// Build PSF Guard's query indexes on a catalog, off the open path.
///
/// This is an optimization, not a correctness step, and the expensive half
/// of opening a catalog: building an index takes the catalog's write lock,
/// which on a rollback journal blocks every other writer meanwhile. A
/// catalog being written now is left alone; per-target lookups scan until
/// the next start builds the indexes.
fn build_indexes(path: &str) {
    match open(path).and_then(|conn| crate::db::ensure_query_indexes(&conn)) {
        Ok(true) => tracing::info!("Added PSF Guard query indexes to {path}"),
        Ok(false) => {}
        Err(error) => tracing::info!(
            "Left PSF Guard query indexes for later on {path}: {error:#}. \
             Queries still work; per-target lookups scan the image table."
        ),
    }
}

/// Fill in readout-mode names for calibration frames imported before the
/// catalog kept them: one commit per hundred frames, a progress line every
/// chunk, and a clean stop if the capture software holds the catalog. The
/// next start picks up where it left off.
fn read_readout_modes(path: &str, job: &Job, keep_going: impl Fn() -> bool) {
    let outcome = open(path).and_then(|conn| {
        let mut announced = false;
        crate::calibration::backfill_readout_mode_names(&conn, keep_going, |done, total| {
            job.update(|status| {
                status.done = done;
                status.total = total;
            });
            if !announced {
                tracing::info!(
                    "Reading the headers of {total} calibration frame(s) in {path} to \
                     record their readout modes; the server is already up"
                );
                announced = true;
            }
            if done < total {
                tracing::info!("Readout modes: {done} of {total} frames in {path}");
            }
        })
    });
    match outcome {
        Ok(outcome) if outcome.pending == 0 => {}
        Ok(outcome) => tracing::info!(
            "Recorded readout modes in {path}: {} named, {} name none, {} unreadable \
             (left for the next start)",
            outcome.named,
            outcome.nameless,
            outcome.unreadable
        ),
        Err(error) => tracing::info!(
            "Paused recording readout modes in {path}: {error:#}. \
             The next server start continues where this left off."
        ),
    }
}

/// Pair the calibrated and registered copies in a published tree with their
/// lights. Stands aside while an import holds the catalog; the next refresh
/// runs it again.
fn pair_copies(path: &str, pairing: &Pairing, checked: &mut HashMap<PathBuf, FileStamp>) {
    if !pairing.enabled.is_on() {
        return;
    }
    let files: Vec<PathBuf> = pairing.tree.get_fits_files().into_iter().cloned().collect();
    let outcome = match pairing.import_lock.try_lock() {
        Err(_) => Ok(None),
        Ok(_guard) => open(path).and_then(|mut conn| {
            crate::frame_derivatives::pair_files_on_disk(&mut conn, &files, checked).map(Some)
        }),
    };
    match outcome {
        Ok(Some(report)) if report.paired > 0 || report.ambiguous > 0 => tracing::info!(
            "Paired {} calibrated or registered cop(ies) with their lights in {path} \
             ({} superseded, {} ambiguous, {} with no light yet)",
            report.paired,
            report.superseded,
            report.ambiguous,
            report.unmatched
        ),
        Ok(_) => {}
        Err(error) => tracing::info!(
            "Paused pairing calibrated copies in {path}: {error:#}. \
             The next directory refresh tries again."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> Arc<AppState> {
        Arc::new(AppState::new_for_test(
            Connection::open_in_memory().unwrap(),
        ))
    }

    fn wait_for_runner_to_exit(path: &str) {
        let job = JOBS.job(path);
        for _ in 0..400 {
            if !job.queue().has_runner() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the upkeep runner never let go of {path}");
    }

    #[test]
    fn upkeep_builds_the_indexes_then_reads_readout_modes_then_pairs() {
        let state = test_state();
        let temp = tempfile::tempdir().unwrap();
        let mut queue = Queue {
            indexes: true,
            readout_modes: true,
            pairing: Some(Pairing {
                tree: Arc::new(DirectoryTree::build_multiple(&[temp.path()]).unwrap()),
                enabled: LiveSetting::new(true),
                import_lock: Arc::new(TokioMutex::new(())),
            }),
            ..Queue::default()
        };
        let mut next = || CatalogUpkeep::next(&state, "c", &mut queue, Instant::now());
        assert!(matches!(next(), Step::Run(Item::Indexes)));
        assert!(matches!(next(), Step::Run(Item::ReadoutModes)));
        assert!(matches!(next(), Step::Run(Item::Pairing(_))));
        assert!(matches!(next(), Step::Exit));
    }

    #[test]
    fn a_catalog_the_server_started_is_paired_after_a_refresh_and_others_are_not() {
        let state = test_state();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("catalog.sqlite");
        Connection::open(&path).unwrap();
        let path = path.to_string_lossy().into_owned();
        let tree = || Arc::new(DirectoryTree::build_multiple(&[temp.path()]).unwrap());
        let lock = Arc::new(TokioMutex::new(()));

        // No server has started this catalog's upkeep: nothing is queued.
        tree_published(&path, tree(), LiveSetting::new(true), Arc::clone(&lock));
        assert!(JOBS.existing(&path).is_none());

        start(&state, &path);
        wait_for_runner_to_exit(&path);
        tree_published(&path, tree(), LiveSetting::new(true), Arc::clone(&lock));
        wait_for_runner_to_exit(&path);
        assert_eq!(status(&path).stage, UpkeepStage::Idle);
        // Turned off in Settings, a refresh pairs nothing.
        tree_published(&path, tree(), LiveSetting::new(false), lock);
        assert!(JOBS.job(&path).queue().pairing.is_none());
    }

    #[test]
    fn a_failed_runner_clears_the_stage() {
        let mut status = UpkeepStatus {
            stage: UpkeepStage::ReadingReadoutModes,
            done: 3,
            total: 9,
        };
        CatalogUpkeep::runner_failed(&mut Queue::default(), &mut status);
        assert_eq!(status.stage, UpkeepStage::Idle);
        assert_eq!(status.total, 0);
    }
}
