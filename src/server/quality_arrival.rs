//! Quality analysis of frames as they arrive.
//!
//! Frames that reach a database by sync, peer pull, remote upload or
//! auto-import are scanned only when someone runs the quality backfill, so
//! a rig that syncs every night keeps a growing tail of unmeasured frames,
//! and a target scored on two kinds of star count. A database with
//! **Analyze new frames as they arrive** on queues the background quality
//! scan for the targets that hold unmeasured frames, a couple of minutes
//! after the last arrival, so a night's batch is scanned once. The scan
//! writes nothing to the catalog: its results stay in the quality cache.
//!
//! A frame the scan tried and could not measure (its file is missing, say)
//! is not tried again for a day, so a broken file never keeps the scan busy.
//! That memory lasts until the server restarts. Each start checks every
//! database that asks, for frames that arrived while it was down. One job
//! per database waits for arrivals to settle ([`CatalogJobs`]).

use crate::concurrency::Stop;
use crate::server::database_context::DatabaseContext;
use crate::server::job_line::catalog::{CatalogJob, CatalogWork, Settling, Step};
use crate::server::state::AppState;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long after the last arrival the scan waits, so a batch arriving over
/// a few minutes is scanned once.
const SETTLE: Duration = Duration::from_secs(2 * 60);
/// How long a frame the scan tried stays out of the next attempts.
const RETRY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// How long the job waits before it looks again at a database that could
/// not take a scan: one already scanning, or a catalog it could not read.
const RETRY: Duration = Duration::from_secs(30);

/// Scans of newly arrived frames, run on one thread per database.
pub(crate) struct Arrivals;

type Job = CatalogJob<Arrivals>;

/// What waits for one database's scan of arrived frames.
#[derive(Debug, Default)]
pub(crate) struct Queue {
    /// Frames arrived: the scan is queued once arrivals pause for
    /// [`SETTLE`].
    arrivals: Settling,
    /// When to look again at a database that could not take a scan.
    retry_at: Option<Instant>,
    /// Frames queued for a scan, and when, so a frame the scan could not
    /// measure is not queued again for [`RETRY_AFTER`].
    tried: HashMap<i32, Instant>,
}

impl CatalogWork for Arrivals {
    type Queue = Queue;
    type Status = ();
    type Item = ();
    const RUNNER: &'static str = "quality-arrival";
    /// It reads the catalog and queues a scan; the scan holds shutdown.
    const SHUTDOWN_WAITS_FOR: Option<&'static str> = None;

    fn next(_: &AppState, _: &str, queue: &mut Queue, now: Instant) -> Step<()> {
        if let Some(at) = queue.retry_at {
            if now < at {
                return Step::Sleep(at - now);
            }
            queue.retry_at = None;
            return Step::Run(());
        }
        queue.arrivals.step(now, SETTLE, None)
    }

    fn run(state: &Arc<AppState>, database_id: &str, job: &Job, (): (), _: &Stop) {
        queue_scan(state, database_id, job);
    }

    fn has_work(queue: &Queue) -> bool {
        queue.arrivals.is_pending() || queue.retry_at.is_some()
    }

    fn runner_failed(queue: &mut Queue, _: &mut ()) {
        queue.retry_at = None;
    }
}

/// Frames arrived in a database. Cheap: the scan is queued later, if the
/// database asks for it.
pub fn note_arrival(state: &Arc<AppState>, database_id: &str) {
    // One job per database id.
    state
        .catalog_jobs
        .arrivals
        .request(state, database_id, |queue| {
            queue.arrivals.touch(Instant::now());
        });
}

/// Frames that arrived while the server was down raised no arrival: every
/// database that analyzes new frames is looked at once arrivals settle.
pub fn note_start(state: &Arc<AppState>) {
    for ctx in state.all_databases() {
        if ctx.analyze_new_frames.is_on() {
            note_arrival(state, &ctx.id);
        }
    }
}

/// Queue the database-wide analysis for the targets that hold unmeasured
/// frames not tried in the last day. A database that is scanning already,
/// or whose catalog cannot be read now, is looked at again after [`RETRY`].
fn queue_scan(state: &Arc<AppState>, database_id: &str, job: &Job) {
    let Some(ctx) = state.get_database(database_id) else {
        return;
    };
    if !ctx.analyze_new_frames.is_on() {
        return;
    }
    let retry = || job.queue().retry_at = Some(Instant::now() + RETRY);
    // A backfill or scan under way would refuse this one: wait for it
    // rather than read the whole catalog meanwhile.
    if ctx.quality_backfill.read().unwrap().progress.running
        || ctx.spatial_metrics.read().unwrap().progress.running
    {
        retry();
        return;
    }
    let Some(unmeasured) = unmeasured_frames(&ctx) else {
        retry();
        return;
    };
    let targets = untried(&mut job.queue().tried, &unmeasured, Instant::now());
    if targets.is_empty() {
        return;
    }
    // A backfill already running keeps the arrival waiting.
    if crate::server::handlers::spawn_quality_backfill(state, ctx, targets, false, false) {
        let now = Instant::now();
        let mut queue = job.queue();
        for (image_id, _) in &unmeasured {
            queue.tried.entry(*image_id).or_insert(now);
        }
        tracing::info!(db = %database_id, "📐 Analyzing quality of newly arrived frames");
    } else {
        retry();
    }
}

/// The targets of unmeasured frames not tried in the last day.
fn untried(tried: &mut HashMap<i32, Instant>, unmeasured: &[(i32, i32)], now: Instant) -> Vec<i32> {
    tried.retain(|_, at| now.saturating_duration_since(*at) < RETRY_AFTER);
    let mut targets: Vec<i32> = unmeasured
        .iter()
        .filter(|(image_id, _)| !tried.contains_key(image_id))
        .map(|(_, target_id)| *target_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    targets.sort_unstable();
    targets
}

/// Every light frame with no current quality measurement, as (image, target).
/// `None` when the catalog cannot be read.
fn unmeasured_frames(ctx: &DatabaseContext) -> Option<Vec<(i32, i32)>> {
    // After a start or a database edit the store is empty until loaded, and
    // every frame would look unmeasured.
    crate::server::spatial_scan::ensure_loaded(&ctx.spatial_metrics, &ctx.cache_dir_path);
    let connection = crate::server::database_context::open_scheduler_connection_with_flags(
        &ctx.database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let mut statement = connection
        .prepare("SELECT Id, targetId, metadata FROM acquiredimage")
        .ok()?;
    let rows: Vec<(i32, i32, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .ok()?
        .collect::<rusqlite::Result<_>>()
        .ok()?;
    Some(
        rows.into_iter()
            .filter_map(|(image_id, target_id, metadata)| {
                let filename = crate::server::handlers::filename_from_metadata(&metadata)?;
                crate::server::spatial_scan::valid_quality_entry(
                    &ctx.spatial_metrics,
                    image_id,
                    &filename,
                )
                .is_none()
                .then_some((image_id, target_id))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_tried_today_is_not_queued_again() {
        let now = Instant::now();
        let mut tried: HashMap<i32, Instant> = [(1, now)].into_iter().collect();
        // Frame 1 was tried; frame 2 on target 20 was not.
        assert_eq!(untried(&mut tried, &[(1, 10), (2, 20)], now), vec![20]);
        assert!(untried(&mut tried, &[(1, 10)], now).is_empty());
        // A day on, frame 1 is tried again.
        assert_eq!(untried(&mut tried, &[(1, 10)], now + RETRY_AFTER), vec![10]);
    }

    #[test]
    fn arrivals_queue_a_scan_once_they_settle_and_a_busy_database_is_retried() {
        let state = AppState::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
        let start = Instant::now();
        let mut queue = Queue::default();
        let next = |queue: &mut Queue, now| Arrivals::next(&state, "db", queue, now);
        assert_eq!(next(&mut queue, start), Step::Exit);
        queue.arrivals.touch(start);
        assert_eq!(next(&mut queue, start), Step::Sleep(SETTLE));
        assert_eq!(next(&mut queue, start + SETTLE), Step::Run(()));
        assert_eq!(next(&mut queue, start + SETTLE), Step::Exit);
        // A busy database is looked at again after the retry wait.
        queue.retry_at = Some(start + RETRY);
        assert!(Arrivals::has_work(&queue));
        assert_eq!(next(&mut queue, start), Step::Sleep(RETRY));
        assert_eq!(next(&mut queue, start + RETRY), Step::Run(()));
        assert!(!Arrivals::has_work(&queue));
    }

    #[test]
    fn a_failed_runner_drops_its_retry() {
        let mut queue = Queue {
            retry_at: Some(Instant::now()),
            ..Queue::default()
        };
        Arrivals::runner_failed(&mut queue, &mut ());
        assert!(queue.retry_at.is_none());
    }

    #[test]
    fn frames_without_a_measurement_are_listed_with_their_targets() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("catalog.sqlite");
        crate::ts_schema::create_fresh_db(&database).unwrap();
        let images = temp.path().join("images");
        std::fs::create_dir_all(&images).unwrap();
        let ctx = DatabaseContext::new(
            "db".into(),
            "Db".into(),
            database.to_string_lossy().into_owned(),
            vec![images.to_string_lossy().into_owned()],
            None,
            None,
            None,
            temp.path().join("cache"),
        )
        .unwrap();
        {
            let connection = ctx.db();
            let connection = connection.lock().unwrap();
            connection
                .execute_batch(
                    "INSERT INTO project (Id, profileId, name, isMosaic, flatsHandling)
                         VALUES (1, 'p', 'Project', 0, 0);
                     INSERT INTO target (Id, name, active, epochcode, projectId)
                         VALUES (3, 'Target', 1, 0, 1);
                     INSERT INTO acquiredimage
                        (Id, projectId, targetId, acquireddate, filtername, gradingStatus,
                         metadata, profileId)
                     VALUES (7, 1, 3, 0, 'Ha', 0, '{\"FileName\": \"a.fits\"}', 'p'),
                            (8, 1, 3, 0, 'Ha', 0, '{}', 'p');",
                )
                .unwrap();
        }

        // Frame 8 names no file, so there is nothing to scan.
        assert_eq!(unmeasured_frames(&ctx), Some(vec![(7, 3)]));
    }
}
