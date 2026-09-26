use psf_guard_director_core::preparation::{
    Command, Completion, Context, Estimates, Next, Outcome,
};
use psf_guard_director_core::{Decision, Request, State};
use psf_guard_director_ledger::preparation::{EventKind, Lifecycle};
use psf_guard_director_ledger::{Error, Evidence, Ledger, Reservation};
use rusqlite::Connection;
use std::{
    path::Path,
    sync::{Arc, Barrier},
};
use tempfile::TempDir;

fn request() -> Request {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../director-core/tests/fixtures/decisions.json"
    ))
    .unwrap();
    let mut r: Request = serde_json::from_value(fixture["base"].clone()).unwrap();
    r.assignment.goals.truncate(1);
    r.assignment.goals[0].requested = 1;
    r.assignment.goals[0].accepted = 0;
    r.assignment.goals[0].pending = 0;
    r.assignment.goals[0].attempts_remaining = 2;
    r
}

fn open(path: &Path) -> Ledger {
    let r = request();
    Ledger::open(path, r.assignment, r.state).unwrap()
}

fn context() -> Context {
    Context {
        goal_id: "short-ha".into(),
        target_id: "target-1".into(),
        recipe_id: "recipe-1".into(),
        previous_target_id: Some("target-1".into()),
        filter_id: "ha".into(),
        readout_mode: 1,
        mount_parked: false,
        rotator_connected: false,
        enable_slew_center: true,
        dither_every: 0,
        dither_override: None,
        filter_exposures_since_dither: 0,
    }
}

fn begin(ledger: &mut Ledger) {
    assert!(
        ledger
            .begin_preparation("prep-1", context(), Estimates::default(), request().state)
            .unwrap()
            .created
    );
}

fn issue(ledger: &mut Ledger, state: &State) -> Command {
    let Next::Run(command) = ledger.advance_preparation("prep-1", state.clone()).unwrap() else {
        panic!("expected command");
    };
    command
}

fn receipt(command: &Command, now: u64, outcome: Outcome) -> Completion {
    Completion {
        preparation_id: command.preparation_id.clone(),
        ordinal: command.ordinal,
        ended_at_ms: now,
        elapsed_ms: 10,
        outcome,
    }
}

fn ready(ledger: &mut Ledger) -> State {
    let mut state = request().state;
    for _ in 0..2 {
        let command = issue(ledger, &state);
        state.now_ms += 10;
        ledger
            .complete_preparation(receipt(&command, state.now_ms, Outcome::Succeeded))
            .unwrap();
    }
    assert!(matches!(
        ledger.advance_preparation("prep-1", state.clone()).unwrap(),
        Next::ReadyToReserve { .. }
    ));
    state
}

#[test]
fn legacy_reserved_capture_check_retains_one_attempt_and_sticky_refusal() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    begin(&mut ledger);
    let mut state = ready(&mut ledger);
    ledger
        .reserve_prepared("prep-1", "capture", state.clone())
        .unwrap();
    assert!(matches!(
        ledger.check_prepared_capture_dispatch("prep-1", "capture", state.clone()),
        Ok(Decision::Acquire { .. })
    ));
    state.conditions_valid_until_ms = state.now_ms;
    let refusal = ledger
        .check_prepared_capture_dispatch("prep-1", "capture", state.clone())
        .unwrap();
    assert!(matches!(refusal, Decision::CheckIn { .. }));
    drop(ledger);
    let mut ledger = open(&path);
    state.conditions_valid_until_ms += 120_000;
    assert_eq!(
        ledger
            .check_prepared_capture_dispatch("prep-1", "capture", state)
            .unwrap(),
        refusal
    );
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 1);
}

#[test]
fn lost_reply_and_reopen_never_repeat_native_dispatch() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    begin(&mut ledger);
    let command = issue(&mut ledger, &request().state);
    drop(ledger);
    let mut ledger = open(&path);
    assert_eq!(
        ledger
            .advance_preparation("prep-1", request().state)
            .unwrap(),
        Next::InFlight { ordinal: 1 }
    );
    assert!(
        !ledger
            .begin_preparation("prep-1", context(), Estimates::default(), request().state)
            .unwrap()
            .created
    );
    assert_eq!(
        ledger.preparation("prep-1").unwrap().unwrap().pending,
        Some(command.clone())
    );
    let completion = receipt(&command, 10010, Outcome::Succeeded);
    ledger.complete_preparation(completion.clone()).unwrap();
    drop(ledger);
    let mut ledger = open(&path);
    ledger.complete_preparation(completion).unwrap();
    let mut state = request().state;
    state.now_ms = 10010;
    assert_eq!(issue(&mut ledger, &state).ordinal, 2);
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap().len(), 4);
    assert!(ledger.events_after(0, 256).unwrap().is_empty());
}

#[test]
fn final_reservation_is_atomic_linked_and_idempotent() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    begin(&mut ledger);
    let state = ready(&mut ledger);
    assert!(matches!(
        ledger
            .reserve_prepared("prep-1", "capture-1", state.clone())
            .unwrap(),
        Reservation::Created(_)
    ));
    drop(ledger);
    let mut ledger = open(&path);
    assert!(matches!(
        ledger
            .reserve_prepared("prep-1", "capture-1", state.clone())
            .unwrap(),
        Reservation::Existing(_)
    ));
    assert!(ledger
        .reserve_prepared("prep-1", "capture-2", state)
        .is_err());
    let record = ledger.preparation("prep-1").unwrap().unwrap();
    assert_eq!(record.lifecycle, Lifecycle::Captured);
    assert_eq!(record.capture_id.as_deref(), Some("capture-1"));
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 1);
    assert_eq!(ledger.events_after(0, 256).unwrap()[0].schema_version, 1);
    let events = ledger.preparation_events_after(0, 256).unwrap();
    assert_eq!(events.len(), 6);
    assert!(
        matches!(&events[5].event, EventKind::Captured { capture_id } if capture_id == "capture-1")
    );
    assert_eq!(ledger.preparation_events_after(2, 2).unwrap(), events[2..4]);
    assert!(ledger.preparation_events_after(6, 2).unwrap().is_empty());
    assert!(ledger
        .begin_preparation("prep-2", context(), Estimates::default(), request().state)
        .is_err());
}

#[test]
fn concurrent_prepared_reservations_commit_one_capture_and_link() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    begin(&mut ledger);
    let state = ready(&mut ledger);
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            let state = state.clone();
            std::thread::spawn(move || {
                let mut ledger = open(&path);
                barrier.wait();
                ledger
                    .reserve_prepared("prep-1", "capture-1", state)
                    .unwrap()
            })
        })
        .collect();
    let replies: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(
        replies
            .iter()
            .filter(|r| matches!(r, Reservation::Created(_)))
            .count(),
        1
    );
    assert_eq!(
        replies
            .iter()
            .filter(|r| matches!(r, Reservation::Existing(_)))
            .count(),
        1
    );
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 1);
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap().len(), 6);
}

#[test]
fn prepared_capture_uses_the_same_pending_progress_and_attempt_budget() {
    for saved in [false, true] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("execution.sqlite");
        let mut ledger = open(&path);
        // Consume one authorized attempt before this preparation starts.
        ledger.reserve("old-capture", request().state).unwrap();
        ledger
            .record(
                "old-capture",
                Evidence::Failed {
                    reason: "known_failure".into(),
                },
            )
            .unwrap();
        begin(&mut ledger);
        let state = ready(&mut ledger);
        ledger
            .reserve_prepared("prep-1", "capture-1", state.clone())
            .unwrap();
        let evidence = if saved {
            Evidence::Saved {
                image_id: "image-1".into(),
                elapsed_ms: 30000,
            }
        } else {
            Evidence::Failed {
                reason: "known_failure".into(),
            }
        };
        ledger.record("capture-1", evidence).unwrap();
        drop(ledger);
        let mut ledger = open(&path);
        assert!(ledger
            .begin_preparation("prep-2", context(), Estimates::default(), state.clone())
            .is_err());
        let next = ledger.reserve("extra-capture", state).unwrap();
        if saved {
            assert!(matches!(next, Reservation::Decision(Decision::Wait { .. })));
        } else {
            assert!(matches!(
                next,
                Reservation::Decision(Decision::CheckIn { .. })
            ));
        }
        assert!(ledger.preparation("prep-2").unwrap().is_none());
        assert!(ledger.attempt("extra-capture").unwrap().is_none());
    }
}

#[test]
fn ordinary_reservation_and_parallel_preparation_cannot_bypass_active_work() {
    let dir = TempDir::new().unwrap();
    let mut ledger = open(&dir.path().join("execution.sqlite"));
    begin(&mut ledger);
    assert!(
        matches!(ledger.reserve("capture-1", request().state).unwrap(), Reservation::Decision(Decision::CheckIn { reason }) if reason == "preparation_active")
    );
    assert!(ledger
        .begin_preparation("prep-2", context(), Estimates::default(), request().state)
        .is_err());
    assert!(ledger
        .reserve_prepared("prep-1", "capture-1", request().state)
        .is_err());
    assert!(ledger.attempt("capture-1").unwrap().is_none());
    ledger.close_preparation("prep-1").unwrap();
    ledger.close_preparation("prep-1").unwrap();
    assert!(
        ledger
            .begin_preparation("prep-2", context(), Estimates::default(), request().state)
            .unwrap()
            .created
    );
}

#[test]
fn stale_readiness_never_commits_a_capture() {
    let dir = TempDir::new().unwrap();
    let mut ledger = open(&dir.path().join("execution.sqlite"));
    begin(&mut ledger);
    let mut state = ready(&mut ledger);
    state.conditions_valid_until_ms = state.now_ms;
    assert!(
        matches!(ledger.reserve_prepared("prep-1", "capture-1", state).unwrap(), Reservation::Decision(Decision::CheckIn { reason }) if reason == "conditions_stale")
    );
    assert!(ledger.attempt("capture-1").unwrap().is_none());
    assert!(ledger
        .preparation("prep-1")
        .unwrap()
        .unwrap()
        .halted
        .is_some());
    assert!(ledger.events_after(0, 256).unwrap().is_empty());
}

#[test]
fn pending_and_uncertain_operations_cannot_be_closed_or_retried() {
    for uncertain in [false, true] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("execution.sqlite");
        let mut ledger = open(&path);
        begin(&mut ledger);
        let command = issue(&mut ledger, &request().state);
        if uncertain {
            ledger
                .complete_preparation(receipt(
                    &command,
                    10000,
                    Outcome::Uncertain {
                        reason: "lost_device".into(),
                    },
                ))
                .unwrap();
        }
        drop(ledger);
        let mut ledger = open(&path);
        assert!(ledger.close_preparation("prep-1").is_err());
        assert!(ledger
            .begin_preparation("prep-2", context(), Estimates::default(), request().state)
            .is_err());
        assert!(ledger
            .reserve_prepared("prep-1", "capture-1", request().state)
            .is_err());
    }
}

#[test]
fn concurrent_handles_issue_one_command_only() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    begin(&mut open(&path));
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut ledger = open(&path);
                barrier.wait();
                ledger
                    .advance_preparation("prep-1", request().state)
                    .unwrap()
            })
        })
        .collect();
    let replies: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(
        replies.iter().filter(|r| matches!(r, Next::Run(_))).count(),
        1
    );
    assert_eq!(
        replies
            .iter()
            .filter(|r| matches!(r, Next::InFlight { ordinal: 1 }))
            .count(),
        1
    );
}

#[test]
fn operation_and_outbox_write_failures_roll_back_together() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    begin(&mut ledger);
    let other = Connection::open(&path).unwrap();
    other.execute_batch("CREATE TRIGGER fail_preparation_event BEFORE INSERT ON preparation_event BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(matches!(
        ledger.advance_preparation("prep-1", request().state),
        Err(Error::Sqlite(_))
    ));
    assert!(ledger
        .preparation("prep-1")
        .unwrap()
        .unwrap()
        .pending
        .is_none());
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap().len(), 1);
    other
        .execute_batch("DROP TRIGGER fail_preparation_event")
        .unwrap();
    let state = ready(&mut ledger);
    other.execute_batch("CREATE TRIGGER fail_preparation_event BEFORE INSERT ON preparation_event BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(matches!(
        ledger.reserve_prepared("prep-1", "capture-1", state.clone()),
        Err(Error::Sqlite(_))
    ));
    assert!(ledger.attempt("capture-1").unwrap().is_none());
    assert_eq!(
        ledger.preparation("prep-1").unwrap().unwrap().lifecycle,
        Lifecycle::Active
    );
    assert!(ledger.events_after(0, 256).unwrap().is_empty());
    other
        .execute_batch("DROP TRIGGER fail_preparation_event")
        .unwrap();
    assert!(matches!(
        ledger
            .reserve_prepared("prep-1", "capture-1", state)
            .unwrap(),
        Reservation::Created(_)
    ));
}

#[test]
fn version_one_migration_preserves_identity_capture_evidence_and_event_schema() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    let identity = ledger.info();
    ledger.reserve("capture-1", request().state).unwrap();
    ledger
        .record(
            "capture-1",
            Evidence::Failed {
                reason: "known_failure".into(),
            },
        )
        .unwrap();
    let events = ledger.events_after(0, 256).unwrap();
    drop(ledger);
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "DROP TABLE preparation; DROP TABLE preparation_event; DROP TABLE execution_program; ALTER TABLE allocation DROP COLUMN program_required; DROP TABLE execution_geometry; ALTER TABLE allocation DROP COLUMN geometry_required; PRAGMA user_version=1;",
    )
    .unwrap();
    drop(db);
    let mut ledger = open(&path);
    assert_eq!(ledger.info(), identity);
    assert_eq!(ledger.events_after(0, 256).unwrap(), events);
    begin(&mut ledger);
    let state = ready(&mut ledger);
    ledger
        .reserve_prepared("prep-1", "capture-2", state)
        .unwrap();
    assert_eq!(ledger.events_after(0, 256).unwrap()[2].schema_version, 1);
}

#[test]
fn late_and_duplicate_receipts_are_durable_and_conflicts_do_not_change_evidence() {
    let dir = TempDir::new().unwrap();
    let mut ledger = open(&dir.path().join("execution.sqlite"));
    begin(&mut ledger);
    let command = issue(&mut ledger, &request().state);
    let mut state = request().state;
    state.now_ms += 100;
    ledger.advance_preparation("prep-1", state.clone()).unwrap();
    let receipt = receipt(&command, state.now_ms - 50, Outcome::Succeeded);
    ledger.complete_preparation(receipt.clone()).unwrap();
    ledger.complete_preparation(receipt.clone()).unwrap();
    let mut conflict = receipt;
    conflict.elapsed_ms += 1;
    assert!(ledger.complete_preparation(conflict).is_err());
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap().len(), 3);
    assert_eq!(issue(&mut ledger, &state).ordinal, 2);
}

#[test]
fn malformed_checkpoints_fail_closed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    begin(&mut ledger);
    let db = Connection::open(&path).unwrap();
    db.execute("UPDATE preparation SET checkpoint=?1", [b"{}".as_slice()])
        .unwrap();
    assert!(matches!(
        ledger.advance_preparation("prep-1", request().state),
        Err(Error::CorruptLedger)
    ));
    assert!(matches!(
        ledger.reserve_prepared("prep-1", "capture-1", request().state),
        Err(Error::CorruptLedger)
    ));
    assert!(ledger.attempt("capture-1").unwrap().is_none());
}

#[test]
fn valid_json_corruption_cannot_erase_an_in_flight_operation() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    begin(&mut ledger);
    issue(&mut ledger, &request().state);
    let db = Connection::open(&path).unwrap();
    let bytes: Vec<u8> = db
        .query_row(
            "SELECT checkpoint FROM preparation WHERE id='prep-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["pending_issued_at_ms"] = serde_json::Value::Null;
    db.execute(
        "UPDATE preparation SET checkpoint=?1",
        [serde_json::to_vec(&value).unwrap()],
    )
    .unwrap();
    assert!(matches!(
        ledger.advance_preparation("prep-1", request().state),
        Err(Error::CorruptLedger)
    ));
    assert!(ledger.close_preparation("prep-1").is_err());
    assert!(ledger
        .begin_preparation("prep-2", context(), Estimates::default(), request().state)
        .is_err());
}

#[test]
fn prepared_capture_does_not_charge_finished_setup_estimates_twice() {
    let dir = TempDir::new().unwrap();
    let mut ledger = open(&dir.path().join("execution.sqlite"));
    begin(&mut ledger);
    let mut state = ready(&mut ledger);
    state.now_ms = 70_000; // 30-second exposure fits exactly; old 10-second setup does not.
    assert!(matches!(
        ledger
            .reserve_prepared("prep-1", "capture-1", state)
            .unwrap(),
        Reservation::Created(_)
    ));
}

#[test]
fn wrong_rig_and_configuration_changes_remain_halted_after_reopen() {
    for change_rig in [false, true] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("execution.sqlite");
        let mut ledger = open(&path);
        begin(&mut ledger);
        let command = issue(&mut ledger, &request().state);
        let mut state = request().state;
        if change_rig {
            state.rig_id = "wrong-rig".into();
        } else {
            state.configuration_id = "changed-config".into();
        }
        assert!(matches!(
            ledger.advance_preparation("prep-1", state).unwrap(),
            Next::InFlight { .. }
        ));
        drop(ledger);
        let mut ledger = open(&path);
        ledger
            .complete_preparation(receipt(&command, 10000, Outcome::Succeeded))
            .unwrap();
        assert!(
            matches!(ledger.advance_preparation("prep-1", request().state).unwrap(), Next::Decision(Decision::CheckIn { reason }) if reason == "configuration_mismatch")
        );
        assert!(ledger
            .reserve_prepared("prep-1", "capture-1", request().state)
            .is_err());
    }
}

#[test]
fn migration_failure_does_not_modify_another_assignments_ledger() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    drop(open(&path));
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "DROP TABLE preparation; DROP TABLE preparation_event; DROP TABLE execution_program; ALTER TABLE allocation DROP COLUMN program_required; DROP TABLE execution_geometry; ALTER TABLE allocation DROP COLUMN geometry_required; PRAGMA user_version=1;",
    )
    .unwrap();
    let mut r = request();
    r.assignment.id = "not-the-owner".into();
    assert!(matches!(
        Ledger::open(&path, r.assignment, r.state),
        Err(Error::AssignmentMismatch)
    ));
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='preparation'",
            [],
            |r| r.get::<_, i32>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn invalid_ids_pages_and_changed_definitions_do_not_mutate_journal() {
    let dir = TempDir::new().unwrap();
    let mut ledger = open(&dir.path().join("execution.sqlite"));
    begin(&mut ledger);
    let mut changed = context();
    changed.recipe_id = "different-recipe".into();
    assert!(ledger
        .begin_preparation("prep-1", changed, Estimates::default(), request().state)
        .is_err());
    assert!(ledger.preparation("").is_err());
    assert!(ledger
        .advance_preparation("unknown", request().state)
        .is_err());
    assert!(ledger
        .reserve_prepared("prep-1", "", request().state)
        .is_err());
    assert!(ledger.preparation_events_after(0, 0).is_err());
    assert!(ledger.preparation_events_after(0, 257).is_err());
    assert!(ledger.preparation_events_after(u64::MAX, 1).is_err());
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap().len(), 1);
}

#[test]
fn abrupt_exit_preserves_pending_and_completed_preparation_and_rolls_back_partial_writes() {
    for phase in ["pending", "completed", "partial"] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("execution.sqlite");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "preparation_crash_child", "--nocapture"])
            .env("DIRECTOR_PREPARATION_CRASH_PATH", &path)
            .env("DIRECTOR_PREPARATION_CRASH_PHASE", phase)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(72));
        let mut ledger = open(&path);
        let result = ledger
            .advance_preparation("prep-1", request().state)
            .unwrap();
        if phase == "completed" {
            assert!(matches!(result, Next::Run(Command { ordinal: 2, .. })));
            assert_eq!(ledger.preparation_events_after(0, 256).unwrap().len(), 4);
        } else {
            assert_eq!(result, Next::InFlight { ordinal: 1 });
            assert_eq!(ledger.preparation_events_after(0, 256).unwrap().len(), 2);
        }
    }
}

#[test]
fn preparation_crash_child() {
    let Some(path) = std::env::var_os("DIRECTOR_PREPARATION_CRASH_PATH") else {
        return;
    };
    let mut ledger = open(Path::new(&path));
    begin(&mut ledger);
    let command = issue(&mut ledger, &request().state);
    match std::env::var("DIRECTOR_PREPARATION_CRASH_PHASE").as_deref() {
        Ok("completed") => {
            ledger
                .complete_preparation(receipt(&command, 10000, Outcome::Succeeded))
                .unwrap();
        }
        Ok("partial") => {
            let db = Connection::open(&path).unwrap();
            db.execute_batch("BEGIN IMMEDIATE; UPDATE preparation SET checkpoint=X'00'; INSERT INTO preparation_event(payload) VALUES('unfinished');").unwrap();
            std::process::exit(72);
        }
        _ => {}
    }
    // Skip SQLite and Rust destructors, as when the sidecar is terminated.
    std::process::exit(72);
}
