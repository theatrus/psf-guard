use psf_guard_director_core::preparation::{Completion, Estimates, Next, Outcome};
use psf_guard_director_core::program::*;
use psf_guard_director_core::{Decision, Request, State};
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
    let mut request: Request = serde_json::from_value(fixture["base"].clone()).unwrap();
    request.assignment.goals.truncate(1);
    let goal = &mut request.assignment.goals[0];
    goal.requested = 1;
    goal.accepted = 0;
    goal.pending = 0;
    goal.attempts_remaining = 2;
    request
}

fn program() -> Program {
    Program {
        schema_version: PROGRAM_VERSION,
        assignment: request().assignment,
        configuration: Configuration {
            rig_id: "rig-1".into(),
            id: "config-1".into(),
            camera_id: "camera-1".into(),
            filter_wheel_id: Some("wheel-1".into()),
            filters: vec![Filter {
                id: "ha".into(),
                position: Some(2),
            }],
            binning_modes: vec![Binning { x: 1, y: 1 }],
            readout_modes: vec![0],
            gain: Control::Range {
                minimum: 0,
                maximum: 100,
            },
            offset: Control::Unsupported,
            exposure_min_ms: 1,
            exposure_max_ms: 1000000,
            enable_slew_center: true,
            dither_every: 0,
        },
        targets: vec![Target {
            id: "target-1".into(),
            name: "M42".into(),
            icrs_ra_mas: 298800000,
            icrs_dec_mas: -18000000,
            position_angle_mas: None,
        }],
        recipes: vec![Recipe {
            id: "recipe-1".into(),
            exposure_ms: 30000,
            filter_id: "ha".into(),
            binning: Binning { x: 1, y: 1 },
            gain: Some(40),
            offset: None,
            readout_mode: 0,
            dither_override: None,
        }],
        bindings: vec![Binding {
            goal_id: "short-ha".into(),
            target_id: "target-1".into(),
            recipe_id: "recipe-1".into(),
        }],
    }
}

fn local() -> LocalState {
    let program = program();
    LocalState {
        configuration: program.configuration.clone(),
        previous_pointing: Some(PointingContext {
            configuration_id: program.configuration.id,
            target: program.targets[0].clone(),
        }),
        mount_parked: false,
        rotator_connected: false,
        filter_exposures_since_dither: 0,
    }
}

fn open(path: &Path) -> Ledger {
    Ledger::open_program(path, program(), request().state).unwrap()
}
fn begin(ledger: &mut Ledger) {
    ledger
        .begin_program_preparation(
            "prep",
            "short-ha",
            local(),
            Estimates::default(),
            request().state,
        )
        .unwrap();
}
fn ready(ledger: &mut Ledger) -> State {
    let mut state = request().state;
    for _ in 0..2 {
        let Next::Run(command) = ledger
            .advance_program_preparation("prep", state.clone(), &program().configuration)
            .unwrap()
        else {
            panic!("expected native operation")
        };
        state.now_ms += 1;
        ledger
            .complete_preparation(Completion {
                preparation_id: "prep".into(),
                ordinal: command.ordinal,
                ended_at_ms: state.now_ms,
                elapsed_ms: 1,
                outcome: Outcome::Succeeded,
            })
            .unwrap();
    }
    assert!(matches!(
        ledger
            .advance_program_preparation("prep", state.clone(), &program().configuration)
            .unwrap(),
        Next::ReadyToReserve { .. }
    ));
    state
}

#[test]
fn program_and_exact_capture_recipe_survive_restart() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    let identity = ledger.info();
    assert_eq!(ledger.program(), Some(&program()));
    assert!(ledger.capture_binding("missing").unwrap().is_none());
    begin(&mut ledger);
    let state = ready(&mut ledger);
    assert!(matches!(
        ledger
            .reserve_program_prepared("prep", "capture", state.clone(), &program().configuration)
            .unwrap(),
        Reservation::Created(_)
    ));
    let binding = ledger.capture_binding("capture").unwrap().unwrap();
    assert_eq!(binding.ledger, identity);
    assert_eq!(binding.recipe, program().recipes[0]);
    assert_eq!(binding.target, program().targets[0]);
    drop(ledger);
    let mut ledger = open(&path);
    assert_eq!(ledger.capture_binding("capture").unwrap(), Some(binding));
    assert!(matches!(
        ledger
            .reserve_program_prepared("prep", "capture", state.clone(), &program().configuration)
            .unwrap(),
        Reservation::Existing(_)
    ));
    ledger
        .record(
            "capture",
            Evidence::Saved {
                image_id: "image".into(),
                elapsed_ms: 9007199254740993,
            },
        )
        .unwrap();
    assert!(
        matches!(ledger.evaluate(state).unwrap(),Decision::Wait{reason} if reason=="pending_assessment")
    );
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 2);
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap().len(), 6);
}

#[test]
fn bound_ledger_cannot_downgrade_or_change_its_program() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    begin(&mut ledger);
    let request = request();
    assert!(matches!(
        Ledger::open(&path, request.assignment, request.state),
        Err(Error::AssignmentMismatch)
    ));
    for fault in 0..4 {
        let mut program = program();
        match fault {
            0 => program.targets[0].icrs_ra_mas += 1,
            1 => program.recipes[0].gain = Some(99),
            2 => program.configuration.camera_id = "replacement".into(),
            _ => program.configuration.filters[0].position = Some(3),
        }
        assert!(matches!(
            Ledger::open_program(&path, program, self::request().state),
            Err(Error::AssignmentMismatch)
        ));
    }
    assert!(ledger.active_preparation().unwrap().is_some());
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap().len(), 1);
}

#[test]
fn legacy_mutations_cannot_bypass_program_checks() {
    let dir = TempDir::new().unwrap();
    let mut ledger = open(&dir.path().join("execution.sqlite"));
    let context = BoundProgram::new(program(), &request().state)
        .unwrap()
        .preparation_context("short-ha", local())
        .unwrap();
    assert!(matches!(
        ledger.reserve("capture", request().state),
        Err(Error::ConflictingEvidence)
    ));
    assert!(matches!(
        ledger.begin_preparation("prep", context, Estimates::default(), request().state),
        Err(Error::ConflictingEvidence)
    ));
    begin(&mut ledger);
    assert!(matches!(
        ledger.advance_preparation("prep", request().state),
        Err(Error::ConflictingEvidence)
    ));
    let state = ready(&mut ledger);
    assert!(matches!(
        ledger.reserve_prepared("prep", "capture", state),
        Err(Error::ConflictingEvidence)
    ));
    assert!(ledger.events_after(0, 256).unwrap().is_empty());
}

#[test]
fn unbound_ledgers_cannot_adopt_programs_even_when_empty() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let request = request();
    let mut ledger = Ledger::open(&path, request.assignment, request.state).unwrap();
    assert!(matches!(
        Ledger::open_program(&path, program(), self::request().state),
        Err(Error::AssignmentMismatch)
    ));
    assert!(matches!(
        ledger.begin_program_preparation(
            "prep",
            "short-ha",
            local(),
            Estimates::default(),
            self::request().state
        ),
        Err(Error::ConflictingEvidence)
    ));
    assert!(ledger.program().is_none());
    assert!(matches!(
        ledger
            .reserve("legacy-capture", self::request().state)
            .unwrap(),
        Reservation::Created(_)
    ));
}

#[test]
fn fresh_equipment_snapshot_is_required_at_every_bound_operation() {
    let dir = TempDir::new().unwrap();
    let mut ledger = open(&dir.path().join("execution.sqlite"));
    let mut changed = local();
    changed.configuration.filters[0].position = Some(4);
    assert!(matches!(
        ledger.begin_program_preparation(
            "prep",
            "short-ha",
            changed.clone(),
            Estimates::default(),
            request().state
        ),
        Err(Error::AssignmentMismatch)
    ));
    assert!(ledger.active_preparation().unwrap().is_none());
    begin(&mut ledger);
    assert!(matches!(
        ledger.advance_program_preparation("prep", request().state, &changed.configuration),
        Err(Error::AssignmentMismatch)
    ));
    assert!(ledger
        .active_preparation()
        .unwrap()
        .unwrap()
        .pending
        .is_none());
    let state = ready(&mut ledger);
    assert!(matches!(
        ledger.reserve_program_prepared("prep", "capture", state, &changed.configuration),
        Err(Error::AssignmentMismatch)
    ));
    assert!(ledger.events_after(0, 256).unwrap().is_empty());
}

#[test]
fn migration_retains_unbound_preparation_without_reinterpreting_it() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let request = request();
    let mut ledger =
        Ledger::open(&path, request.assignment.clone(), request.state.clone()).unwrap();
    let identity = ledger.info();
    let context = BoundProgram::new(program(), &request.state)
        .unwrap()
        .preparation_context("short-ha", local())
        .unwrap();
    ledger
        .begin_preparation(
            "legacy",
            context,
            Estimates::default(),
            request.state.clone(),
        )
        .unwrap();
    ledger
        .advance_preparation("legacy", request.state.clone())
        .unwrap();
    let record = ledger.preparation("legacy").unwrap();
    let events = ledger.preparation_events_after(0, 256).unwrap();
    drop(ledger);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TABLE execution_program; ALTER TABLE allocation DROP COLUMN program_required; PRAGMA user_version=2;").unwrap();
    assert!(matches!(
        Ledger::open_program(&path, program(), request.state.clone()),
        Err(Error::AssignmentMismatch)
    ));
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='execution_program'",
            [],
            |row| row.get::<_, i32>(0)
        )
        .unwrap(),
        0
    );
    let mut ledger = Ledger::open(&path, request.assignment, request.state.clone()).unwrap();
    assert_eq!(ledger.info(), identity);
    assert!(ledger.program().is_none());
    assert_eq!(ledger.preparation("legacy").unwrap(), record);
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap(), events);
    assert!(matches!(
        ledger.advance_preparation("legacy", request.state).unwrap(),
        Next::InFlight { .. }
    ));
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
            .unwrap(),
        3
    );
}

#[test]
fn damaged_or_missing_program_metadata_never_becomes_an_unbound_ledger() {
    for change in [
        "UPDATE execution_program SET payload=replace(payload,'M42','M43')",
        "DELETE FROM execution_program",
        "UPDATE allocation SET program_required=0",
    ] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("execution.sqlite");
        drop(open(&path));
        let db = Connection::open(&path).unwrap();
        db.execute_batch(change).unwrap();
        assert!(matches!(
            Ledger::open_program(&path, program(), request().state),
            Err(Error::CorruptLedger)
        ));
        let request = request();
        assert!(matches!(
            Ledger::open(&path, request.assignment, request.state),
            Err(Error::CorruptLedger)
        ));
    }
}

#[test]
fn idempotent_begin_does_not_reselect_after_capture_changes_progress() {
    let dir = TempDir::new().unwrap();
    let mut ledger = open(&dir.path().join("execution.sqlite"));
    begin(&mut ledger);
    let state = ready(&mut ledger);
    ledger
        .reserve_program_prepared("prep", "capture", state, &program().configuration)
        .unwrap();
    let retry = ledger
        .begin_program_preparation(
            "prep",
            "short-ha",
            local(),
            Estimates::default(),
            request().state,
        )
        .unwrap();
    assert!(!retry.created);
    assert_eq!(retry.record.capture_id.as_deref(), Some("capture"));
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap().len(), 6);
}

#[test]
fn concurrent_bound_handles_issue_once_and_reserve_once() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut first = open(&path);
    begin(&mut first);
    let second = open(&path);
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = [first, second]
        .into_iter()
        .map(|mut ledger| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                ledger
                    .advance_program_preparation("prep", request().state, &program().configuration)
                    .unwrap()
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Next::Run(_)))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Next::InFlight { .. }))
            .count(),
        1
    );
    let mut ledger = open(&path);
    let pending = ledger
        .active_preparation()
        .unwrap()
        .unwrap()
        .pending
        .unwrap();
    ledger
        .complete_preparation(Completion {
            preparation_id: "prep".into(),
            ordinal: pending.ordinal,
            ended_at_ms: 10000,
            elapsed_ms: 1,
            outcome: Outcome::Succeeded,
        })
        .unwrap();
    let Next::Run(command) = ledger
        .advance_program_preparation("prep", request().state, &program().configuration)
        .unwrap()
    else {
        panic!()
    };
    ledger
        .complete_preparation(Completion {
            preparation_id: "prep".into(),
            ordinal: command.ordinal,
            ended_at_ms: 10000,
            elapsed_ms: 1,
            outcome: Outcome::Succeeded,
        })
        .unwrap();
    let second = open(&path);
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = [ledger, second]
        .into_iter()
        .map(|mut ledger| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                ledger
                    .reserve_program_prepared(
                        "prep",
                        "capture",
                        request().state,
                        &program().configuration,
                    )
                    .unwrap()
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Reservation::Created(_)))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Reservation::Existing(_)))
            .count(),
        1
    );
    assert_eq!(open(&path).events_after(0, 256).unwrap().len(), 1);
}

#[test]
fn failed_event_commit_keeps_program_binding_and_reservation_atomic() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = open(&path);
    begin(&mut ledger);
    let state = ready(&mut ledger);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_event BEFORE INSERT ON event BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(matches!(
        ledger.reserve_program_prepared("prep", "capture", state.clone(), &program().configuration),
        Err(Error::Sqlite(_))
    ));
    assert!(ledger.capture_binding("capture").unwrap().is_none());
    assert!(ledger
        .preparation("prep")
        .unwrap()
        .unwrap()
        .capture_id
        .is_none());
    db.execute_batch("DROP TRIGGER fail_event").unwrap();
    assert!(matches!(
        ledger
            .reserve_program_prepared("prep", "capture", state, &program().configuration)
            .unwrap(),
        Reservation::Created(_)
    ));
    assert_eq!(
        ledger.capture_binding("capture").unwrap().unwrap().recipe,
        program().recipes[0]
    );
}

#[test]
fn program_process_exit_helper() {
    let Some(path) = std::env::var_os("DIRECTOR_BOUND_CRASH_DB") else {
        return;
    };
    let mut ledger = open(Path::new(&path));
    begin(&mut ledger);
    if std::env::var("DIRECTOR_BOUND_CRASH_PHASE").unwrap() == "issued" {
        ledger
            .advance_program_preparation("prep", request().state, &program().configuration)
            .unwrap();
    } else {
        let state = ready(&mut ledger);
        ledger
            .reserve_program_prepared("prep", "capture", state, &program().configuration)
            .unwrap();
    }
    std::process::exit(81);
}

#[test]
fn process_exit_cannot_erase_binding_or_authorize_redispatch() {
    for phase in ["issued", "captured"] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("execution.sqlite");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "program_process_exit_helper"])
            .env("DIRECTOR_BOUND_CRASH_DB", &path)
            .env("DIRECTOR_BOUND_CRASH_PHASE", phase)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(81));
        let mut ledger = open(&path);
        assert_eq!(ledger.program(), Some(&program()));
        if phase == "issued" {
            assert!(matches!(
                ledger
                    .advance_program_preparation("prep", request().state, &program().configuration)
                    .unwrap(),
                Next::InFlight { .. }
            ));
            assert!(matches!(
                ledger.close_preparation("prep"),
                Err(Error::ConflictingEvidence)
            ));
        } else {
            assert!(matches!(
                ledger
                    .reserve_program_prepared(
                        "prep",
                        "capture",
                        request().state,
                        &program().configuration
                    )
                    .unwrap(),
                Reservation::Existing(_)
            ));
            assert_eq!(
                ledger.capture_binding("capture").unwrap().unwrap().recipe,
                program().recipes[0]
            );
        }
    }
}
