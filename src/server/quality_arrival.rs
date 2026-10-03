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
//! database that asks, for frames that arrived while it was down.

use crate::server::database_context::DatabaseContext;
use crate::server::state::AppState;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long after the last arrival the scan waits, so a batch arriving over
/// a few minutes is scanned once.
const SETTLE: Duration = Duration::from_secs(2 * 60);
/// How long a frame the scan tried stays out of the next attempts.
const RETRY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the queue is looked at.
const POLL: Duration = Duration::from_secs(30);

/// Databases with arrivals waiting, and when the last one came.
static PENDING: std::sync::LazyLock<Mutex<HashMap<String, Instant>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
/// Frames queued for a scan, per database, and when.
static TRIED: std::sync::LazyLock<Mutex<HashMap<String, HashMap<i32, Instant>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Frames arrived in a database. Cheap: the scan starts later, if the
/// database asks for it.
pub fn note_arrival(database_id: &str) {
    PENDING
        .lock()
        .unwrap()
        .insert(database_id.to_string(), Instant::now());
}

/// Start the scans arrivals asked for, once they settle. Holds the state
/// weakly, so a server the desktop app restarted stops this with it.
pub async fn run(state: Arc<AppState>) {
    // Frames that arrived while the server was down raised no arrival.
    for ctx in state.all_databases() {
        if ctx.analyze_new_frames.load(Ordering::Relaxed) {
            note_arrival(&ctx.id);
        }
    }
    let state = Arc::downgrade(&state);
    let mut interval = tokio::time::interval(POLL);
    loop {
        interval.tick().await;
        let Some(state) = state.upgrade() else {
            return;
        };
        let settled: Vec<String> = PENDING
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, at)| at.elapsed() >= SETTLE)
            .map(|(database, _)| database.clone())
            .collect();
        for database in settled {
            let Some(ctx) = state.get_database(&database) else {
                PENDING.lock().unwrap().remove(&database);
                continue;
            };
            if !ctx.analyze_new_frames.load(Ordering::Relaxed) {
                PENDING.lock().unwrap().remove(&database);
                continue;
            }
            // A backfill or scan under way would refuse this one: wait for
            // it rather than read the whole catalog every poll meanwhile.
            if ctx.quality_backfill.read().unwrap().progress.running
                || ctx.spatial_metrics.read().unwrap().progress.running
            {
                continue;
            }
            let lookup = Arc::clone(&ctx);
            let unmeasured = tokio::task::spawn_blocking(move || unmeasured_frames(&lookup))
                .await
                .ok()
                .flatten();
            let Some(unmeasured) = unmeasured else {
                // The catalog could not be read now; try at the next poll.
                continue;
            };
            let targets: Vec<i32> = untried(&database, &unmeasured);
            if targets.is_empty() {
                PENDING.lock().unwrap().remove(&database);
                continue;
            }
            // A backfill already running keeps the arrival waiting.
            if crate::server::handlers::spawn_quality_backfill(&state, ctx, targets, false, false) {
                PENDING.lock().unwrap().remove(&database);
                let now = Instant::now();
                let mut tried = TRIED.lock().unwrap();
                let tried = tried.entry(database.clone()).or_default();
                for (image_id, _) in &unmeasured {
                    tried.entry(*image_id).or_insert(now);
                }
                tracing::info!(db = %database, "📐 Analyzing quality of newly arrived frames");
            }
        }
    }
}

/// The targets of unmeasured frames not tried in the last day.
fn untried(database: &str, unmeasured: &[(i32, i32)]) -> Vec<i32> {
    let mut tried = TRIED.lock().unwrap();
    let tried = tried.entry(database.to_string()).or_default();
    tried.retain(|_, at| at.elapsed() < RETRY_AFTER);
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
        let database = "quality-arrival-test";
        TRIED
            .lock()
            .unwrap()
            .insert(database.into(), [(1, Instant::now())].into_iter().collect());

        // Frame 1 was tried; frame 2 on target 20 was not.
        assert_eq!(untried(database, &[(1, 10), (2, 20)]), vec![20]);
        assert!(untried(database, &[(1, 10)]).is_empty());
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
            temp.path().join("cache").to_string_lossy().into_owned(),
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
