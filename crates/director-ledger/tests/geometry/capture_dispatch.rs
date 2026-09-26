use super::*;
use psf_guard_director_ledger::preparation::EventKind;

fn reserve(f: &Fixture, ledger: &mut Ledger) {
    f.begin(ledger);
    f.ready(ledger);
    assert!(matches!(f.reserve(ledger), Ok(Reservation::Created(_))));
}

fn check(f: &Fixture, ledger: &mut Ledger) -> Result<Decision, Error> {
    ledger.check_geometry_capture_dispatch(
        "prep",
        "capture",
        f.state.clone(),
        &f.program.configuration,
        &f.constraints,
    )
}

#[test]
fn checking_last_reserved_attempt_does_not_spend_or_restore_credit() {
    let mut f = Fixture::new();
    f.program.assignment.goals[0].attempts_remaining = 1;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    reserve(&f, &mut ledger);
    let record = ledger.preparation("prep").unwrap();
    let captures = ledger.events_after(0, 256).unwrap();
    let preparations = ledger.preparation_events_after(0, 256).unwrap();
    for _ in 0..3 {
        assert!(
            matches!(check(&f, &mut ledger), Ok(Decision::Acquire { goal_id, .. }) if goal_id == "short-ha")
        );
    }
    assert_eq!(ledger.preparation("prep").unwrap(), record);
    assert_eq!(ledger.events_after(0, 256).unwrap(), captures);
    assert_eq!(
        ledger.preparation_events_after(0, 256).unwrap(),
        preparations
    );
    assert!(matches!(
        f.reserve(&mut ledger),
        Ok(Reservation::Existing(_))
    ));
    assert!(!matches!(
        ledger.evaluate_geometry(f.state.clone(), &f.constraints),
        Ok(Decision::Acquire { .. })
    ));
    assert!(ledger
        .begin_geometry_preparation(
            "another",
            "short-ha",
            f.local(),
            Estimates::default(),
            f.state.clone(),
            &f.constraints
        )
        .is_err());
    ledger
        .record(
            "capture",
            Evidence::Failed {
                reason: "not_dispatched".into(),
            },
        )
        .unwrap();
    assert!(check(&f, &mut ledger).is_err());
    assert!(!matches!(
        ledger.evaluate_geometry(f.state.clone(), &f.constraints),
        Ok(Decision::Acquire { .. })
    ));
    assert!(ledger
        .begin_geometry_preparation(
            "another",
            "short-ha",
            f.local(),
            Estimates::default(),
            f.state.clone(),
            &f.constraints
        )
        .is_err());
}

#[test]
fn slow_before_exposure_hook_cannot_cross_horizon_and_refusal_survives_reopen() {
    let mut f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    reserve(&f, &mut ledger);
    let events = ledger.preparation_events_after(0, 256).unwrap();
    f.state.now_ms = f.first_end() - 4_999;
    let refusal = check(&f, &mut ledger).unwrap();
    assert!(matches!(refusal, Decision::Wait { .. }));
    drop(ledger);
    let mut ledger = f.open(&path);
    f.state.now_ms = START + 30_000;
    assert_eq!(check(&f, &mut ledger).unwrap(), refusal);
    assert_eq!(
        ledger.preparation_events_after(0, 256).unwrap().len(),
        events.len() + 1
    );
    assert_eq!(
        ledger.attempt("capture").unwrap().unwrap().evidence,
        Evidence::Reserved
    );
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 1);
    // A delayed save receipt remains evidence even after a refused boundary.
    ledger
        .record(
            "capture",
            Evidence::Saved {
                image_id: "image".into(),
                elapsed_ms: 5000,
            },
        )
        .unwrap();
    assert!(check(&f, &mut ledger).is_err());
    assert!(matches!(
        ledger
            .capture_binding("capture")
            .unwrap()
            .unwrap()
            .attempt
            .evidence,
        Evidence::Saved { .. }
    ));
}

#[test]
fn final_check_accounts_for_capture_overhead_but_not_completed_preparation() {
    let mut f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    ledger
        .begin_geometry_preparation(
            "prep",
            "short-ha",
            f.local(),
            Estimates {
                center_ms: 10_000,
                capture_overhead_ms: 2_000,
                ..Estimates::default()
            },
            f.state.clone(),
            &f.constraints,
        )
        .unwrap();
    f.ready(&mut ledger);
    f.state.now_ms += 10_000;
    assert!(matches!(
        f.reserve(&mut ledger),
        Ok(Reservation::Created(_))
    ));
    assert!(matches!(
        check(&f, &mut ledger),
        Ok(Decision::Acquire { .. })
    ));
    f.state.now_ms = f.first_end() - 6_999;
    assert!(matches!(check(&f, &mut ledger), Ok(Decision::Wait { .. })));
}

#[test]
fn safety_expiry_and_full_constraint_content_are_checked_after_reservation() {
    for fault in 0..6 {
        let f = Fixture::new();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("execution.sqlite");
        let mut ledger = f.open(&path);
        reserve(&f, &mut ledger);
        let mut changed = f.clone();
        match fault {
            0 => changed.state.safety = Safety::Unsafe,
            1 => changed.state.operator_stop = true,
            2 => changed.state.conditions_valid_until_ms = START,
            3 => changed.state.now_ms = f.program.assignment.expires_at_ms,
            4 => changed.constraints.rig.site.latitude_degrees += 1.0,
            _ => changed.constraints.rig.orientation.valid_until_ms -= 1,
        }
        let refusal = check(&changed, &mut ledger).unwrap();
        assert!(!matches!(
            refusal,
            Decision::Acquire { .. } | Decision::Continue { .. }
        ));
        let mut second = f.open(&path);
        let mut recovered = f.clone();
        recovered.state.now_ms = changed.state.now_ms;
        assert_eq!(check(&recovered, &mut second).unwrap(), refusal);
        recovered.state.safety = Safety::Unknown;
        assert!(matches!(
            check(&recovered, &mut second),
            Ok(Decision::Stop { .. })
        ));
        assert_eq!(
            ledger.attempt("capture").unwrap().unwrap().evidence,
            Evidence::Reserved
        );
    }
}

#[test]
fn wrong_links_modes_configuration_and_terminal_evidence_cannot_pass() {
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    f.begin(&mut ledger);
    assert!(check(&f, &mut ledger).is_err());
    f.ready(&mut ledger);
    assert!(check(&f, &mut ledger).is_err());
    f.reserve(&mut ledger).unwrap();
    let before = ledger.preparation_events_after(0, 256).unwrap();
    for id in ["", "missing", "other"] {
        assert!(ledger
            .check_geometry_capture_dispatch(
                id,
                "capture",
                f.state.clone(),
                &f.program.configuration,
                &f.constraints
            )
            .is_err());
        assert!(ledger
            .check_geometry_capture_dispatch(
                "prep",
                id,
                f.state.clone(),
                &f.program.configuration,
                &f.constraints
            )
            .is_err());
    }
    assert!(ledger
        .check_prepared_capture_dispatch("prep", "capture", f.state.clone())
        .is_err());
    assert!(ledger
        .check_program_capture_dispatch(
            "prep",
            "capture",
            f.state.clone(),
            &f.program.configuration
        )
        .is_err());
    let mut changed = f.clone();
    changed.program.configuration.id = "wrong".into();
    assert!(check(&changed, &mut ledger).is_err());
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap(), before);
    ledger
        .record(
            "capture",
            Evidence::Uncertain {
                reason: "lost_receipt".into(),
            },
        )
        .unwrap();
    assert!(check(&f, &mut ledger).is_err());
    ledger
        .record(
            "capture",
            Evidence::Saved {
                image_id: "verified-image".into(),
                elapsed_ms: 5000,
            },
        )
        .unwrap();
    assert!(check(&f, &mut ledger).is_err());
}

#[test]
fn halt_event_failure_rolls_back_snapshot_clock_and_refusal() {
    let mut f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    reserve(&f, &mut ledger);
    let before = ledger.preparation("prep").unwrap();
    let events = ledger.preparation_events_after(0, 256).unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_halt BEFORE INSERT ON preparation_event BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
    f.state.now_ms = f.first_end() - 4_999;
    assert!(check(&f, &mut ledger).is_err());
    drop(ledger);
    let mut ledger = f.open(&path);
    assert_eq!(ledger.preparation("prep").unwrap(), before);
    assert_eq!(ledger.preparation_events_after(0, 256).unwrap(), events);
    db.execute_batch("DROP TRIGGER fail_halt;").unwrap();
    f.state.now_ms = START;
    assert!(matches!(
        check(&f, &mut ledger),
        Ok(Decision::Acquire { .. })
    ));
}

#[test]
fn final_check_persists_clock_without_treating_continue_as_readiness() {
    let mut f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    reserve(&f, &mut ledger);
    f.state.now_ms += 100;
    f.state.at_boundary = false;
    assert!(matches!(
        check(&f, &mut ledger),
        Ok(Decision::Continue { .. })
    ));
    drop(ledger);
    let mut ledger = f.open(&path);
    f.state.now_ms -= 1;
    f.state.at_boundary = true;
    assert!(check(&f, &mut ledger).is_err());
    f.state.now_ms += 1;
    assert!(matches!(
        check(&f, &mut ledger),
        Ok(Decision::Acquire { .. })
    ));
    assert!(!ledger
        .preparation_events_after(0, 256)
        .unwrap()
        .iter()
        .any(|e| matches!(e.event, EventKind::Halted { .. })));
}

#[test]
fn program_mode_uses_its_reservation_and_refuses_geometry_adoption() {
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
    loop {
        match ledger
            .advance_program_preparation("prep", f.state.clone(), &f.program.configuration)
            .unwrap()
        {
            Next::Run(command) => f.finish(&mut ledger, command.ordinal),
            Next::ReadyToReserve { .. } => break,
            other => panic!("{other:?}"),
        }
    }
    ledger
        .reserve_program_prepared("prep", "capture", f.state.clone(), &f.program.configuration)
        .unwrap();
    assert!(check(&f, &mut ledger).is_err());
    assert!(ledger
        .check_prepared_capture_dispatch("prep", "capture", f.state.clone())
        .is_err());
    assert!(matches!(
        ledger.check_program_capture_dispatch(
            "prep",
            "capture",
            f.state.clone(),
            &f.program.configuration
        ),
        Ok(Decision::Acquire { .. })
    ));
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 1);
}

#[test]
fn computed_meridian_window_still_gates_an_already_reserved_exposure() {
    let mut f = Fixture::new();
    let transit = START + 30_000;
    let mut position = IcrsPosition {
        ra_degrees: 83.0,
        dec_degrees: -5.0,
    };
    for _ in 0..8 {
        let ha = observe(
            position,
            f.constraints.rig.site,
            f.constraints.rig.orientation,
            transit,
        )
        .unwrap()
        .hour_angle_degrees;
        position.ra_degrees = (position.ra_degrees + ha).rem_euclid(360.0);
    }
    f.program.targets[0].icrs_ra_mas =
        (position.ra_degrees * f64::from(MAS_PER_DEGREE)).round() as u32;
    f.constraints.rig.horizon = Horizon::FixedMinimum {};
    f.constraints.rig.orientation.valid_from_ms = START - 3_000;
    f.constraints.rig.meridian_exclusion = MeridianExclusion {
        before_ms: 1000,
        after_ms: 2000,
    };
    f.state.meridian_exclusion = f.constraints.rig.meridian_exclusion;
    f.program.assignment.goals[0].transits = Some(TransitCoverage {
        searched: Interval {
            start_ms: START - 2000,
            end_ms: START + 61000,
        },
        transits_ms: vec![],
    });
    let dir = TempDir::new().unwrap();
    let mut ledger = f.open(&dir.path().join("execution.sqlite"));
    reserve(&f, &mut ledger);
    f.state.now_ms = f.first_end() - 4_999;
    assert!(matches!(check(&f, &mut ledger), Ok(Decision::Wait { .. })));
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 1);
}

#[test]
fn capture_dispatch_crash_child() {
    let Some(path) = std::env::var_os("DIRECTOR_CAPTURE_DISPATCH_CRASH_DB") else {
        return;
    };
    let mut f = Fixture::new();
    let mut ledger = f.open(Path::new(&path));
    reserve(&f, &mut ledger);
    f.state.now_ms = f.first_end() - 4_999;
    assert!(matches!(check(&f, &mut ledger), Ok(Decision::Wait { .. })));
    std::process::exit(83);
}

#[test]
fn only_the_linked_reserved_attempt_is_restored_in_the_temporary_projection() {
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let mut ledger = f.open(&dir.path().join("execution.sqlite"));
    reserve(&f, &mut ledger);
    ledger
        .record(
            "capture",
            Evidence::Failed {
                reason: "not_dispatched".into(),
            },
        )
        .unwrap();
    ledger
        .begin_geometry_preparation(
            "next",
            "short-ha",
            f.local(),
            Estimates::default(),
            f.state.clone(),
            &f.constraints,
        )
        .unwrap();
    loop {
        match ledger
            .advance_geometry_preparation(
                "next",
                f.state.clone(),
                &f.program.configuration,
                &f.constraints,
            )
            .unwrap()
        {
            Next::Run(command) => {
                ledger
                    .complete_preparation(Completion {
                        preparation_id: "next".into(),
                        ordinal: command.ordinal,
                        ended_at_ms: START,
                        elapsed_ms: 0,
                        outcome: Outcome::Succeeded,
                    })
                    .unwrap();
            }
            Next::ReadyToReserve { .. } => break,
            other => panic!("{other:?}"),
        }
    }
    ledger
        .reserve_geometry_prepared(
            "next",
            "next-capture",
            f.state.clone(),
            &f.program.configuration,
            &f.constraints,
        )
        .unwrap();
    assert!(matches!(
        ledger.check_geometry_capture_dispatch(
            "next",
            "next-capture",
            f.state.clone(),
            &f.program.configuration,
            &f.constraints
        ),
        Ok(Decision::Acquire { .. })
    ));
    assert!(ledger
        .check_geometry_capture_dispatch(
            "next",
            "capture",
            f.state.clone(),
            &f.program.configuration,
            &f.constraints
        )
        .is_err());
    assert!(check(&f, &mut ledger).is_err());
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 3);
    ledger
        .record(
            "next-capture",
            Evidence::Failed {
                reason: "not_dispatched".into(),
            },
        )
        .unwrap();
    assert!(!matches!(
        ledger.evaluate_geometry(f.state.clone(), &f.constraints),
        Ok(Decision::Acquire { .. })
    ));
}

#[test]
fn abrupt_exit_preserves_refusal_and_capture_evidence_without_replaying() {
    let mut f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "capture_dispatch::capture_dispatch_crash_child"])
        .env("DIRECTOR_CAPTURE_DISPATCH_CRASH_DB", &path)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(83));
    let mut ledger = f.open(&path);
    f.state.now_ms = START + 30_000;
    assert!(matches!(check(&f, &mut ledger), Ok(Decision::Wait { .. })));
    assert!(matches!(
        f.reserve(&mut ledger),
        Ok(Reservation::Existing(_))
    ));
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 1);
    let halt_count = ledger
        .preparation_events_after(0, 256)
        .unwrap()
        .iter()
        .filter(|event| matches!(event.event, EventKind::Halted { .. }))
        .count();
    assert_eq!(halt_count, 1);
}
