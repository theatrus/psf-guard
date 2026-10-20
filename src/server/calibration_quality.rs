//! The calibration quality job: measure new calibration frames and grade
//! the library, in the background, one pass per catalog at a time.
//!
//! A pass measures every raw frame that has no measurement from the current
//! algorithm of its current file, then grades the whole library (see
//! [`crate::calibration::grade_library`]). It runs when the server starts,
//! after an import, after a forget, and two minutes after remote uploads
//! stop arriving, so a batch sent one frame at a time shares a pass. When it
//! ends, the calibration health of every project is judged again (see
//! [`crate::server::calibration_health`]). A pass measures only the frames
//! it found when it began, so a request while one runs makes it run once
//! more when it ends.
//!
//! Measuring reads every pixel of a frame. The pass leases its workers from
//! the shared background budget, whose memory ceiling sizes it, and reads
//! as many frames at once as it has workers, so on a network share one
//! frame's read overlaps another's measuring. Seiza runs on those workers
//! alone, and each frame waits while interactive work runs.

use crate::calibration::{GradeOutcome, MeasureOutcome};
use crate::concurrency::{ComputePool, Priority, Stop};
use crate::server::api::ApiResponse;
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;
use crate::server::job_line::catalog::{CatalogJob, CatalogWork, Settling, Step};
use crate::server::state::AppState;
use axum::Json;
use rayon::prelude::*;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the pass waits after the last calibration frame a remote client
/// uploads.
const UPLOAD_SETTLE: Duration = Duration::from_secs(2 * 60);

/// How long the pass waits for the catalog's write lock before it stops for
/// now. Short, as for the other background passes: N.I.N.A. may be saving
/// frames into this file, and the next pass continues where this one left
/// off.
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// How often a pass looks again while interactive work runs.
const YIELD_POLL: Duration = Duration::from_millis(250);

/// Where a catalog's grading stands, for the progress route.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CalibrationQualityProgress {
    pub running: bool,
    /// `measuring`, `waiting` while interactive work runs, or `grading`.
    pub stage: Option<String>,
    /// Frames the running or last pass set out to measure.
    pub to_measure: usize,
    /// Of those, how many it has read so far.
    pub done: usize,
    /// Workers leased from the background budget.
    pub workers: usize,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    /// What the last finished pass measured and graded.
    pub last_measured: Option<MeasureOutcome>,
    pub last_graded: Option<GradeOutcome>,
    /// Why the last pass stopped early, if it did.
    pub last_error: Option<String>,
}

/// Calibration grading passes, run on one thread per catalog.
pub(crate) struct CalibrationQuality;

type Job = CatalogJob<CalibrationQuality>;

/// What waits for a pass of one catalog.
#[derive(Debug, Default)]
pub(crate) struct Queue {
    /// Whether a pass was requested after the last pass started.
    pass_requested: bool,
    /// Frames uploaded to the catalog: a pass runs once uploads pause for
    /// [`UPLOAD_SETTLE`].
    uploads: Settling,
}

impl CatalogWork for CalibrationQuality {
    type Queue = Queue;
    type Status = CalibrationQualityProgress;
    type Item = ();
    const RUNNER: &'static str = "calibration-quality";
    /// Shutdown waits until the batch being measured is committed.
    const SHUTDOWN_WAITS_FOR: Option<&'static str> = Some("calibration grading");

    /// A pass measures only the frames that existed when it started, so a
    /// request during a pass starts one more pass after it ends. Uploads
    /// start a pass once they pause for [`UPLOAD_SETTLE`].
    fn next(_: &AppState, _: &str, queue: &mut Queue, now: Instant) -> Step<()> {
        if std::mem::take(&mut queue.pass_requested) {
            // The pass reads every frame the uploads brought as well.
            queue.uploads = Settling::default();
            return Step::Run(());
        }
        queue.uploads.step(now, UPLOAD_SETTLE, None)
    }

    fn run(state: &Arc<AppState>, path: &str, job: &Job, (): (), _: &Stop) {
        run_logged(state, path, job);
    }

    /// Grades decide which calibration frames each project's lights use, so
    /// health is judged again after the last pass of a batch.
    fn after_queue_empty(state: &Arc<AppState>, path: &str) {
        crate::server::calibration_health::request(state, path);
    }

    fn has_work(queue: &Queue) -> bool {
        queue.pass_requested || queue.uploads.is_pending()
    }

    fn runner_failed(_: &mut Queue, progress: &mut CalibrationQualityProgress) {
        progress.running = false;
        progress.stage = None;
        progress.last_error = Some("calibration grading stopped unexpectedly".into());
    }
}

/// The job of the catalog file at `path`, whichever database context
/// asks: a database edit replaces its context, and two passes over one file
/// would measure every frame twice.
fn job(state: &AppState, path: &str) -> Arc<Job> {
    state.catalog_jobs.calibration_quality.job(path)
}

/// Whether a grading pass runs over the catalog at `path`. It commits
/// grades in batches, and each batch changes every project's calibration
/// health key.
pub fn running(state: &AppState, path: &str) -> bool {
    let job = job(state, path);
    let queue = job.queue();
    queue.running() || queue.pass_requested
}

/// Whether grading runs over the catalog at `path`, or waits for uploads
/// to it to stop: flat measuring waits for it, since grades decide which
/// flats are in use.
pub fn busy(state: &AppState, path: &str) -> bool {
    let job = job(state, path);
    let queue = job.queue();
    queue.running() || queue.pass_requested || queue.uploads.is_pending()
}

/// Where grading stands for the catalog at `path`.
pub fn progress(state: &AppState, path: &str) -> CalibrationQualityProgress {
    job(state, path).status()
}

/// Measure what is new in the catalog at `path` and grade it, on a thread
/// of its own.
pub fn request(state: &Arc<AppState>, path: &str) {
    state
        .catalog_jobs
        .calibration_quality
        .request(state, path, |queue| queue.pass_requested = true);
}

/// Grade every database's library again, after a grade limit changed. A
/// pass reads only frames it has not measured; grading itself needs no
/// pixels. Each pass asks for its catalog's health when it ends.
pub fn request_all(state: &Arc<AppState>) {
    for ctx in state.all_databases() {
        request(state, &ctx.database_path);
    }
}

/// A frame reached the catalog at `path` by remote upload: a calibration
/// frame to grade, or a light whose project's calibration health the pass
/// that follows judges again. The pass starts once uploads to it pause for
/// [`UPLOAD_SETTLE`]; each new frame before then pushes it back.
pub fn note_upload(state: &Arc<AppState>, path: &str) {
    state
        .catalog_jobs
        .calibration_quality
        .request(state, path, |queue| queue.uploads.touch(Instant::now()));
}

fn run_logged(state: &AppState, path: &str, job: &Job) {
    match run_pass(state, path, job) {
        Ok((measured, graded)) => {
            if measured.pending > 0 || graded.changed > 0 {
                tracing::info!(
                    "Graded {} calibration frame(s) in {path}: {} poor, {} suspect \
                     ({} measured this pass, {} unreadable and left for the next)",
                    graded.graded,
                    graded.poor,
                    graded.suspect,
                    measured.measured,
                    measured.unreadable
                );
            }
        }
        Err(error) => {
            tracing::info!(
                "Paused grading calibration frames in {path}: {error:#}. \
                 The next pass continues where this left off."
            );
            job.update(|progress| {
                progress.last_error = Some(format!("{error:#}"));
            });
        }
    }
    job.update(|progress| {
        progress.running = false;
        progress.stage = None;
        progress.finished_at = Some(chrono::Utc::now().timestamp());
    });
}

/// One pass on its own connection: measure, then grade.
fn run_pass(
    state: &AppState,
    path: &str,
    job: &Job,
) -> anyhow::Result<(MeasureOutcome, GradeOutcome)> {
    job.update(|progress| {
        *progress = CalibrationQualityProgress {
            running: true,
            stage: Some("measuring".into()),
            started_at: Some(chrono::Utc::now().timestamp()),
            last_measured: progress.last_measured.take(),
            last_graded: progress.last_graded.take(),
            ..Default::default()
        };
    });
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    if conn.is_readonly(rusqlite::MAIN_DB)? {
        return Ok(Default::default());
    }
    let pending = crate::calibration::pending_measurements(&conn)?;
    job.update(|progress| progress.to_measure = pending.len());
    let measured = if pending.is_empty() {
        MeasureOutcome::default()
    } else {
        let frame_pixels = crate::concurrency::probe_frame_pixels(&pending[0].source_path);
        let budget = crate::concurrency::plan_workers(
            None,
            &state.worker_policy(),
            Priority::Background,
            frame_pixels,
        );
        let lease = state.lease_workers(Priority::Background, budget.workers);
        let pool = ComputePool::take("calibration-quality", lease.workers)?;
        job.update(|progress| progress.workers = lease.workers);
        tracing::info!(
            "Measuring {} calibration frame(s) in {path} to grade them — {}; {}",
            pending.len(),
            budget.rationale,
            lease.summary()
        );
        // Frames are read side by side on the leased workers, so one
        // frame's read from a network share overlaps another's measuring.
        let measure = |frame: &crate::calibration::PendingFrame| {
            // Interactive work comes first; this frame waits for it.
            if state.interactive_job_active() {
                job.update(|progress| progress.stage = Some("waiting".into()));
                while state.interactive_job_active() && !state.shutdown.is_stopping() {
                    std::thread::sleep(YIELD_POLL);
                }
                job.update(|progress| progress.stage = Some("measuring".into()));
            }
            crate::calibration::measure_frame(frame)
        };
        crate::calibration::measure_frames(
            &conn,
            &pending,
            |frames| pool.install(|| frames.par_iter().map(measure).collect()),
            || !state.shutdown.is_stopping(),
            |done, _| job.update(|progress| progress.done = done),
        )?
    };
    if state.shutdown.is_stopping() {
        // Grading reads the whole library; the next start does it.
        return Ok((measured, Default::default()));
    }
    job.update(|progress| {
        progress.stage = Some("grading".into());
        progress.last_measured = Some(measured.clone());
    });
    let graded = crate::calibration::grade_library(&conn)?;
    job.update(|progress| progress.last_graded = Some(graded.clone()));
    Ok((measured, graded))
}

#[derive(Debug, Serialize)]
pub struct CalibrationQualityResponse {
    pub progress: CalibrationQualityProgress,
    pub counts: crate::calibration::QualityCounts,
}

/// `GET /api/db/{db}/calibrations/quality` — the grading job's progress and
/// how the library's frames are graded.
pub async fn get_progress(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    ctx: DbContext,
) -> Result<Json<ApiResponse<CalibrationQualityResponse>>, AppError> {
    let path = ctx.database_path.clone();
    let counts = tokio::task::spawn_blocking({
        let path = path.clone();
        move || {
            let conn = crate::server::database_context::open_scheduler_connection_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )?;
            crate::calibration::quality_counts(&conn)
        }
    })
    .await
    .map_err(|error| AppError::InternalError(format!("counting calibration grades: {error}")))?
    .map_err(|error| AppError::InternalError(format!("counting calibration grades: {error:#}")))?;
    Ok(Json(ApiResponse::success(CalibrationQualityResponse {
        progress: progress(&state, &path),
        counts,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::test_frames::{noise, write_frame};

    /// A catalog with three steady darks and one that caught light.
    fn dark_catalog(root: &std::path::Path) -> String {
        let mut frames = Vec::new();
        for (index, level) in [503.0, 503.0, 503.0, 900.0].into_iter().enumerate() {
            let path = root.join(format!("dark-{index}.fits"));
            write_frame(&path, "DARK", (64, 64), "2026-09-10T10:00:00", |x, y| {
                level + 5.0 * noise(x, y, index as u64)
            });
            frames.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        let path = root.join("catalog.sqlite");
        let mut conn = Connection::open(&path).unwrap();
        let tx = conn.transaction().unwrap();
        crate::calibration::import_calibration_frames(&tx, &frames, Some("profile")).unwrap();
        tx.commit().unwrap();
        path.to_string_lossy().into_owned()
    }

    fn test_state() -> AppState {
        AppState::new_for_test(Connection::open_in_memory().unwrap())
    }

    #[test]
    fn an_upload_starts_a_pass_once_uploads_pause() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("uploads.sqlite");
        let path = path.to_string_lossy();
        let state = Arc::new(test_state());
        let started = Instant::now();
        note_upload(&state, &path);
        note_upload(&state, &path);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a note never waits"
        );
        // The catalog's runner waits for the uploads to settle; grading
        // counts as busy meanwhile, and as not yet running.
        assert!(busy(&state, &path));
        assert!(!running(&state, &path));
        assert!(job(&state, &path).queue().has_runner());
    }

    #[test]
    fn a_pass_measures_and_grades_and_says_how_far_it_got() {
        let temp = tempfile::tempdir().unwrap();
        let path = dark_catalog(temp.path());
        let job = Job::default();
        let (measured, graded) = run_pass(&test_state(), &path, &job).unwrap();
        assert_eq!((measured.measured, graded.poor), (4, 1));
        let progress = job.status();
        assert_eq!((progress.to_measure, progress.done), (4, 4));
        assert!(progress.workers >= 1);
        assert_eq!(progress.last_graded, Some(graded));

        // The next pass has nothing to read and only grades.
        let (measured, graded) = run_pass(&test_state(), &path, &job).unwrap();
        assert_eq!((measured.pending, graded.graded, graded.changed), (0, 4, 0));
    }

    #[test]
    fn a_pass_waits_while_interactive_work_runs() {
        let temp = tempfile::tempdir().unwrap();
        let path = dark_catalog(temp.path());
        let state = Arc::new(test_state());
        let job = Arc::new(Job::default());
        let interactive = state.begin_interactive_job();
        let pass = {
            let (state, job, path) = (Arc::clone(&state), Arc::clone(&job), path.clone());
            std::thread::spawn(move || run_pass(&state, &path, &job).unwrap())
        };
        let deadline = Instant::now() + Duration::from_secs(20);
        while job.status().stage.as_deref() != Some("waiting") {
            assert!(Instant::now() < deadline, "the pass never waited");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(job.status().done, 0, "nothing read yet");
        drop(interactive);
        let (measured, _) = pass.join().unwrap();
        assert_eq!(measured.measured, 4);
    }

    #[test]
    fn a_request_runs_the_pass_in_the_background() {
        let temp = tempfile::tempdir().unwrap();
        let path = dark_catalog(temp.path());
        let state = Arc::new(test_state());
        request(&state, &path);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let progress = progress(&state, &path);
            if !progress.running
                && let Some(graded) = &progress.last_graded
            {
                assert_eq!(graded.poor, 1);
                assert!(progress.finished_at.is_some());
                break;
            }
            assert!(Instant::now() < deadline, "the pass never finished");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_request_during_a_pass_runs_one_more() {
        let state = test_state();
        let mut queue = Queue {
            pass_requested: true,
            ..Queue::default()
        };
        let next = |queue: &mut Queue| CalibrationQuality::next(&state, "c", queue, Instant::now());
        assert_eq!(next(&mut queue), Step::Run(()));
        // Two requests arrive during the pass.
        queue.pass_requested = true;
        queue.pass_requested = true;
        assert_eq!(next(&mut queue), Step::Run(()), "one more pass");
        assert_eq!(next(&mut queue), Step::Exit, "then it stops");
    }

    #[test]
    fn uploads_wait_for_the_catalog_to_go_quiet() {
        let state = test_state();
        let start = Instant::now();
        let mut queue = Queue::default();
        queue.uploads.touch(start);
        let next = |queue: &mut Queue, now| CalibrationQuality::next(&state, "a", queue, now);
        assert_eq!(next(&mut queue, start), Step::Sleep(UPLOAD_SETTLE));
        // A frame a minute later pushes the pass back.
        let later = start + Duration::from_secs(60);
        queue.uploads.touch(later);
        assert_eq!(
            next(&mut queue, start + UPLOAD_SETTLE),
            Step::Sleep(Duration::from_secs(60))
        );
        assert_eq!(next(&mut queue, later + UPLOAD_SETTLE), Step::Run(()));
        assert_eq!(next(&mut queue, later + UPLOAD_SETTLE), Step::Exit);
        // A pass requested meanwhile also reads the uploaded frames.
        queue.uploads.touch(later);
        queue.pass_requested = true;
        assert_eq!(next(&mut queue, later), Step::Run(()));
        assert!(!queue.uploads.is_pending());
    }
}
