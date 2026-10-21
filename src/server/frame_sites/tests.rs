use super::*;

fn test_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test(
        Connection::open_in_memory().unwrap(),
    ))
}

#[test]
fn requests_settle_and_unfinished_passes_wait_longer_each_time() {
    let start = Instant::now();
    let mut queue = Queue::default();
    assert_eq!(queue.next_step(start), Step::Exit);
    queue.add(start);
    assert!(matches!(queue.next_step(start), Step::Sleep(_)), "settling");
    assert_eq!(queue.next_step(start + SETTLE), Step::Run(()));
    // The volume was away: tried again a minute later, then two.
    let now = start + SETTLE;
    assert!(queue.retry(now));
    assert!(matches!(queue.next_step(now), Step::Sleep(_)));
    assert_eq!(queue.next_step(now + RETRY_FIRST), Step::Run(()));
    assert!(queue.retry(now));
    assert_eq!(queue.due, Some(now + RETRY_FIRST * 2));
    for _ in 2..RETRY_LIMIT {
        assert!(queue.retry(now));
    }
    assert_eq!(queue.due, Some(now + RETRY_MAX), "capped");
    assert!(!queue.retry(now), "out of tries until the next change");
    assert_eq!(queue.next_step(now + RETRY_MAX), Step::Exit);
    // A change starts the tries again.
    queue.add(now);
    assert_eq!(queue.attempts, 0);
}

/// The hourly watch does not reset a retry in flight.
#[test]
fn the_watch_respects_a_waiting_retry() {
    let now = Instant::now();
    // A retry waits after an unfinished pass: the watch leaves it be.
    let mut queue = Queue::default();
    queue.add(now);
    assert_eq!(queue.next_step(now + SETTLE), Step::Run(()));
    assert!(queue.retry(now));
    assert!(queue.retry(now));
    let due = queue.due;
    queue.nudge(now);
    assert_eq!((queue.attempts, queue.due, queue.pending), (2, due, false));
    // At rest, it queues a pass.
    let mut queue = Queue::default();
    queue.nudge(now);
    assert!(queue.pending);
    assert_eq!(queue.next_step(now + SETTLE), Step::Run(()));
}

/// A paused job is not started by the watch's request; Resume runs the
/// pass it queued.
#[test]
fn the_watch_starts_no_runner_for_a_paused_job() {
    let temp = tempfile::tempdir().unwrap();
    let (path, _files) = library(temp.path(), 1);
    let state = test_state();
    assert!(job(&state, &path).pause());
    nudge(&state, &path);
    assert!(!job(&state, &path).queue().has_runner());
    assert_eq!(status(&state, &path).state, "paused");
    assert!(state.catalog_jobs.frame_sites.resume(&state, &path));
    wait_until("the pass ran after Resume", || {
        !job(&state, &path).queue().has_runner() && status(&state, &path).last.is_some()
    });
}

/// Frames that come while a pass runs are another pass, and the status
/// says so.
#[test]
fn the_status_says_when_another_pass_waits() {
    let state = test_state();
    let path = "/next-pass/frame-sites.sqlite";
    let job = job(&state, path);
    job.queue().running = true;
    assert!(!status(&state, path).next_pass);
    job.queue().add(Instant::now());
    let running = status(&state, path);
    assert_eq!(running.state, "running");
    assert!(running.next_pass);
    job.queue().running = false;
    let queued = status(&state, path);
    assert_eq!(queued.state, "queued");
    assert!(!queued.next_pass, "only behind a pass under way");
    job.queue().pending = false;
}

/// A catalog of `count` flats with a PDT clock, their sites not yet read.
fn library(dir: &std::path::Path, count: usize) -> (String, Vec<std::path::PathBuf>) {
    let mut frames = Vec::new();
    let mut files = Vec::new();
    for index in 0..count {
        let path = dir.join(format!("f{index}.fits"));
        let at = 1_787_140_800 + index as i64 * 60;
        let text = |at: i64| {
            chrono::DateTime::from_timestamp(at, 0)
                .unwrap()
                .naive_utc()
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string()
        };
        let card = format!("DATE-LOC= '{}'", text(at - 7 * 3600));
        crate::calibration::test_frames::write_frame_with(
            &path,
            "FLAT",
            (16, 16),
            &text(at),
            &[card.as_str()],
            |_, _| 20_000.0,
        );
        frames.push(crate::commands::import::headers::read_frame_meta(&path));
        files.push(path);
    }
    let catalog = dir.join("catalog.sqlite");
    let mut conn = Connection::open(&catalog).unwrap();
    let tx = conn.transaction().unwrap();
    crate::calibration::import_calibration_frames(&tx, &frames, Some("profile")).unwrap();
    tx.commit().unwrap();
    conn.execute_batch(
        "DELETE FROM psf_guard_rig_site;
         DELETE FROM psf_guard_rig_site_night;
         UPDATE psf_guard_calibration_frame
         SET site_longitude = NULL, site_utc_offset_s = NULL, site_read_at = NULL;",
    )
    .unwrap();
    (catalog.to_string_lossy().into_owned(), files)
}

/// With the volume away nothing is marked, and the pass is tried again
/// while the server runs; once the files are back it reads them, says how
/// far it got, and hands the rigs it placed to grading and flat measuring.
#[test]
fn a_volume_away_is_tried_again_and_its_frames_read_once_back() {
    let _settings = crate::calibration::tests::POLICY_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let (path, files) = library(temp.path(), 3);
    let state = test_state();
    let job = Job::default();
    // Unmounted: every file away.
    let away = temp.path().join("away");
    std::fs::create_dir(&away).unwrap();
    for file in &files {
        std::fs::rename(file, away.join(file.file_name().unwrap())).unwrap();
    }
    job.queue().running = true;
    run_logged(&state, &path, &job, &Stop::new());
    let last = job.status().clone();
    assert!(last.last.as_ref().unwrap().volume_away);
    assert!(last.last_error.is_some());
    assert_eq!(job.queue().attempts, 1);
    assert!(
        job.queue().due.is_some(),
        "tried again while the server runs"
    );
    let conn = Connection::open(&path).unwrap();
    let marked: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM psf_guard_calibration_frame WHERE site_read_at IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(marked, 0, "nothing marked away");

    // Mounted again.
    for file in &files {
        std::fs::rename(away.join(file.file_name().unwrap()), file).unwrap();
    }
    job.queue().running = true;
    run_logged(&state, &path, &job, &Stop::new());
    let done = job.status().clone();
    let outcome = done.last.unwrap();
    assert_eq!((outcome.clocked, outcome.volume_away), (3, false));
    assert_eq!((done.done, done.total), (3, 3));
    assert_eq!(done.last_error, None);
    assert_eq!(job.queue().attempts, 0);
    // Its rig went to flat measuring, which waits for grading.
    assert!(!crate::server::flat_measure::status(&state, &path)
        .rigs
        .is_empty());
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(30), "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The runner reads what is pending, lets go of the catalog once nothing
/// waits, and a later request starts another.
#[test]
fn the_runner_exits_when_idle_and_starts_again_on_request() {
    let _settings = crate::calibration::tests::POLICY_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let (path, _files) = library(temp.path(), 2);
    let state = test_state();
    queue(&state, &path);
    assert!(job(&state, &path).queue().has_runner());
    wait_until("the runner never let go", || {
        !job(&state, &path).queue().has_runner()
    });
    let done = status(&state, &path);
    assert_eq!(done.state, "idle");
    assert_eq!(done.last.unwrap().clocked, 2);
    queue(&state, &path);
    assert!(
        job(&state, &path).queue().has_runner(),
        "a new request starts a runner"
    );
    wait_until("the second runner never let go", || {
        !job(&state, &path).queue().has_runner()
    });
}

/// A stop of the server ends the runner, and no request starts one.
#[test]
fn the_runner_ends_with_the_server() {
    let temp = tempfile::tempdir().unwrap();
    let (path, _files) = library(temp.path(), 1);
    let state = test_state();
    queue(&state, &path);
    state.shutdown.begin();
    wait_until("the runner outlived the server", || {
        !job(&state, &path).queue().has_runner()
    });
    queue(&state, &path);
    assert!(!job(&state, &path).queue().has_runner());
}

/// When the runner thread fails, the status shows an error and no pass is
/// running.
#[test]
fn a_failed_runner_reports_an_error() {
    let mut queue = Queue {
        running: true,
        ..Queue::default()
    };
    let mut status = FrameSitesStatus {
        active: true,
        ..FrameSitesStatus::default()
    };
    FrameSites::runner_failed(&mut queue, &mut status);
    assert!(!queue.running && !status.active);
    assert!(status.last_error.is_some());
}

/// A pause during a pass ends it at its next chunk, and the pass runs
/// again at Resume.
#[test]
fn a_pause_during_a_pass_runs_it_again_at_resume() {
    let _settings = crate::calibration::tests::POLICY_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let (path, _files) = library(temp.path(), 2);
    let state = test_state();
    let job = Job::default();
    job.queue().running = true;
    let paused = Stop::new();
    paused.request();
    run_logged(&state, &path, &job, &paused);
    assert_eq!(job.status().last.as_ref().unwrap().done, 0);
    let queue = job.queue();
    assert!(!queue.running && queue.pending, "the pass waits for Resume");
    assert_eq!(queue.attempts, 0, "a pause is not a failed try");
}

/// Frames marked away come due a week on, for the watch to queue.
#[test]
fn frames_marked_away_come_due_after_a_week() {
    let _settings = crate::calibration::tests::POLICY_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let (path, _files) = library(temp.path(), 1);
    let conn = Connection::open(&path).unwrap();
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE psf_guard_calibration_frame SET site_read_at = ?1",
        [-now],
    )
    .unwrap();
    assert!(!crate::calibration::night::away_frames_due(&conn));
    conn.execute(
        "UPDATE psf_guard_calibration_frame SET site_read_at = ?1",
        [-(now - crate::calibration::night::SITE_RETRY_SECONDS - 60)],
    )
    .unwrap();
    assert!(crate::calibration::night::away_frames_due(&conn));
}
