use psf_guard_director_core::{Decision, Request, Safety};
use psf_guard_director_ledger::{Error, Evidence, Ledger, Reservation};
use rusqlite::Connection;
use std::{
    path::Path,
    process::Command,
    sync::{Arc, Barrier},
};
use tempfile::TempDir;

fn request() -> Request {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../director-core/tests/fixtures/decisions.json"
    ))
    .unwrap();
    let mut request: Request = serde_json::from_value(fixture["base"].clone()).unwrap();
    request.assignment.revision = 9_007_199_254_740_993;
    request.assignment.goals.truncate(1);
    let goal = &mut request.assignment.goals[0];
    goal.requested = 1;
    goal.accepted = 0;
    goal.pending = 0;
    goal.attempts_remaining = 2;
    request
}

fn open(path: &Path) -> Ledger {
    let r = request();
    Ledger::open(path, r.assignment, r.state).unwrap()
}

fn saved(image: &str) -> Evidence {
    Evidence::Saved {
        image_id: image.into(),
        elapsed_ms: 1500,
    }
}

fn created(ledger: &mut Ledger, id: &str) {
    assert!(matches!(
        ledger.reserve(id, request().state).unwrap(),
        Reservation::Created(_)
    ));
}

#[test]
fn restart_requires_recovery_and_never_replays_a_reservation() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    created(&mut ledger, "capture-1");
    drop(ledger);
    let mut ledger = open(&path);
    assert!(matches!(
        ledger.reserve("capture-1", request().state).unwrap(),
        Reservation::Existing(_)
    ));
    assert!(matches!(
        ledger.reserve("capture-2", request().state).unwrap(),
        Reservation::RecoveryRequired(_)
    ));
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 1);
}

#[test]
fn saved_images_are_pending_not_accepted_and_events_are_idempotent() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    created(&mut ledger, "capture-1");
    ledger.record("capture-1", saved("image-1")).unwrap();
    ledger.record("capture-1", saved("image-1")).unwrap();
    drop(ledger);
    let mut ledger = open(&path);
    assert_eq!(
        ledger.reserve("capture-2", request().state).unwrap(),
        Reservation::Decision(Decision::Wait {
            reason: "pending_assessment".into()
        })
    );
    let events = ledger.events_after(0, 256).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].sequence, 1);
    assert_eq!(events[1].sequence, 2);
    assert_eq!(events[1].assignment_revision, 9_007_199_254_740_993);
    assert_eq!(events[1].attempt.evidence, saved("image-1"));
    assert_eq!(ledger.events_after(1, 1).unwrap(), events[1..]);
    assert!(ledger.events_after(2, 1).unwrap().is_empty());
}

#[test]
fn uncertain_save_blocks_other_goals_and_late_receipt_resolves_it() {
    let dir = TempDir::new().unwrap();
    let mut r = request();
    let mut other = r.assignment.goals[0].clone();
    other.id = "other-goal".into();
    other.priority = 0;
    r.assignment.goals.push(other);
    let mut ledger =
        Ledger::open(&dir.path().join("execution.sqlite"), r.assignment, r.state).unwrap();
    created(&mut ledger, "capture-1");
    let uncertain = Evidence::Uncertain {
        reason: "save_timeout".into(),
    };
    ledger.record("capture-1", uncertain.clone()).unwrap();
    ledger.record("capture-1", uncertain).unwrap();
    assert!(matches!(
        ledger.reserve("capture-2", request().state).unwrap(),
        Reservation::RecoveryRequired(_)
    ));
    assert!(matches!(
        ledger.record(
            "capture-1",
            Evidence::Failed {
                reason: "timeout".into()
            }
        ),
        Err(Error::ConflictingEvidence)
    ));
    ledger.record("capture-1", saved("image-1")).unwrap();
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 3);
    let Reservation::Created(next) = ledger.reserve("capture-2", request().state).unwrap() else {
        panic!("expected unblocked other goal");
    };
    assert_eq!(next.goal_id, "other-goal");
}

#[test]
fn failures_consume_attempts_without_refunds() {
    let dir = TempDir::new().unwrap();
    let mut ledger = open(&dir.path().join("execution.sqlite"));
    for id in ["capture-1", "capture-2"] {
        created(&mut ledger, id);
        ledger
            .record(
                id,
                Evidence::Failed {
                    reason: "camera_refused".into(),
                },
            )
            .unwrap();
    }
    assert_eq!(
        ledger.reserve("capture-3", request().state).unwrap(),
        Reservation::Decision(Decision::CheckIn {
            reason: "no_authorized_feasible_work".into()
        })
    );
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 4);
}

#[test]
fn terminal_results_cannot_be_overwritten_or_capture_ids_reused() {
    let dir = TempDir::new().unwrap();
    let mut ledger = open(&dir.path().join("execution.sqlite"));
    created(&mut ledger, "capture-1");
    ledger.record("capture-1", saved("image-1")).unwrap();
    for evidence in [
        saved("image-2"),
        Evidence::Failed {
            reason: "failed".into(),
        },
        Evidence::Uncertain {
            reason: "unknown".into(),
        },
    ] {
        assert!(matches!(
            ledger.record("capture-1", evidence),
            Err(Error::ConflictingEvidence)
        ));
    }
    assert!(matches!(
        ledger.reserve("capture-1", request().state).unwrap(),
        Reservation::Existing(_)
    ));
    assert_eq!(
        ledger.attempt("capture-1").unwrap().unwrap().evidence,
        saved("image-1")
    );
}

#[test]
fn one_saved_image_cannot_credit_two_captures() {
    let dir = TempDir::new().unwrap();
    let r = request();
    let mut assignment = r.assignment;
    assignment.goals[0].requested = 2;
    let mut ledger =
        Ledger::open(&dir.path().join("execution.sqlite"), assignment, r.state).unwrap();
    created(&mut ledger, "capture-1");
    ledger.record("capture-1", saved("image-1")).unwrap();
    created(&mut ledger, "capture-2");
    assert!(matches!(
        ledger.record("capture-2", saved("image-1")),
        Err(Error::Sqlite(_))
    ));
    assert_eq!(
        ledger.attempt("capture-2").unwrap().unwrap().evidence,
        Evidence::Reserved
    );
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 3);
}

#[test]
fn foreign_and_future_databases_are_not_repurposed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("scheduler.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE acquiredimage(id INTEGER); INSERT INTO acquiredimage VALUES(42);",
        )
        .unwrap();
    let r = request();
    assert!(matches!(
        Ledger::open(&path, r.assignment, r.state),
        Err(Error::ForeignDatabase)
    ));
    assert_eq!(
        connection
            .query_row("SELECT id FROM acquiredimage", [], |r| r.get::<_, i32>(0))
            .unwrap(),
        42
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "application_id", |r| r.get::<_, i32>(0))
            .unwrap(),
        0
    );
    let path = dir.path().join("future.sqlite");
    drop(open(&path));
    let connection = Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    let r = request();
    assert!(matches!(
        Ledger::open(&path, r.assignment, r.state),
        Err(Error::UnsupportedSchema)
    ));
}

#[test]
fn revision_baseline_and_rig_changes_cannot_reset_budgets() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    created(&mut ledger, "capture-1");
    drop(ledger);
    for change in 0..5 {
        let mut r = request();
        match change {
            0 => r.assignment.revision += 1,
            1 => r.assignment.goals[0].pending += 1,
            2 => r.assignment.goals[0].attempts_remaining += 1,
            3 => r.assignment.rig_id = "other-rig".into(),
            _ => r.assignment.configuration_id = "other-config".into(),
        }
        assert!(matches!(
            Ledger::open(&path, r.assignment, r.state),
            Err(Error::AssignmentMismatch)
        ));
    }
    assert!(matches!(
        open(&path).reserve("capture-2", request().state).unwrap(),
        Reservation::RecoveryRequired(_)
    ));
}

#[test]
fn latest_state_and_shared_core_policy_gate_every_new_reservation() {
    let dir = TempDir::new().unwrap();
    let mut ledger = open(&dir.path().join("execution.sqlite"));
    for change in 0..6 {
        let mut state = request().state;
        match change {
            0 => state.safety = Safety::Unsafe,
            1 => state.operator_stop = true,
            2 => state.now_ms = 1_000_001,
            3 => state.conditions_valid_until_ms = 1,
            4 => state.rig_id = "other-rig".into(),
            _ => state.at_boundary = false,
        }
        assert!(matches!(
            ledger.reserve("capture-1", state).unwrap(),
            Reservation::Decision(_)
        ));
    }
    assert!(ledger.events_after(0, 256).unwrap().is_empty());
    created(&mut ledger, "capture-1");
}

#[test]
fn concurrent_handles_commit_only_one_dispatch_candidate() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let first = open(&path);
    let second = open(&path);
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = [first, second]
        .into_iter()
        .enumerate()
        .map(|(i, mut ledger)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                ledger
                    .reserve(&format!("capture-{i}"), request().state)
                    .unwrap()
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Reservation::Created(_)))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Reservation::RecoveryRequired(_)))
            .count(),
        1
    );
    assert_eq!(open(&path).events_after(0, 256).unwrap().len(), 1);
}

#[test]
fn event_write_failure_rolls_back_attempt_and_result() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    let connection = Connection::open(&path).unwrap();
    let fail = "CREATE TRIGGER fail_event BEFORE INSERT ON event BEGIN SELECT RAISE(ABORT,'injected'); END;";
    connection.execute_batch(fail).unwrap();
    assert!(matches!(
        ledger.reserve("capture-1", request().state),
        Err(Error::Sqlite(_))
    ));
    assert!(ledger.attempt("capture-1").unwrap().is_none());
    connection
        .execute_batch("DROP TRIGGER fail_event;")
        .unwrap();
    created(&mut ledger, "capture-1");
    connection.execute_batch(fail).unwrap();
    assert!(matches!(
        ledger.record("capture-1", saved("image-1")),
        Err(Error::Sqlite(_))
    ));
    assert_eq!(
        ledger.attempt("capture-1").unwrap().unwrap().evidence,
        Evidence::Reserved
    );
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 1);
}

#[test]
fn contention_returns_an_error_without_reserving_work() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(matches!(
        ledger.reserve("capture-1", request().state),
        Err(Error::Sqlite(_))
    ));
    connection.execute_batch("ROLLBACK").unwrap();
    assert!(ledger.attempt("capture-1").unwrap().is_none());
    created(&mut ledger, "capture-1");
}

#[test]
fn bounds_and_invalid_evidence_do_not_mutate_ledger() {
    let dir = TempDir::new().unwrap();
    let mut ledger = open(&dir.path().join("execution.sqlite"));
    for id in ["", "has space", &"x".repeat(129)] {
        assert!(matches!(
            ledger.reserve(id, request().state),
            Err(Error::InvalidInput)
        ));
    }
    assert!(matches!(
        ledger.events_after(0, 0),
        Err(Error::InvalidInput)
    ));
    assert!(matches!(
        ledger.events_after(0, 257),
        Err(Error::InvalidInput)
    ));
    assert!(matches!(
        ledger.events_after(u64::MAX, 1),
        Err(Error::InvalidInput)
    ));
    assert!(matches!(
        ledger.record("missing", saved("image-1")),
        Err(Error::UnknownCapture)
    ));
    created(&mut ledger, "capture-1");
    for evidence in [
        Evidence::Reserved,
        saved(""),
        Evidence::Failed { reason: "".into() },
        Evidence::Uncertain {
            reason: "x".repeat(129),
        },
    ] {
        assert!(matches!(
            ledger.record("capture-1", evidence),
            Err(Error::InvalidInput)
        ));
    }
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 1);
}

#[test]
fn baseline_progress_is_preserved_and_local_saved_counts_are_added_once() {
    let dir = TempDir::new().unwrap();
    let mut r = request();
    r.assignment.goals[0].requested = 4;
    r.assignment.goals[0].accepted = 2;
    r.assignment.goals[0].pending = 1;
    let mut ledger =
        Ledger::open(&dir.path().join("execution.sqlite"), r.assignment, r.state).unwrap();
    created(&mut ledger, "capture-1");
    ledger.record("capture-1", saved("image-1")).unwrap();
    assert_eq!(
        ledger.reserve("capture-2", request().state).unwrap(),
        Reservation::Decision(Decision::Wait {
            reason: "pending_assessment".into()
        })
    );
}

#[test]
fn abrupt_process_exit_preserves_committed_reservation() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_writer_child", "--nocapture"])
        .env("DIRECTOR_LEDGER_CRASH_TEST", &path)
        .status()
        .unwrap();
    assert_eq!(result.code(), Some(71));
    let mut ledger = open(&path);
    assert!(matches!(
        ledger.reserve("capture-1", request().state).unwrap(),
        Reservation::Existing(_)
    ));
    assert!(matches!(
        ledger.reserve("capture-2", request().state).unwrap(),
        Reservation::RecoveryRequired(_)
    ));
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 1);
}

#[test]
fn crash_writer_child() {
    let Some(path) = std::env::var_os("DIRECTOR_LEDGER_CRASH_TEST") else {
        return;
    };
    let mut ledger = open(Path::new(&path));
    created(&mut ledger, "capture-1");
    match std::env::var("DIRECTOR_LEDGER_CRASH_PHASE").as_deref() {
        Ok("saved") => {
            ledger.record("capture-1", saved("image-1")).unwrap();
        }
        Ok("uncommitted") => {
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch("BEGIN IMMEDIATE; UPDATE attempt SET payload='unfinished'; INSERT INTO event(payload) VALUES('unfinished');").unwrap();
            std::process::exit(71);
        }
        _ => {}
    }
    // Deliberately skip Rust/SQLite destructors, like a terminated sidecar.
    std::process::exit(71);
}

#[test]
fn abrupt_exit_preserves_saved_receipt_and_rolls_back_partial_transactions() {
    for phase in ["saved", "uncommitted"] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("execution.sqlite");
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_writer_child", "--nocapture"])
            .env("DIRECTOR_LEDGER_CRASH_TEST", &path)
            .env("DIRECTOR_LEDGER_CRASH_PHASE", phase)
            .status()
            .unwrap();
        assert_eq!(result.code(), Some(71));
        let mut ledger = open(&path);
        let result = ledger.reserve("capture-2", request().state).unwrap();
        if phase == "saved" {
            assert!(matches!(
                result,
                Reservation::Decision(Decision::Wait { .. })
            ));
            assert_eq!(ledger.events_after(0, 256).unwrap().len(), 2);
        } else {
            assert!(matches!(result, Reservation::RecoveryRequired(_)));
            assert_eq!(
                ledger.attempt("capture-1").unwrap().unwrap().evidence,
                Evidence::Reserved
            );
            assert_eq!(ledger.events_after(0, 256).unwrap().len(), 1);
        }
    }
}

#[test]
fn engine_changes_require_explicit_migration() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    drop(open(&path));
    let connection = Connection::open(&path).unwrap();
    connection
        .execute("UPDATE allocation SET engine_version='future'", [])
        .unwrap();
    let r = request();
    assert!(matches!(
        Ledger::open(&path, r.assignment, r.state),
        Err(Error::UnsupportedEngine)
    ));
}

#[test]
fn timestamps_and_event_identity_survive_reopen_without_integer_truncation() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut r = request();
    r.assignment.valid_from_ms = u64::MAX - 1_000_000;
    r.assignment.expires_at_ms = u64::MAX;
    r.assignment.goals[0].eligible_windows[0].start_ms = u64::MAX - 1_000_000;
    r.assignment.goals[0].eligible_windows[0].end_ms = u64::MAX;
    r.state.now_ms = u64::MAX - 100_000;
    r.state.conditions_valid_until_ms = u64::MAX;
    let mut ledger = Ledger::open(&path, r.assignment.clone(), r.state.clone()).unwrap();
    assert!(matches!(
        ledger.reserve("capture-1", r.state.clone()).unwrap(),
        Reservation::Created(_)
    ));
    let before = ledger.events_after(0, 256).unwrap();
    drop(ledger);
    let ledger = Ledger::open(&path, r.assignment, r.state.clone()).unwrap();
    assert_eq!(ledger.events_after(0, 256).unwrap(), before);
    assert_eq!(before[0].attempt.reserved_at_ms, r.state.now_ms);
    assert!(!before[0].ledger_id.is_empty());
    assert_eq!(
        before[0].engine_version,
        psf_guard_director_core::ENGINE_VERSION
    );
}

#[test]
fn slow_preparation_changes_selection_using_the_same_core() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut r = request();
    let mut later = r.assignment.goals[0].clone();
    later.id = "later-target".into();
    later.priority = 1;
    later.eligible_windows[0].end_ms = 900_000;
    r.assignment.goals.push(later);
    let mut ledger = Ledger::open(&path, r.assignment, r.state.clone()).unwrap();
    r.state.now_ms = 80_000;
    let Reservation::Created(attempt) = ledger.reserve("capture-1", r.state).unwrap() else {
        panic!("expected later goal");
    };
    assert_eq!(attempt.goal_id, "later-target");
}
