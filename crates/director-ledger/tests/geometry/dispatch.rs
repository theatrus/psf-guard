use super::*;
use psf_guard_director_core::preparation::Command;
use psf_guard_director_ledger::preparation::EventKind;

fn check(f: &Fixture, ledger: &mut Ledger, command: &Command) -> Result<Decision, Error> {
    ledger.check_geometry_pending_dispatch(
        command,
        f.state.clone(),
        &f.program.configuration,
        &f.constraints,
    )
}

fn issued(f: &Fixture, ledger: &mut Ledger) -> Command {
    f.begin(ledger);
    let Next::Run(command) = f.next(ledger) else {
        panic!()
    };
    command
}

#[test]
fn dispatch_refusal_survives_reopen_without_duplicate_halt_or_issuance() {
    let mut f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    let command = issued(&f, &mut ledger);
    let original = ledger.preparation("prep").unwrap();
    for _ in 0..3 {
        assert!(matches!(
            check(&f, &mut ledger, &command),
            Ok(Decision::Acquire { .. })
        ));
        assert_eq!(ledger.preparation("prep").unwrap(), original);
        assert_eq!(ledger.preparation_events_after(0, 256).unwrap().len(), 2);
    }
    f.state.now_ms = f.first_end() - 4_999;
    let refused = check(&f, &mut ledger, &command).unwrap();
    assert!(matches!(refused, Decision::Wait { .. }));
    drop(ledger);
    let mut ledger = f.open(&path);
    for _ in 0..3 {
        assert_eq!(check(&f, &mut ledger, &command).unwrap(), refused);
    }
    let events = ledger.preparation_events_after(0, 256).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[2].event,
        EventKind::Halted {
            decision: refused.clone()
        }
    );
    assert_eq!(
        ledger.preparation("prep").unwrap().unwrap().pending,
        Some(command.clone())
    );
    assert!(ledger.events_after(0, 256).unwrap().is_empty());
    let completion = Completion {
        preparation_id: command.preparation_id.clone(),
        ordinal: command.ordinal,
        ended_at_ms: f.state.now_ms,
        elapsed_ms: 0,
        outcome: Outcome::Failed {
            reason: "not_dispatched".into(),
        },
    };
    ledger.complete_preparation(completion.clone()).unwrap();
    ledger.complete_preparation(completion).unwrap();
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap().len(), 4);
    assert_eq!(f.next(&mut ledger), Next::Decision(refused));
    assert!(check(&f, &mut ledger, &command).is_err());
}

#[test]
fn event_failure_rolls_back_checkpoint_and_clock_together() {
    let mut f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    let command = issued(&f, &mut ledger);
    let original = ledger.preparation("prep").unwrap();
    let events = ledger.preparation_events_after(0, 256).unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_halt BEFORE INSERT ON preparation_event BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
    f.state.now_ms = f.first_end() - 4_999;
    assert!(check(&f, &mut ledger, &command).is_err());
    drop(ledger);
    let mut ledger = f.open(&path);
    assert_eq!(ledger.preparation("prep").unwrap(), original);
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap(), events);
    db.execute_batch("DROP TRIGGER fail_halt;").unwrap();
    f.state.now_ms = START;
    assert!(matches!(
        check(&f, &mut ledger, &command),
        Ok(Decision::Acquire { .. })
    ));
}

#[test]
fn stale_commands_modes_and_configuration_cannot_mutate_dispatch_evidence() {
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    let command = issued(&f, &mut ledger);
    let original = ledger.preparation("prep").unwrap();
    let events = ledger.preparation_events_after(0, 256).unwrap();
    assert!(ledger
        .check_pending_preparation_dispatch(&command, f.state.clone())
        .is_err());
    assert!(ledger
        .check_program_pending_dispatch(&command, f.state.clone(), &f.program.configuration)
        .is_err());
    let mut changed = f.clone();
    changed.program.configuration.id = "other".into();
    assert!(check(&changed, &mut ledger, &command).is_err());
    for fault in 0..6 {
        let mut wrong = command.clone();
        match fault {
            0 => wrong.preparation_id = "other".into(),
            1 => wrong.ordinal += 1,
            2 => wrong.goal_id = "other".into(),
            3 => wrong.target_id = "other".into(),
            4 => wrong.recipe_id = "other".into(),
            _ => wrong.operation = psf_guard_director_core::preparation::Operation::Unpark,
        }
        assert!(check(&f, &mut ledger, &wrong).is_err());
    }
    assert_eq!(ledger.preparation("prep").unwrap(), original);
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap(), events);
    f.finish(&mut ledger, command.ordinal);
    assert!(check(&f, &mut ledger, &command).is_err());
    f.ready(&mut ledger);
    f.reserve(&mut ledger).unwrap();
    assert!(matches!(
        check(&f, &mut ledger, &command),
        Err(Error::ConflictingEvidence)
    ));
}

#[test]
fn second_handle_observes_sticky_constraint_refusal_and_safety_escalation() {
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut first = f.open(&path);
    let mut second = f.open(&path);
    let command = issued(&f, &mut first);
    let mut changed = f.clone();
    changed.constraints.rig.site.longitude_degrees += 0.001;
    let refusal = check(&changed, &mut second, &command).unwrap();
    assert!(matches!(refusal, Decision::CheckIn { .. }));
    assert_eq!(check(&f, &mut first, &command).unwrap(), refusal);
    changed.state.safety = Safety::Unsafe;
    let stopped = check(&changed, &mut first, &command).unwrap();
    assert!(matches!(stopped, Decision::Stop { .. }));
    assert_eq!(check(&f, &mut second, &command).unwrap(), stopped);
    let events = second.preparation_events_after(0, 256).unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events[2].event, EventKind::Halted { decision: refusal });
    assert_eq!(events[3].event, EventKind::Halted { decision: stopped });
    f.finish(&mut first, command.ordinal);
    assert!(check(&f, &mut second, &command).is_err());
}

#[test]
fn nonboundary_and_clock_checks_remain_durable_without_issuing_events() {
    let mut f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    let command = issued(&f, &mut ledger);
    f.state.now_ms += 100;
    f.state.at_boundary = false;
    assert!(matches!(
        check(&f, &mut ledger, &command),
        Ok(Decision::Continue { .. })
    ));
    assert!(ledger
        .preparation("prep")
        .unwrap()
        .unwrap()
        .halted
        .is_none());
    drop(ledger);
    let mut ledger = f.open(&path);
    f.state.now_ms -= 1;
    assert!(check(&f, &mut ledger, &command).is_err());
    f.state.now_ms += 1;
    f.state.at_boundary = true;
    assert!(matches!(
        check(&f, &mut ledger, &command),
        Ok(Decision::Acquire { .. })
    ));
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap().len(), 2);
    assert_eq!(
        f.next(&mut ledger),
        Next::InFlight {
            ordinal: command.ordinal
        }
    );
}

#[test]
fn program_only_dispatch_check_cannot_adopt_geometry() {
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = Ledger::open_program(&path, f.program.clone(), f.state.clone()).unwrap();
    ledger
        .begin_program_preparation(
            "prep",
            "short-ha",
            f.local(),
            Estimates::default(),
            f.state.clone(),
        )
        .unwrap();
    let Next::Run(command) = ledger
        .advance_program_preparation("prep", f.state.clone(), &f.program.configuration)
        .unwrap()
    else {
        panic!()
    };
    assert!(check(&f, &mut ledger, &command).is_err());
    assert!(ledger
        .check_pending_preparation_dispatch(&command, f.state.clone())
        .is_err());
    assert!(matches!(
        ledger.check_program_pending_dispatch(&command, f.state.clone(), &f.program.configuration),
        Ok(Decision::Acquire { .. })
    ));
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap().len(), 2);
}
