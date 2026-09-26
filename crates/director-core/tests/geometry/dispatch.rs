use super::*;

#[test]
fn pending_dispatch_rechecks_independent_meridian_windows() {
    let (mut program, mut request, mut constraints) = fixture();
    let transit = START + 30_000;
    let mut position = IcrsPosition {
        ra_degrees: 83.0,
        dec_degrees: -5.0,
    };
    for _ in 0..8 {
        let ha = observe(
            position,
            constraints.rig.site,
            constraints.rig.orientation,
            transit,
        )
        .unwrap()
        .hour_angle_degrees;
        position.ra_degrees = (position.ra_degrees + ha).rem_euclid(360.0);
    }
    program.targets[0].icrs_ra_mas =
        (position.ra_degrees * f64::from(MAS_PER_DEGREE)).round() as u32;
    constraints.rig.meridian_exclusion = MeridianExclusion {
        before_ms: 1_000,
        after_ms: 2_000,
    };
    request.state.meridian_exclusion = constraints.rig.meridian_exclusion;
    request.assignment.goals[0].exposure_ms = 5_000;
    request.assignment.goals[0].transits = Some(TransitCoverage {
        searched: interval(START - 2_000, START + 61_000),
        transits_ms: vec![],
    });
    program.assignment = request.assignment.clone();
    program.recipes[0].exposure_ms = 5_000;
    let local = local(&program);
    let bound = compile(program, &request, constraints.clone()).unwrap();
    let mut prep = bound
        .preparation(
            "prep".into(),
            &request,
            &constraints,
            "goal",
            local,
            Estimates::default(),
        )
        .unwrap();
    let Next::Run(command) = prep.next(&request, &constraints).unwrap() else {
        panic!()
    };
    request.state.now_ms = bound.windows("goal").unwrap()[0].end_ms - 4_999;
    assert!(matches!(evaluate(&request), Ok(Decision::Acquire { .. })));
    assert!(matches!(
        prep.check_pending_dispatch(&request, &constraints, &command),
        Ok(Decision::Wait { .. })
    ));
    assert_eq!(prep.pending(), Some(&command));
}

#[test]
fn slow_before_hook_cannot_dispatch_across_a_horizon_gap() {
    let (program, mut request, constraints) = preparation_fixture();
    let local = local(&program);
    let bound = compile(program, &request, constraints.clone()).unwrap();
    let mut prep = bound
        .preparation(
            "prep".into(),
            &request,
            &constraints,
            "goal",
            local,
            Estimates::default(),
        )
        .unwrap();
    let Next::Run(command) = prep.next(&request, &constraints).unwrap() else {
        panic!()
    };
    assert!(
        matches!(prep.check_pending_dispatch(&request, &constraints, &command), Ok(Decision::Acquire { goal_id, .. }) if goal_id == "goal")
    );
    request.state.now_ms += 18_000;
    // A normal poll still waits for evidence rather than interrupting an action.
    assert_eq!(
        prep.next(&request, &constraints).unwrap(),
        Next::InFlight { ordinal: 1 }
    );
    let decision = prep
        .check_pending_dispatch(&request, &constraints, &command)
        .unwrap();
    assert!(matches!(decision, Decision::Wait { .. }));
    assert_eq!(prep.pending(), Some(&command));
    assert!(prep.observations().is_empty());
    let mut restored = bound
        .restore_preparation(&prep.checkpoint().unwrap())
        .unwrap();
    request.state.now_ms = bound.windows("goal").unwrap()[1].start_ms;
    assert_eq!(
        restored
            .check_pending_dispatch(&request, &constraints, &command)
            .unwrap(),
        decision
    );
    let mut completion = receipt(&command, request.state.now_ms);
    completion.outcome = Outcome::Failed {
        reason: "not_dispatched".into(),
    };
    restored.complete(completion.clone()).unwrap();
    restored.complete(completion).unwrap();
    assert_eq!(restored.observations().len(), 1);
    assert_eq!(
        restored.next(&request, &constraints).unwrap(),
        Next::Decision(decision)
    );
}

#[test]
fn dispatch_check_includes_all_remaining_work_without_reissuing() {
    let (program, mut request, constraints) = preparation_fixture();
    let local = local(&program);
    let bound = compile(program, &request, constraints.clone()).unwrap();
    let estimates = Estimates {
        center_ms: 4_000,
        before_target_ms: 3_000,
        filter_ms: 2_000,
        readout_ms: 1_000,
        capture_overhead_ms: 1_000,
        ..Estimates::default()
    };
    let mut prep = bound
        .preparation(
            "prep".into(),
            &request,
            &constraints,
            "goal",
            local,
            estimates,
        )
        .unwrap();
    let Next::Run(command) = prep.next(&request, &constraints).unwrap() else {
        panic!()
    };
    let checkpoint = prep.checkpoint().unwrap();
    for _ in 0..3 {
        assert!(matches!(
            prep.check_pending_dispatch(&request, &constraints, &command),
            Ok(Decision::Acquire { .. })
        ));
        assert_eq!(prep.checkpoint().unwrap(), checkpoint);
    }
    request.state.now_ms += 7_000;
    // Exposure alone still fits, but all undispatched preparation no longer does.
    assert!(request.state.now_ms + 5_000 < bound.windows("goal").unwrap()[0].end_ms);
    assert!(matches!(
        prep.check_pending_dispatch(&request, &constraints, &command),
        Ok(Decision::Wait { .. })
    ));
    assert_eq!(prep.pending(), Some(&command));
}

#[test]
fn completed_steps_are_not_charged_again_at_dispatch() {
    let (program, mut request, constraints) = preparation_fixture();
    let local = local(&program);
    let bound = compile(program, &request, constraints.clone()).unwrap();
    let mut prep = bound
        .preparation(
            "prep".into(),
            &request,
            &constraints,
            "goal",
            local,
            Estimates {
                center_ms: 10_000,
                ..Estimates::default()
            },
        )
        .unwrap();
    let Next::Run(center) = prep.next(&request, &constraints).unwrap() else {
        panic!()
    };
    request.state.now_ms += 10_000;
    prep.complete(receipt(&center, request.state.now_ms))
        .unwrap();
    let Next::Run(before) = prep.next(&request, &constraints).unwrap() else {
        panic!()
    };
    assert_eq!(before.operation, Operation::BeforeTarget);
    assert!(matches!(
        prep.check_pending_dispatch(&request, &constraints, &before),
        Ok(Decision::Acquire { .. })
    ));
    assert_eq!(prep.observations().len(), 1);
}

#[test]
fn mismatched_or_absent_commands_do_not_mutate_preparation() {
    let (program, request, constraints) = fixture();
    let local = local(&program);
    let bound = compile(program, &request, constraints.clone()).unwrap();
    let mut prep = bound
        .preparation(
            "prep".into(),
            &request,
            &constraints,
            "goal",
            local,
            Estimates::default(),
        )
        .unwrap();
    let before_issue = prep.checkpoint().unwrap();
    let Next::Run(command) = prep.next(&request, &constraints).unwrap() else {
        panic!()
    };
    let mut not_issued = bound.restore_preparation(&before_issue).unwrap();
    assert!(not_issued
        .check_pending_dispatch(&request, &constraints, &command)
        .is_err());
    assert_eq!(not_issued.checkpoint().unwrap(), before_issue);
    let original = prep.checkpoint().unwrap();
    for fault in 0..6 {
        let mut wrong = command.clone();
        match fault {
            0 => wrong.preparation_id = "other".into(),
            1 => wrong.ordinal += 1,
            2 => wrong.goal_id = "other".into(),
            3 => wrong.target_id = "other".into(),
            4 => wrong.recipe_id = "other".into(),
            _ => wrong.operation = Operation::Unpark,
        }
        assert!(prep
            .check_pending_dispatch(&request, &constraints, &wrong)
            .is_err());
        assert_eq!(prep.checkpoint().unwrap(), original);
    }
    prep.complete(receipt(&command, request.state.now_ms))
        .unwrap();
    let completed = prep.checkpoint().unwrap();
    assert!(prep
        .check_pending_dispatch(&request, &constraints, &command)
        .is_err());
    assert_eq!(prep.checkpoint().unwrap(), completed);
}

#[test]
fn fresh_safety_conditions_expiry_and_constraint_content_gate_pending_dispatch() {
    let (program, request, constraints) = fixture();
    let local = local(&program);
    let bound = compile(program, &request, constraints.clone()).unwrap();
    for fault in 0..7 {
        let mut prep = bound
            .preparation(
                "prep".into(),
                &request,
                &constraints,
                "goal",
                local.clone(),
                Estimates::default(),
            )
            .unwrap();
        let Next::Run(command) = prep.next(&request, &constraints).unwrap() else {
            panic!()
        };
        let mut current = request.clone();
        let mut changed = constraints.clone();
        match fault {
            0 => current.state.safety = Safety::Unsafe,
            1 => current.state.operator_stop = true,
            2 => current.state.conditions_valid_until_ms = current.state.now_ms,
            3 => current.state.now_ms = current.assignment.expires_at_ms,
            4 => changed.rig.site.latitude_degrees += 1.0,
            5 => changed.rig.orientation.valid_until_ms -= 1,
            _ => {
                changed.rig.minimum_altitude_degrees += 1.0;
                current.state.safety = Safety::Unsafe;
            }
        }
        let result = prep
            .check_pending_dispatch(&current, &changed, &command)
            .unwrap();
        assert!(!matches!(
            result,
            Decision::Acquire { .. } | Decision::Continue { .. }
        ));
        if fault == 6 {
            assert!(matches!(result, Decision::Stop { .. }));
        }
        assert_eq!(prep.pending(), Some(&command));
        let mut restored = bound
            .restore_preparation(&prep.checkpoint().unwrap())
            .unwrap();
        // Clearing the condition cannot revive this preparation.
        current.state.safety = Safety::Safe;
        current.state.operator_stop = false;
        current.state.conditions_valid_until_ms += 120_000;
        assert_eq!(
            restored
                .check_pending_dispatch(&current, &constraints, &command)
                .unwrap(),
            result
        );
        restored
            .complete(receipt(&command, request.state.now_ms))
            .unwrap();
        assert_eq!(restored.observations().len(), 1);
    }
}

#[test]
fn nonboundary_is_not_a_dispatch_recommendation_and_clock_errors_do_not_latch() {
    let (program, mut request, constraints) = fixture();
    let local = local(&program);
    let bound = compile(program, &request, constraints.clone()).unwrap();
    let mut prep = bound
        .preparation(
            "prep".into(),
            &request,
            &constraints,
            "goal",
            local,
            Estimates::default(),
        )
        .unwrap();
    let Next::Run(command) = prep.next(&request, &constraints).unwrap() else {
        panic!()
    };
    request.state.at_boundary = false;
    request.state.now_ms += 100;
    assert!(matches!(
        prep.check_pending_dispatch(&request, &constraints, &command),
        Ok(Decision::Continue { .. })
    ));
    assert!(prep.halted().is_none());
    let saved = prep.checkpoint().unwrap();
    request.state.now_ms -= 1;
    let mut changed = constraints.clone();
    changed.rig.site.latitude_degrees += 1.0;
    assert!(matches!(
        prep.check_pending_dispatch(&request, &changed, &command),
        Err(Error::Preparation(
            psf_guard_director_core::preparation::Error::ClockRegression
        ))
    ));
    assert_eq!(saved, prep.checkpoint().unwrap());
    request.state.now_ms += 1;
    request.state.at_boundary = true;
    assert!(matches!(
        prep.check_pending_dispatch(&request, &constraints, &command),
        Ok(Decision::Acquire { .. })
    ));
}
