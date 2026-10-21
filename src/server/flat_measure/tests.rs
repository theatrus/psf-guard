use super::*;
use crate::calibration::flat_groups::job::{Rigs, Scope};
use crate::calibration::flat_groups::store;
use crate::calibration::import_calibration_frames;
use crate::calibration::test_frames::write_frame_with;
use crate::commands::import::headers::{read_frame_meta, FrameMeta};
use crate::server::extract::DbContext;
use axum::extract::State;
use axum::Json;
use serde_json::json;

const SIZE: (usize, usize) = (240, 160);
/// A shadow some mornings carry.
const DUST: (f32, f32, f32, f32) = (120.0, 70.0, 16.0, 0.03);

/// One morning's three flats on a camera, dusty or not.
fn session(
    dir: &Path,
    name: &str,
    camera: &str,
    captured: &str,
    dusty: bool,
    seed: u64,
) -> Vec<FrameMeta> {
    let dust: &[(f32, f32, f32, f32)] = if dusty { &[DUST] } else { &[] };
    (0..3)
        .map(|index| {
            let path = dir.join(format!("{name}-{index}.fits"));
            let plane = crate::calibration::flat_groups::testing::synthetic(
                SIZE.0,
                SIZE.1,
                dust,
                0.0,
                &[],
                0.0005,
                seed * 10 + index as u64,
            );
            let at = format!("{captured}:0{index}");
            write_frame_with(&path, "FLAT", SIZE, &at, &["ROTATANG= 0.0"], |x, y| {
                30_000.0 * f64::from(plane.data[y * SIZE.0 + x])
            });
            let mut meta = read_frame_meta(&path);
            meta.filter = Some("Ha".into());
            meta.rotator_position = Some(0.0);
            meta.camera = Some(camera.into());
            meta
        })
        .collect()
}

fn import(conn: &mut Connection, frames: &[FrameMeta]) {
    let tx = conn.transaction().unwrap();
    import_calibration_frames(&tx, frames, Some("profile")).unwrap();
    tx.commit().unwrap();
}

fn rig_of(conn: &Connection, camera: &str) -> String {
    conn.query_row(
        "SELECT rig_uuid FROM psf_guard_rig WHERE camera = ?1",
        [camera],
        |row| row.get(0),
    )
    .unwrap()
}

fn test_state() -> AppState {
    AppState::new_for_test(Connection::open_in_memory().unwrap())
}

/// Settings are per thread in tests; the lock keeps them from other tests
/// that change the process's.
fn settings_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::calibration::tests::POLICY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A catalog with two rigs, two mornings each: new dust on rig A's second.
struct Library {
    _temp: tempfile::TempDir,
    root: PathBuf,
    path: String,
    dir: PathBuf,
    conn: Connection,
    a: String,
    b: String,
}

fn library() -> Library {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let path = root.join("catalog.sqlite");
    let mut conn = Connection::open(&path).unwrap();
    let mut frames = session(&root, "a1", "CamA", "2026-09-10T14:00", false, 1);
    frames.extend(session(&root, "a2", "CamA", "2026-09-14T14:00", true, 2));
    frames.extend(session(&root, "b1", "CamB", "2026-09-10T14:30", false, 3));
    frames.extend(session(&root, "b2", "CamB", "2026-09-14T14:30", false, 4));
    import(&mut conn, &frames);
    store::add_tables(&conn).unwrap();
    let (a, b) = (rig_of(&conn, "CamA"), rig_of(&conn, "CamB"));
    assert_ne!(a, b);
    Library {
        dir: root.join("responses"),
        path: path.to_string_lossy().into_owned(),
        _temp: temp,
        root,
        conn,
        a,
        b,
    }
}

impl Library {
    fn pass(&self, state: &AppState, job: &Job, scope: Scope) -> PassOutcome {
        self.pass_until(state, job, scope, &Stop::new())
    }

    fn pass_until(&self, state: &AppState, job: &Job, scope: Scope, stop: &Stop) -> PassOutcome {
        run_with(state, &self.path, job, &scope, stop, &self.dir, &|_| None).unwrap()
    }

    fn measured_rigs(&self) -> Vec<String> {
        let mut rigs: Vec<String> = store::load_responses(&self.conn)
            .unwrap()
            .into_iter()
            .map(|row| row.rig_uuid)
            .collect();
        rigs.sort();
        rigs.dedup();
        rigs
    }
}

#[test]
fn requests_settle_into_one_pass() {
    let start = Instant::now();
    let mut queue = Queue::default();
    assert_eq!(queue.take_ready(start), Ready::Idle);
    // A burst of three imports, a second apart, each touching its rig.
    for (second, rig) in [(0, "a"), (1, "b"), (2, "a")] {
        queue.add(
            Scope::flats(Rigs::of([rig])),
            start + Duration::from_secs(second),
            Trigger::Change,
        );
    }
    queue.add(
        Scope::lights(Rigs::All),
        start + Duration::from_secs(2),
        Trigger::Change,
    );
    let last = start + Duration::from_secs(2);
    assert_eq!(
        queue.take_ready(last + Duration::from_secs(1)),
        Ready::Settle(FLAT_MEASURE_SETTLE - Duration::from_secs(1)),
        "still settling after the last request"
    );
    assert_eq!(
        queue.take_ready(last + FLAT_MEASURE_SETTLE),
        Ready::Run(Scope {
            flats: Rigs::of(["a", "b"]),
            lights: Rigs::All,
        }),
        "one pass for the burst"
    );
    assert_eq!(
        queue.take_ready(last + FLAT_MEASURE_SETTLE * 2),
        Ready::Idle
    );
}

#[test]
fn a_trigger_touches_only_its_rig_and_sessions() {
    let _settings = settings_lock();
    let mut library = library();
    let state = test_state();
    let job = Job::default();
    let first = library.pass(&state, &job, Scope::everything());
    assert_eq!((first.measured, first.compared), (4, 2));

    // New flats arrive for both rigs, but the import touched rig A only:
    // rig B's arrived behind the catalog's back, as from N.I.N.A.
    let before = rig_state(&library.conn);
    let root = library.root.clone();
    import(
        &mut library.conn,
        &session(&root, "a3", "CamA", "2026-09-18T14:00", true, 5),
    );
    let after = rig_state(&library.conn);
    let touched = changed_rigs(before.as_ref(), after.as_ref());
    assert_eq!(touched, Rigs::of([library.a.clone()]));
    import(
        &mut library.conn,
        &session(&root, "b3", "CamB", "2026-09-18T14:30", false, 6),
    );

    let outcome = library.pass(&state, &job, Scope::flats(touched));
    assert_eq!(outcome.measured, 1, "rig A's new morning alone");
    assert_eq!(outcome.frames_read, 3, "its three flats, nothing else");
    assert!(outcome.compared >= 1);
    let sessions_of = |rig: &str| {
        store::load_responses(&library.conn)
            .unwrap()
            .into_iter()
            .filter(|row| row.rig_uuid == rig)
            .count()
    };
    assert_eq!(sessions_of(&library.a), 3);
    assert_eq!(
        sessions_of(&library.b),
        2,
        "rig B waits for its own trigger"
    );

    // New lights alone ask only for evidence: no flat is read.
    let lights = library.pass(&state, &job, Scope::lights(Rigs::All));
    assert_eq!(
        (lights.measured, lights.compared, lights.frames_read),
        (0, 0, 0)
    );
    assert_eq!(sessions_of(&library.b), 2);
}

#[test]
fn forgetting_a_flat_measures_only_its_session_again() {
    let _settings = settings_lock();
    let mut library = library();
    let state = test_state();
    let job = Job::default();
    library.pass(&state, &job, Scope::everything());
    // A fourth flat on rig A's first morning, then forgotten again: the
    // session keeps three.
    let uuid: String = library
        .conn
        .query_row(
            "SELECT frame_uuid FROM psf_guard_calibration_frame WHERE source_path LIKE '%a2-0.fits'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    // As the route does: the frame's rig, read before it goes.
    let touched = rigs_of_frames(&library.conn, std::slice::from_ref(&uuid));
    crate::calibration::forget_frame(&mut library.conn, &uuid).unwrap();
    assert_eq!(touched, Rigs::of([library.a.clone()]));
    let outcome = library.pass(&state, &job, Scope::flats(touched));
    // Two flats still make a response: the session is measured again from
    // its two, and nothing else is read.
    assert_eq!((outcome.measured, outcome.frames_read), (1, 2));
    assert_eq!(library.measured_rigs().len(), 2);
}

#[test]
fn a_catalog_up_to_date_reads_no_pixels() {
    let _settings = settings_lock();
    let library = library();
    let state = test_state();
    let job = Job::default();
    let first = library.pass(&state, &job, Scope::everything());
    assert_eq!(first.frames_read, 12);
    // A restart asks for everything unfinished or stale: there is none.
    let job = Job::default();
    let start = library.pass(&state, &job, Scope::everything());
    assert_eq!(
        (
            start.measured,
            start.compared,
            start.evidence,
            start.frames_read
        ),
        (0, 0, 0, 0)
    );
    assert!(!job.status().active, "never shown as work");
}

#[test]
fn a_rig_grouped_by_time_is_not_measured_until_it_is_switched() {
    let _settings = settings_lock();
    let library = library();
    let state = test_state();
    let job = Job::default();
    let before = crate::calibration::rule_book();
    {
        let _time = crate::calibration::tests::RulesInForce::with(
            &[],
            &[(library.a.as_str(), "A", &[("flat_grouping", json!("time"))])],
        );
        let time = crate::calibration::rule_book();
        // Switching to time asks nothing of the job.
        assert_eq!(rigs_to_measure_after(&before, &time), Rigs::None);
        let outcome = library.pass(&state, &job, Scope::everything());
        assert_eq!(
            (outcome.measured, outcome.frames_read),
            (2, 6),
            "rig B only"
        );
        assert_eq!(library.measured_rigs(), std::slice::from_ref(&library.b));

        // Back to measured: rig A alone is queued, and measured.
        let _measured = crate::calibration::tests::RulesInForce::global(&[]);
        let measured = crate::calibration::rule_book();
        let rigs_now = rigs_to_measure_after(&time, &measured);
        assert_eq!(rigs_now, Rigs::of([library.a.clone()]));
        let outcome = library.pass(&state, &job, Scope::flats(rigs_now));
        assert_eq!((outcome.measured, outcome.frames_read), (2, 6));
    }
}

#[test]
fn a_grouping_limit_regroups_without_grading_or_reading() {
    let _settings = settings_lock();
    let library = library();
    let state = test_state();
    let job = Job::default();
    library.pass(&state, &job, Scope::everything());
    let book = flat_groups::load_book(&library.conn).unwrap();
    assert_eq!(book.events.len(), 1, "rig A's new dust splits its mornings");

    let before = crate::calibration::rule_book();
    let _loose =
        crate::calibration::tests::RulesInForce::global(&[("flat_split_dust_pct", json!(10.0))]);
    let after = crate::calibration::rule_book();
    assert!(!before.grades_differ(&after), "no regrade");
    // Regrouped from the stored rows, on read.
    let book = flat_groups::load_book(&library.conn).unwrap();
    assert!(book.events.is_empty(), "the same mornings, one group now");
    // A server-wide limit asks for every rig; the pass finds nothing to read.
    let rigs = rigs_to_measure_after(&before, &after);
    assert_eq!(rigs, Rigs::All);
    let outcome = library.pass(&state, &job, Scope::flats(rigs));
    assert_eq!(
        (outcome.measured, outcome.compared, outcome.frames_read),
        (0, 0, 0)
    );
}

#[test]
fn a_rig_whose_night_zone_changes_is_planned_again() {
    use crate::calibration::rules::{RigLayer, RuleBook};
    let auto = RuleBook::new(Default::default(), Default::default());
    let rig = |zone: &str| {
        RuleBook::new(
            Default::default(),
            [(
                "c925".to_string(),
                RigLayer {
                    name: None,
                    values: [("night_zone".to_string(), json!(zone))]
                        .into_iter()
                        .collect(),
                },
            )]
            .into_iter()
            .collect(),
        )
    };
    assert_eq!(rigs_to_measure_after(&auto, &rig("-8")), Rigs::of(["c925"]));
    assert_eq!(rigs_to_measure_after(&rig("-8"), &rig("-8")), Rigs::None);
    let global = RuleBook::new(
        [("night_zone".to_string(), json!("9"))]
            .into_iter()
            .collect(),
        Default::default(),
    );
    assert_eq!(rigs_to_measure_after(&auto, &global), Rigs::All);
}

/// A pass over rig A leaves rig B's stale rows for B's own pass, which
/// measures them again rather than finding them gone.
#[test]
fn a_pass_over_one_rig_leaves_another_rigs_rows() {
    let _settings = settings_lock();
    let library = library();
    let state = test_state();
    let job = Job::default();
    library.pass(&state, &job, Scope::everything());
    // One of rig B's files changed under the catalog: its session's key is
    // new, and its old row stale.
    library
        .conn
        .execute(
            "UPDATE psf_guard_calibration_frame SET source_fingerprint = 'changed'
             WHERE source_path LIKE '%b1-0.fits'",
            [],
        )
        .unwrap();
    let rows_of = |rig: &str| {
        store::load_responses(&library.conn)
            .unwrap()
            .into_iter()
            .filter(|row| row.rig_uuid == rig)
            .count()
    };
    let a = library.pass(&state, &job, Scope::flats(Rigs::of([library.a.clone()])));
    assert_eq!(a.frames_read, 0);
    assert_eq!(rows_of(&library.b), 2, "B's stale row waits for B's pass");
    let b = library.pass(&state, &job, Scope::flats(Rigs::of([library.b.clone()])));
    assert_eq!((b.measured, b.frames_read), (1, 3));
    assert_eq!(rows_of(&library.b), 2, "the stale row replaced");
}

// ---------------------------------------------------------------------------
// The runner: retries, pauses, grading, triggers.

#[test]
fn a_rig_left_unfinished_is_tried_again_later_and_less_often() {
    let start = Instant::now();
    let mut queue = Queue::default();
    assert!(queue.retry("a", start));
    assert_eq!(queue.take_ready(start), Ready::Settle(RETRY_FIRST));
    assert_eq!(
        queue.take_ready(start + RETRY_FIRST),
        Ready::Run(Scope::flats(Rigs::of(["a"]))),
        "due: it runs without waiting for a settle"
    );
    // Each try waits twice as long, up to the most.
    let mut waits = Vec::new();
    for attempt in 2..=RETRY_LIMIT {
        assert!(queue.retry("a", start));
        waits.push(queue.due["a"] - start);
        assert_eq!(backoff(attempt), waits[waits.len() - 1]);
    }
    assert_eq!(waits[0], RETRY_FIRST * 2);
    assert!(waits.windows(2).all(|pair| pair[1] >= pair[0]));
    assert!(waits.iter().all(|wait| *wait <= RETRY_MAX));
    assert_eq!(backoff(20), RETRY_MAX);
    // Then it waits for a change.
    assert!(!queue.retry("a", start), "past the limit");
    assert!(!queue.due.contains_key("a"));
    queue.add(Scope::flats(Rigs::of(["a"])), start, Trigger::Change);
    assert!(!queue.attempts.contains_key("a"), "a change starts again");
    assert!(queue.retry("a", start));
    assert_eq!(queue.due["a"] - start, RETRY_FIRST);
    // A pass that finishes it ends its tries.
    queue.finished("a");
    assert!(!queue.attempts.contains_key("a"));
}

#[test]
fn a_pass_waits_for_grading_without_pushing_its_settle_back() {
    let start = Instant::now();
    let mut queue = Queue::default();
    queue.add(Scope::flats(Rigs::of(["a"])), start, Trigger::Change);
    let ready = start + FLAT_MEASURE_SETTLE;
    assert_eq!(next_step(&mut queue, ready, true), Step::Sleep(POLL));
    assert!(!queue.pending.is_empty(), "back in line");
    // Grading done: it runs at once.
    assert_eq!(
        next_step(&mut queue, ready + POLL, false),
        Step::Run(Scope::flats(Rigs::of(["a"])))
    );
    assert_eq!(next_step(&mut queue, ready + POLL, false), Step::Exit);
    // A retry not yet due naps, at most a second at a time.
    queue.retry("a", ready);
    assert_eq!(next_step(&mut queue, ready, false), Step::Sleep(NAP_MAX));
}

/// Review of #870: flats that are away (an unmounted share) leave their
/// rig queued for another try, not dropped until a restart.
#[test]
fn an_unreadable_session_is_queued_again() {
    let _settings = settings_lock();
    let library = library();
    let away: Vec<(PathBuf, PathBuf)> = (0..3)
        .map(|i| {
            let file = library.root.join(format!("a2-{i}.fits"));
            (file.clone(), file.with_extension("away"))
        })
        .collect();
    for (file, moved) in &away {
        std::fs::rename(file, moved).unwrap();
    }
    let state = Arc::new(
        AppState::new(
            library.path.clone(),
            vec![library.root.to_string_lossy().into_owned()],
            library.root.join("cache").to_string_lossy().into_owned(),
            Default::default(),
        )
        .unwrap(),
    );
    let path = state.all_databases()[0].database_path.clone();
    queue(&state, &path, Scope::flats(Rigs::of([library.a.clone()])));
    let deadline = Instant::now() + Duration::from_secs(60);
    while status(&state, &path).finished_at.is_none() {
        assert!(Instant::now() < deadline, "the pass never ran");
        std::thread::sleep(Duration::from_millis(100));
    }
    let after = status(&state, &path);
    assert_eq!(after.last.as_ref().unwrap().unreadable, 1, "{after:?}");
    assert!(after.last.as_ref().unwrap().retry.contains(&library.a));
    // The share comes back; the rig waits for its next try.
    for (file, moved) in &away {
        std::fs::rename(moved, file).unwrap();
    }
    let now = status(&state, &path);
    assert_eq!(now.state, "queued", "{now:?}");
    assert_eq!(now.rigs, std::slice::from_ref(&library.a));
    assert!(now.retry_at.is_some());
    state.catalog_jobs.flat_measure.job(&path).pause();
}

/// A pause during a pass puts its scope back in line, and the pass after
/// Resume measures it all.
#[test]
fn a_paused_pass_goes_back_in_line_for_resume() {
    let _settings = settings_lock();
    let library = library();
    let state = test_state();
    let job = Job::default();
    let scope = Scope::everything();
    // The pause arrives as the pass starts.
    job.queue().running = Some(scope.clone());
    let paused = Stop::new();
    paused.request();
    let result = Ok(library.pass_until(&state, &job, scope.clone(), &paused));
    let outcome = result.as_ref().unwrap();
    assert!(outcome.stopped);
    assert_eq!(outcome.frames_read, 0, "nothing read after the pause");
    settle_outcome(&job, &scope, &result, Instant::now(), true);
    assert!(
        job.queue().attempts.is_empty(),
        "a pause is not a failed try"
    );
    let resumed = match job.queue().take_ready(Instant::now()) {
        Ready::Run(scope) => scope,
        other => panic!("{other:?}"),
    };
    assert_eq!(resumed, Scope::everything());
    let outcome = library.pass(&state, &job, resumed);
    assert_eq!((outcome.measured, outcome.stopped), (4, false));
}

#[test]
fn a_failed_runner_clears_the_running_pass() {
    let mut queue = Queue {
        running: Some(Scope::everything()),
        ..Queue::default()
    };
    let mut status = FlatMeasureStatus {
        active: true,
        ..FlatMeasureStatus::default()
    };
    FlatMeasure::runner_failed(&mut queue, &mut status);
    assert!(queue.running.is_none());
    assert!(!status.active);
    assert!(status.last_error.is_some());
}

/// A forget through its route queues the forgotten frame's rig alone.
#[tokio::test]
async fn forgetting_through_the_route_queues_its_rig() {
    let library = library();
    let state = Arc::new(
        AppState::new(
            library.path.clone(),
            vec![library.root.to_string_lossy().into_owned()],
            library.root.join("cache").to_string_lossy().into_owned(),
            Default::default(),
        )
        .unwrap(),
    );
    let ctx = state.all_databases()[0].clone();
    let path = ctx.database_path.clone();
    let uuid: String = library
        .conn
        .query_row(
            "SELECT frame_uuid FROM psf_guard_calibration_frame WHERE source_path LIKE '%b2-0.fits'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let _ = crate::server::handlers::forget_calibration_frames(
        State(Arc::clone(&state)),
        DbContext(ctx),
        Json(crate::server::handlers::ForgetCalibrationFramesRequest {
            frame_uuids: vec![uuid],
        }),
    )
    .await
    .unwrap();
    let queued = status(&state, &path);
    assert_eq!(queued.rigs, std::slice::from_ref(&library.b), "{queued:?}");
    state.catalog_jobs.flat_measure.job(&path).pause();
}

// ---------------------------------------------------------------------------
// Tiers: a page lifts a rig's pass without holding up the person.

/// Review of #870: a pass a page lifts borrows from the background share,
/// step by step, and holds no interactive guard, so a stack the person
/// starts meanwhile plans with its whole share and nothing waits for the
/// pass.
#[test]
fn a_viewed_pass_leaves_a_persons_stack_its_share() {
    let state = test_state();
    let path = "/viewed/catalog.sqlite";
    let job = job(&state, path);
    watch_at(&mut job.queue(), "rig-a", Instant::now());
    assert!(watched(&state, path, "rig-a"));
    assert!(
        !watched(&state, path, "rig-b"),
        "another rig stays in the background"
    );
    let workers = PriorityWorkers::new(&state, "rig-a", None, &job);
    let stack_share =
        crate::concurrency::plan_workers(None, &state.worker_policy(), Priority::Interactive, None)
            .workers;
    let mut seen = None;
    workers.run(&mut || {
        let stack = state.lease_workers(Priority::Interactive, stack_share);
        seen = Some((stack.workers, state.interactive_job_active()));
    });
    assert_eq!(
        seen,
        Some((stack_share, false)),
        "the stack's whole share, and no guard"
    );
    assert_eq!(job.status().priority.as_deref(), Some("viewed"));
    // Between steps the pass holds nothing.
    assert_eq!(state.leased_workers(Priority::Viewed), 0);

    // The page goes: the next step is background again.
    watch_at(
        &mut job.queue(),
        "rig-a",
        Instant::now() - WATCH_TTL - Duration::from_secs(1),
    );
    assert!(!watched(&state, path, "rig-a"));
    workers.run(&mut || {});
    assert_eq!(job.status().priority.as_deref(), Some("background"));
}

/// Both tiers wait for interactive work a person started.
#[test]
fn a_pass_waits_for_the_persons_work_at_either_tier() {
    for watch_it in [false, true] {
        let state = Arc::new(test_state());
        let path = if watch_it {
            "/waits/viewed.sqlite"
        } else {
            "/waits/background.sqlite"
        };
        let job = job(&state, path);
        if watch_it {
            watch_at(&mut job.queue(), "rig", Instant::now());
        }
        let preview = state.begin_named_interactive_job("a preview");
        let waiter = {
            let (state, job) = (Arc::clone(&state), Arc::clone(&job));
            std::thread::spawn(move || {
                PriorityWorkers::new(&state, "rig", None, &job).yield_now();
            })
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while job.status().waiting_for.is_none() {
            assert!(Instant::now() < deadline, "the pass never waited");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!waiter.is_finished(), "the person's work comes first");
        assert_eq!(job.status().waiting_for.as_deref(), Some("a preview"));
        drop(preview);
        waiter.join().unwrap();
        assert_eq!(job.status().waiting_for, None);
    }
}

/// Rigs whose frames found their sites are graded again and measured
/// again; none found asks for nothing.
#[test]
fn rigs_that_find_their_sites_are_graded_and_planned_again() {
    let library = library();
    let state = Arc::new(test_state());
    let path = library.path.as_str();
    sites_found(&state, path, Vec::new());
    assert!(job(&state, path).queue().pending.is_empty());
    assert!(!crate::server::calibration_quality::progress(&state, path).running);
    sites_found(&state, path, vec![library.a.clone()]);
    assert_eq!(
        job(&state, path).queue().pending,
        Scope::flats(Rigs::of([library.a.clone()]))
    );
    // Grading ran, or is running, for the catalog.
    let started = std::time::Instant::now();
    while crate::server::calibration_quality::progress(&state, path)
        .finished_at
        .is_none()
    {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "grading never ran"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    job(&state, path).queue().pending = Scope::default();
}

/// A page's look queues a check of its rig at most every
/// [`WATCH_RECHECK`], none while the job is paused, and none for a rig the
/// catalog does not record.
#[test]
fn a_page_looking_at_a_rig_asks_for_a_check_now_and_then() {
    let library = library();
    let state = Arc::new(test_state());
    let path = library.path.as_str();
    // The job is paused: a look queues nothing, though it lifts the rig.
    assert!(job(&state, path).pause());
    watch(&state, path, &library.b);
    assert!(job(&state, path).queue().pending.is_empty());
    assert!(watched(&state, path, &library.b));
    assert!(state.catalog_jobs.flat_measure.resume(&state, path));
    // Rig A is idle: one check, then none until WATCH_RECHECK.
    watch(&state, path, &library.a);
    let first = job(&state, path).queue().pending.clone();
    assert_eq!(first, Scope::flats(Rigs::of([library.a.clone()])));
    job(&state, path).queue().pending = Scope::default();
    watch(&state, path, &library.a);
    assert!(job(&state, path).queue().pending.is_empty(), "rate-limited");
    // A rig the catalog does not record: nothing at all.
    watch(&state, path, "no-such-rig");
    assert!(job(&state, path).queue().pending.is_empty());
    assert!(!watched(&state, path, "no-such-rig"));
    let job = job(&state, path);
    let queue = job.queue();
    assert!(queue.looks.contains_key(&library.a));
    assert!(
        !queue.looks.contains_key(&library.b),
        "a look that queued nothing is not counted"
    );
    drop(queue);
    state.catalog_jobs.flat_measure.job(path).pause();
}

#[test]
fn every_rig_counts_as_changed_when_a_state_cannot_be_read() {
    let state = RigState::from([("a".to_string(), "1".to_string())]);
    assert_eq!(changed_rigs(None, Some(&state)), Rigs::All);
    assert_eq!(changed_rigs(Some(&state), Some(&state)), Rigs::None);
    let gone = RigState::new();
    assert_eq!(changed_rigs(Some(&state), Some(&gone)), Rigs::of(["a"]));
}

/// Requests that come while the job is paused wait for Resume, and the
/// status lists their rigs.
#[test]
fn a_pause_holds_requests_until_resume() {
    let state = Arc::new(test_state());
    let path = "/paused/catalog.sqlite";
    let job = job(&state, path);
    assert!(job.pause());
    queue(&state, path, Scope::flats(Rigs::of(["a"])));
    assert!(!job.queue().has_runner(), "the request waits for Resume");
    let paused = status(&state, path);
    assert_eq!(paused.state, "paused");
    assert_eq!(paused.rigs, ["a"]);
    assert!(state.catalog_jobs.flat_measure.resume(&state, path));
    assert!(job.queue().has_runner());
    assert_eq!(status(&state, path).state, "queued");
    job.pause();
}

// ---------------------------------------------------------------------------
// Second review of #870.

/// Excluding every flat of a rig (the use/exclude choice) leaves it no
/// plan; its responses still go, as the old catalog-wide retain dropped
/// them.
#[test]
fn excluding_every_flat_of_a_rig_drops_its_responses() {
    let _settings = settings_lock();
    let library = library();
    let state = test_state();
    let job = Job::default();
    library.pass(&state, &job, Scope::everything());
    let uuids: Vec<String> = {
        let mut statement = library
            .conn
            .prepare("SELECT frame_uuid FROM psf_guard_calibration_frame WHERE rig_uuid = ?1")
            .unwrap();
        statement
            .query_map([&library.b], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let touched = rigs_of_frames(&library.conn, &uuids);
    crate::calibration::set_overrides(
        &library.conn,
        &uuids,
        Some(crate::calibration::QualityOverride::Exclude),
    )
    .unwrap();
    library.pass(&state, &job, Scope::flats(touched));
    assert_eq!(
        library.measured_rigs(),
        std::slice::from_ref(&library.a),
        "rig B's flats are all kept out, yet its responses stay"
    );
}

/// Rig ids a page posts that the catalog does not record leave no trace,
/// and old looks are pruned.
#[test]
fn looks_are_pruned() {
    let state = Arc::new(test_state());
    let path = "/review2/looks.sqlite";
    for index in 0..500 {
        watch(&state, path, &format!("rig-{index}"));
    }
    state.catalog_jobs.flat_measure.job(path).pause();
    let kept = job(&state, path).queue().looks.len();
    assert!(kept < 500, "{kept} looks kept");
}

/// A pass that ends in an error (as a busy catalog) tries its rigs again;
/// one a pause ended puts them back in line for Resume instead.
#[test]
fn a_pass_that_fails_is_tried_again() {
    let now = Instant::now();
    let job = Job::default();
    let scope = Scope::flats(Rigs::of(["a", "b"]));
    job.queue().running = Some(scope.clone());
    settle_outcome(
        &job,
        &scope,
        &Err(anyhow::anyhow!("database is locked")),
        now,
        false,
    );
    {
        let queue = job.queue();
        assert!(queue.running.is_none());
        assert_eq!(queue.due["a"], now + RETRY_FIRST);
        assert_eq!(queue.due["b"], now + RETRY_FIRST);
    }
    // Every rig's pass failing tries every rig again.
    let job = Job::default();
    settle_outcome(
        &job,
        &Scope::everything(),
        &Err(anyhow::anyhow!("busy")),
        now,
        false,
    );
    assert!(job.queue().due.contains_key(EVERY_RIG));
    // Ended by a pause: back in line, not retried.
    let job = Job::default();
    settle_outcome(&job, &scope, &Err(anyhow::anyhow!("busy")), now, true);
    assert!(job.queue().due.is_empty());
    assert_eq!(job.queue().pending, scope);
}

/// The rig a page shows is measured first.
#[test]
fn the_rig_a_page_shows_goes_first() {
    let _settings = settings_lock();
    let library = library();
    // The rig that would come last in order.
    let last = library.a.clone().max(library.b.clone());
    let mut order: Vec<String> = Vec::new();
    flat_groups::job::run(flat_groups::job::Pass {
        conn: &library.conn,
        dir: &library.dir,
        scope: Scope::everything(),
        keep_going: &|| true,
        yield_now: &|| {},
        progress: &mut |at| {
            if order.last().map(String::as_str) != Some(at.rig) {
                order.push(at.rig.to_string());
            }
        },
        goes_first: &|rig| rig == last,
        find_light: &|_| None,
        pool: None,
    })
    .unwrap();
    assert_eq!(order.first(), Some(&last), "{order:?}");
    assert_eq!(order.len(), 2);
}

/// A pass says how many rigs it has left after the one it works on.
#[test]
fn a_pass_counts_the_rigs_after_this_one() {
    let _settings = settings_lock();
    let library = library();
    let mut after: Vec<(String, usize)> = Vec::new();
    flat_groups::job::run(flat_groups::job::Pass {
        conn: &library.conn,
        dir: &library.dir,
        scope: Scope::everything(),
        keep_going: &|| true,
        yield_now: &|| {},
        progress: &mut |at| {
            if after.last().map(|(rig, _)| rig.as_str()) != Some(at.rig) {
                after.push((at.rig.to_string(), at.rigs_after));
            }
        },
        goes_first: &|_| false,
        find_light: &|_| None,
        pool: None,
    })
    .unwrap();
    let counts: Vec<usize> = after.iter().map(|(_, count)| *count).collect();
    assert_eq!(counts, [1, 0], "{after:?}");
}

/// While a pass runs, what was asked for since is listed for the next pass,
/// apart from the pass itself.
#[test]
fn the_status_lists_what_waits_for_the_next_pass() {
    let state = test_state();
    let path = "/next-pass/catalog.sqlite";
    let job = job(&state, path);
    {
        let mut queue = job.queue();
        queue.add(
            Scope::flats(Rigs::of(["b"])),
            Instant::now(),
            Trigger::Change,
        );
    }
    // Nothing runs: the waiting rig is simply queued.
    let waiting = status(&state, path);
    assert_eq!(waiting.state, "queued");
    assert!(waiting.next_pass.is_empty());
    job.queue().running = Some(Scope::flats(Rigs::of(["a"])));
    let running = status(&state, path);
    assert_eq!(running.rigs, ["a", "b"]);
    assert_eq!(running.next_pass, ["b"]);
    // A retry due by now joins the next pass; one due later does not.
    {
        let mut queue = job.queue();
        let now = Instant::now();
        queue.due.insert("c".into(), now - Duration::from_secs(1));
        queue.due.insert("d".into(), now + Duration::from_secs(600));
    }
    let retrying = status(&state, path);
    assert_eq!(retrying.next_pass, ["b", "c"]);
    assert_eq!(retrying.rigs, ["a", "b", "c", "d"]);
    let mut queue = job.queue();
    queue.running = None;
    queue.pending = Scope::default();
    queue.due.clear();
}

/// A use/exclude choice through its route queues the frames' rig alone.
#[tokio::test]
async fn a_keep_out_choice_through_the_route_queues_its_rig() {
    let library = library();
    let state = Arc::new(
        AppState::new(
            library.path.clone(),
            vec![library.root.to_string_lossy().into_owned()],
            library.root.join("cache").to_string_lossy().into_owned(),
            Default::default(),
        )
        .unwrap(),
    );
    let ctx = state.all_databases()[0].clone();
    let path = ctx.database_path.clone();
    let uuid: String = library
        .conn
        .query_row(
            "SELECT frame_uuid FROM psf_guard_calibration_frame WHERE source_path LIKE '%a1-0.fits'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let _ = crate::server::handlers::set_calibration_quality_override(
        State(Arc::clone(&state)),
        DbContext(ctx),
        Json(
            crate::server::handlers::SetCalibrationQualityOverrideRequest {
                frame_uuids: vec![uuid],
                choice: "exclude".into(),
            },
        ),
    )
    .await
    .unwrap();
    let queued = status(&state, &path);
    assert_eq!(queued.rigs, std::slice::from_ref(&library.a), "{queued:?}");
    state.catalog_jobs.flat_measure.job(&path).pause();
}

/// A session's dust is measured with the pedestal taken out: a bias that
/// arrives later moves the pedestal, and the dust is measured again.
#[test]
fn a_new_pedestal_measures_the_session_dust_again() {
    let _settings = settings_lock();
    let mut library = library();
    let state = test_state();
    let job = Job::default();
    library.pass(&state, &job, Scope::everything());
    let keys: Vec<String> = store::load_responses(&library.conn)
        .unwrap()
        .into_iter()
        .filter(|row| row.rig_uuid == library.a)
        .map(|row| row.response_key)
        .collect();
    assert!(!keys.is_empty());
    let dust = |key: &str| {
        std::fs::read(crate::calibration::flat_groups::dust_path(
            &library.dir,
            key,
        ))
        .unwrap()
    };
    let before: Vec<Vec<u8>> = keys.iter().map(|key| dust(key)).collect();

    // A bias for rig A at 3,000 ADU, measured.
    let path = library.root.join("bias-a.fits");
    write_frame_with(&path, "BIAS", SIZE, "2026-09-12T14:00:00", &[], |_, _| {
        3000.0
    });
    let mut meta = read_frame_meta(&path);
    meta.camera = Some("CamA".into());
    import(&mut library.conn, &[meta]);
    let bias: String = library
        .conn
        .query_row(
            "SELECT frame_uuid FROM psf_guard_calibration_frame WHERE kind = 'bias'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    library
        .conn
        .execute(
            "INSERT OR REPLACE INTO psf_guard_calibration_frame_quality
                (frame_uuid, algorithm_version, measurement_json, measured_at, updated_at)
             VALUES (?1, 1, '{\"level\": 3000.0}', 0, 0)",
            [&bias],
        )
        .unwrap();
    library.pass(&state, &Job::default(), Scope::everything());
    let rows = store::load_responses(&library.conn).unwrap();
    let kept: Vec<Vec<u8>> = keys.iter().map(|key| dust(key)).collect();
    // What a pass measures with the new pedestal, from scratch.
    for key in &keys {
        let row = rows.iter().find(|row| &row.response_key == key).unwrap();
        assert_eq!(row.pedestal_adu, Some(3000.0), "the bias sets the pedestal");
        std::fs::remove_file(crate::calibration::flat_groups::dust_path(
            &library.dir,
            key,
        ))
        .unwrap();
    }
    library.pass(&state, &Job::default(), Scope::everything());
    for ((key, before), kept) in keys.iter().zip(&before).zip(&kept) {
        let fresh = dust(key);
        assert_ne!(&fresh, before, "the new pedestal changes the dust of {key}");
        assert_eq!(kept, &fresh, "the dust of {key} kept the old pedestal");
    }
}

/// A session whose flats combine but whose dust cannot be found (here, a
/// plane under Seiza's 32-px floor) still gets a dust record, so no pass
/// reads its flats again.
#[test]
fn a_session_without_dust_is_not_read_every_pass() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let path = root.join("catalog.sqlite");
    let mut conn = Connection::open(&path).unwrap();
    let frames: Vec<FrameMeta> = (0..3)
        .map(|index| {
            let file = root.join(format!("small-{index}.fits"));
            write_frame_with(
                &file,
                "FLAT",
                (24, 16),
                &format!("2026-09-10T14:00:0{index}"),
                &["ROTATANG= 0.0"],
                |x, y| 30_000.0 + (x + y + index) as f64,
            );
            let mut meta = read_frame_meta(&file);
            meta.filter = Some("Ha".into());
            meta.rotator_position = Some(0.0);
            meta.camera = Some("Small".into());
            meta
        })
        .collect();
    import(&mut conn, &frames);
    store::add_tables(&conn).unwrap();
    let dir = root.join("responses");
    let state = test_state();
    let catalog = path.to_string_lossy().into_owned();
    let pass = || {
        run_with(
            &state,
            &catalog,
            &Job::default(),
            &Scope::everything(),
            &Stop::new(),
            &dir,
            &|_| None,
        )
        .unwrap()
    };
    let first = pass();
    assert_eq!(first.measured, 1, "the session combines: {first:?}");
    let second = pass();
    assert_eq!(second.frames_read, 0, "read again: {second:?}");
}

/// Upgrading a catalog whose responses predate session dust reads no flat
/// again: the dust comes from each session's response plane, and in how
/// many flats it shows from the flats' own stored measurements.
#[test]
fn session_dust_reads_no_flat_once_the_flats_are_measured() {
    let _settings = settings_lock();
    let library = library();
    let state = test_state();
    let first = library.pass(&state, &Job::default(), Scope::everything());
    assert!(first.frames_read > 0, "the responses are built: {first:?}");
    let keys: Vec<String> = store::load_responses(&library.conn)
        .unwrap()
        .into_iter()
        .map(|row| row.response_key)
        .collect();
    let dust = |key: &str| crate::calibration::flat_groups::read_dust(&library.dir, key).unwrap();
    // Before the flats are graded, the stack's dust stands alone.
    assert!(keys.iter().all(|key| !dust(key).persistence));

    crate::calibration::measure_and_grade(&library.conn).unwrap();
    // As after an upgrade: responses on disk, no dust beside them.
    for key in &keys {
        std::fs::remove_file(crate::calibration::flat_groups::dust_path(
            &library.dir,
            key,
        ))
        .unwrap();
    }
    let upgrade = library.pass(&state, &Job::default(), Scope::everything());
    assert_eq!(upgrade.frames_read, 0, "{upgrade:?}");
    for key in &keys {
        let dust = dust(key);
        assert!(dust.persistence, "{key}");
        assert_eq!(dust.frames.len(), 3, "{key}");
        for feature in &dust.map.features {
            assert_eq!(feature.seen_in.map(|seen| seen.of), Some(3), "{feature:?}");
        }
    }
    // The dusty morning's shadow shows in each of its flats.
    let dusty = keys
        .iter()
        .map(|key| dust(key))
        .find(|dust| !dust.map.features.is_empty())
        .expect("the dusty morning holds dust");
    assert!(
        dusty.frames.iter().all(|frame| frame.seen.contains(&0)),
        "{dusty:?}"
    );
    let again = library.pass(&state, &Job::default(), Scope::everything());
    assert_eq!(again.frames_read, 0, "{again:?}");
}
