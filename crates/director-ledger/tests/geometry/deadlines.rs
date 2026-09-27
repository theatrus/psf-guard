use super::*;

#[test]
fn pending_deadline_includes_remaining_work_without_issuing_or_reserving_again() {
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let mut ledger = f.open(&dir.path().join("execution.sqlite"));
    ledger
        .begin_geometry_preparation(
            "prep",
            "short-ha",
            f.local(),
            Estimates {
                center_ms: 3_000,
                capture_overhead_ms: 2_000,
                ..Estimates::default()
            },
            f.state.clone(),
            &f.constraints,
        )
        .unwrap();
    let Next::Run(command) = f.next(&mut ledger) else {
        panic!()
    };
    let record = ledger.preparation("prep").unwrap();
    let events = ledger.preparation_events_after(0, 256).unwrap();
    for _ in 0..2 {
        let check = ledger
            .check_geometry_pending_dispatch_deadline(
                &command,
                f.state.clone(),
                &f.program.configuration,
                &f.constraints,
            )
            .unwrap();
        assert!(matches!(check.decision, Decision::Acquire { .. }));
        assert_eq!(check.evaluated_at_ms, START);
        assert_eq!(check.latest_start_ms, Some(f.first_end() - 10_000));
    }
    assert_eq!(ledger.preparation("prep").unwrap(), record);
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap(), events);
    assert!(ledger.events_after(0, 256).unwrap().is_empty());

    let mut wrong = command.clone();
    wrong.ordinal += 1;
    assert!(ledger
        .check_geometry_pending_dispatch_deadline(
            &wrong,
            f.state.clone(),
            &f.program.configuration,
            &f.constraints,
        )
        .is_err());
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap(), events);
}

#[test]
fn capture_deadline_uses_last_reserved_attempt_and_only_remaining_overhead() {
    let mut f = Fixture::new();
    f.program.assignment.goals[0].attempts_remaining = 1;
    let dir = TempDir::new().unwrap();
    let mut ledger = f.open(&dir.path().join("execution.sqlite"));
    ledger
        .begin_geometry_preparation(
            "prep",
            "short-ha",
            f.local(),
            Estimates {
                center_ms: 3_000,
                capture_overhead_ms: 2_000,
                ..Estimates::default()
            },
            f.state.clone(),
            &f.constraints,
        )
        .unwrap();
    f.ready(&mut ledger);
    f.reserve(&mut ledger).unwrap();
    let attempt = ledger.attempt("capture").unwrap();
    let events = ledger.events_after(0, 256).unwrap();
    let check = ledger
        .check_geometry_capture_dispatch_deadline(
            "prep",
            "capture",
            f.state.clone(),
            &f.program.configuration,
            &f.constraints,
        )
        .unwrap();
    assert_eq!(check.evaluated_at_ms, START);
    assert_eq!(check.latest_start_ms, Some(f.first_end() - 7_000));
    assert_eq!(
        check.decision,
        Decision::Acquire {
            goal_id: "short-ha".into(),
            reason: "reserved_capture_ready".into(),
        }
    );
    assert_eq!(ledger.attempt("capture").unwrap(), attempt);
    assert_eq!(ledger.events_after(0, 256).unwrap(), events);
    assert!(!matches!(
        ledger.evaluate_geometry(f.state.clone(), &f.constraints),
        Ok(Decision::Acquire { .. })
    ));
}

#[test]
fn condition_deadline_and_durable_refusal_survive_capture_reopen() {
    let mut f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    f.begin(&mut ledger);
    f.ready(&mut ledger);
    f.reserve(&mut ledger).unwrap();
    f.state.conditions_valid_until_ms = START + 1_000;
    let check = ledger
        .check_geometry_capture_dispatch_deadline(
            "prep",
            "capture",
            f.state.clone(),
            &f.program.configuration,
            &f.constraints,
        )
        .unwrap();
    assert_eq!(check.latest_start_ms, Some(START + 999));
    f.state.now_ms = START + 1_000;
    let refused = ledger
        .check_geometry_capture_dispatch_deadline(
            "prep",
            "capture",
            f.state.clone(),
            &f.program.configuration,
            &f.constraints,
        )
        .unwrap();
    assert_eq!(refused.latest_start_ms, None);
    assert!(!matches!(refused.decision, Decision::Acquire { .. }));
    let events = ledger.preparation_events_after(0, 256).unwrap();
    drop(ledger);
    f.state.conditions_valid_until_ms = START + 120_000;
    let mut ledger = f.open(&path);
    let recovered = ledger
        .check_geometry_capture_dispatch_deadline(
            "prep",
            "capture",
            f.state.clone(),
            &f.program.configuration,
            &f.constraints,
        )
        .unwrap();
    assert_eq!(recovered, refused);
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap(), events);
    assert_eq!(
        ledger.attempt("capture").unwrap().unwrap().evidence,
        Evidence::Reserved
    );
}

#[test]
fn exact_capture_deadline_is_legal_but_next_millisecond_latches_refusal() {
    let mut f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let mut ledger = f.open(&dir.path().join("execution.sqlite"));
    f.begin(&mut ledger);
    f.ready(&mut ledger);
    f.reserve(&mut ledger).unwrap();
    f.state.now_ms = f.first_end() - 5_000;
    let check = ledger
        .check_geometry_capture_dispatch_deadline(
            "prep",
            "capture",
            f.state.clone(),
            &f.program.configuration,
            &f.constraints,
        )
        .unwrap();
    assert_eq!(check.latest_start_ms, Some(f.state.now_ms));
    f.state.now_ms += 1;
    let refused = ledger
        .check_geometry_capture_dispatch_deadline(
            "prep",
            "capture",
            f.state.clone(),
            &f.program.configuration,
            &f.constraints,
        )
        .unwrap();
    assert_eq!(refused.latest_start_ms, None);
    assert!(matches!(refused.decision, Decision::Wait { .. }));
}
