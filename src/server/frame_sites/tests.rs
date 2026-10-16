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
    assert!(matches!(queue.next_step(start), Step::Nap(_)), "settling");
    assert_eq!(queue.next_step(start + SETTLE), Step::Run);
    // The volume was away: tried again a minute later, then two.
    let now = start + SETTLE;
    assert!(queue.retry(now));
    assert!(matches!(queue.next_step(now), Step::Nap(_)));
    assert_eq!(queue.next_step(now + RETRY_FIRST), Step::Run);
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

#[test]
fn stop_pauses_until_resume_or_a_change() {
    let now = Instant::now();
    let mut queue = Queue::default();
    queue.add(now);
    queue.stop();
    assert!(queue.stopped);
    assert_eq!(queue.next_step(now + SETTLE), Step::Exit);
    queue.add(now);
    assert!(!queue.stopped);
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
         DELETE FROM psf_guard_rig_site_seen;
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
    run_logged(&state, &path, &job);
    let last = job.status.read().unwrap().clone();
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
    run_logged(&state, &path, &job);
    let done = job.status.read().unwrap().clone();
    let outcome = done.last.unwrap();
    assert_eq!((outcome.clocked, outcome.volume_away), (3, false));
    assert_eq!((done.done, done.total), (3, 3));
    assert_eq!(done.last_error, None);
    assert_eq!(job.queue().attempts, 0);
    // Its rig went to flat measuring, which waits for grading.
    assert!(!crate::server::flat_measure::status(&path).rigs.is_empty());
}
