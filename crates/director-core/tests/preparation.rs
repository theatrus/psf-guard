use psf_guard_director_core::preparation::{
    Command, Completion, Context, Error, Estimates, Next, Operation, Outcome, Preparation,
};
use psf_guard_director_core::{Decision, Request, Safety};

fn request() -> Request {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/decisions.json")).unwrap();
    serde_json::from_value(fixture["base"].clone()).unwrap()
}

fn context() -> Context {
    Context {
        goal_id: "short-ha".into(),
        target_id: "target-1".into(),
        recipe_id: "recipe-1".into(),
        previous_target_id: None,
        filter_id: "ha".into(),
        readout_mode: 1,
        mount_parked: true,
        rotator_connected: true,
        enable_slew_center: true,
        dither_every: 3,
        dither_override: None,
        filter_exposures_since_dither: 3,
    }
}

fn preparation(r: &Request, c: Context) -> Preparation {
    Preparation::new("prep-1".into(), r, c, Estimates::default()).unwrap()
}

fn run(p: &mut Preparation, r: &Request) -> Command {
    let Next::Run(command) = p.next(r).unwrap() else {
        panic!("expected a native operation");
    };
    command
}

fn completion(command: &Command, now: u64, outcome: Outcome) -> Completion {
    Completion {
        preparation_id: command.preparation_id.clone(),
        ordinal: command.ordinal,
        ended_at_ms: now,
        elapsed_ms: 100,
        outcome,
    }
}

fn finish(p: &mut Preparation, r: &mut Request, command: &Command) {
    r.state.now_ms += 100;
    p.complete(completion(command, r.state.now_ms, Outcome::Succeeded))
        .unwrap();
}

fn operations(c: Context) -> Vec<Operation> {
    let mut r = request();
    let baseline = serde_json::to_value(&r.assignment).unwrap();
    let mut p = preparation(&r, c);
    let mut result = Vec::new();
    loop {
        match p.next(&r).unwrap() {
            Next::Run(command) => {
                assert_eq!(command.goal_id, "short-ha");
                assert_eq!(command.target_id, "target-1");
                result.push(command.operation.clone());
                finish(&mut p, &mut r, &command);
            }
            Next::ReadyToReserve { goal_id } => {
                assert_eq!(goal_id, "short-ha");
                // Preparation never consumes attempts or creates accepted images.
                assert_eq!(serde_json::to_value(&r.assignment).unwrap(), baseline);
                assert_eq!(p.observations().len(), result.len());
                return result;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[test]
fn new_target_uses_native_preparation_order_without_old_dither_history() {
    assert_eq!(
        operations(context()),
        vec![
            Operation::Unpark,
            Operation::Center { rotate: true },
            Operation::BeforeTarget,
            Operation::SwitchFilter {
                filter_id: "ha".into()
            },
            Operation::SetReadoutMode { mode: 1 },
        ]
    );
}

#[test]
fn slew_option_does_not_disable_before_target_hook() {
    let mut c = context();
    c.mount_parked = false;
    c.enable_slew_center = false;
    let result = operations(c.clone());
    assert_eq!(result.len(), 3);
    assert_eq!(result[0], Operation::BeforeTarget);
    c.enable_slew_center = true;
    c.rotator_connected = false;
    assert_eq!(operations(c)[0], Operation::Center { rotate: false });
}

#[test]
fn same_target_dithers_by_filter_cadence_and_recipe_override() {
    let mut c = context();
    c.previous_target_id = Some(c.target_id.clone());
    c.mount_parked = false;
    assert_eq!(operations(c.clone())[0], Operation::Dither);
    c.recipe_id = "another-recipe-same-filter".into();
    assert_eq!(operations(c.clone())[0], Operation::Dither);
    for count in [0, 1, 2] {
        c.filter_exposures_since_dither = count;
        assert_eq!(operations(c.clone()).len(), 2);
    }
    c.filter_exposures_since_dither = 3;
    c.dither_override = Some(0);
    assert_eq!(operations(c.clone()).len(), 2);
    c.dither_override = Some(4);
    assert_eq!(operations(c.clone()).len(), 2);
    c.filter_exposures_since_dither = 4;
    assert_eq!(operations(c)[0], Operation::Dither);
}

#[test]
fn commands_are_issued_once_and_receipts_are_idempotent() {
    let mut r = request();
    let mut p = preparation(&r, context());
    let command = run(&mut p, &r);
    assert_eq!(p.next(&r).unwrap(), Next::InFlight { ordinal: 1 });
    let receipt = completion(&command, r.state.now_ms, Outcome::Succeeded);
    p.complete(receipt.clone()).unwrap();
    p.complete(receipt.clone()).unwrap();
    assert_eq!(p.observations().len(), 1);
    let mut conflict = receipt.clone();
    conflict.elapsed_ms += 1;
    assert_eq!(p.complete(conflict), Err(Error::ConflictingCompletion));
    let next = run(&mut p, &r);
    assert_eq!(next.ordinal, 2);
    p.complete(receipt).unwrap();
    assert_eq!(p.next(&r).unwrap(), Next::InFlight { ordinal: 2 });
    finish(&mut p, &mut r, &next);
}

#[test]
fn delayed_receipt_preserves_latest_poll_time_and_observed_duration() {
    let mut r = request();
    let mut p = preparation(&r, context());
    let command = run(&mut p, &r);
    let mut receipt = completion(&command, r.state.now_ms + 50, Outcome::Succeeded);
    receipt.elapsed_ms = 7; // Never derive monotonic duration from wall time.
    r.state.now_ms += 100;
    assert_eq!(p.next(&r).unwrap(), Next::InFlight { ordinal: 1 });
    p.complete(receipt).unwrap();
    assert_eq!(p.observations()[0].completion.elapsed_ms, 7);
    r.state.now_ms -= 1;
    assert_eq!(p.next(&r), Err(Error::ClockRegression));
    r.state.now_ms += 1;
    assert_eq!(run(&mut p, &r).ordinal, 2);
}

#[test]
fn mismatched_or_invalid_receipts_leave_operation_in_flight() {
    let r = request();
    let mut p = preparation(&r, context());
    let command = run(&mut p, &r);
    let valid = completion(&command, r.state.now_ms, Outcome::Succeeded);
    let mut receipts = vec![valid.clone(); 4];
    receipts[0].preparation_id = "another-preparation".into();
    receipts[1].ordinal = 2;
    receipts[2].ended_at_ms -= 1;
    receipts[3].outcome = Outcome::Failed {
        reason: String::new(),
    };
    for invalid in receipts {
        assert_eq!(p.complete(invalid), Err(Error::InvalidCompletion));
        assert_eq!(p.next(&r).unwrap(), Next::InFlight { ordinal: 1 });
    }
    p.complete(valid).unwrap();
}

#[test]
fn slow_preparation_reselects_without_exposing_a_capture_permit() {
    let mut r = request();
    let mut p = preparation(&r, context());
    let command = run(&mut p, &r);
    r.state.now_ms = 80_000;
    p.complete(completion(&command, r.state.now_ms, Outcome::Succeeded))
        .unwrap();
    assert!(
        matches!(p.next(&r).unwrap(), Next::Decision(Decision::CheckIn { reason }) if reason == "preparation_goal_changed")
    );
    // This preparation can never be resumed for the old target.
    r.state.now_ms += 1;
    assert!(matches!(
        p.next(&r).unwrap(),
        Next::Decision(Decision::CheckIn { .. })
    ));
}

#[test]
fn remaining_estimates_are_removed_after_success_not_added_to_elapsed_time() {
    let mut r = request();
    let mut p = Preparation::new(
        "prep-1".into(),
        &r,
        context(),
        Estimates {
            unpark_ms: 40_000,
            ..Estimates::default()
        },
    )
    .unwrap();
    let command = run(&mut p, &r);
    r.state.now_ms += 40_000;
    p.complete(completion(&command, r.state.now_ms, Outcome::Succeeded))
        .unwrap();
    assert_eq!(
        run(&mut p, &r).operation,
        Operation::Center { rotate: true }
    );
}

#[test]
fn estimates_include_capture_overhead_and_checked_sums() {
    let r = request();
    assert!(matches!(
        Preparation::new(
            "prep-1".into(),
            &r,
            context(),
            Estimates {
                capture_overhead_ms: 61_000,
                ..Estimates::default()
            }
        ),
        Err(Error::NotSelected)
    ));
    assert!(matches!(
        Preparation::new(
            "prep-1".into(),
            &r,
            context(),
            Estimates {
                unpark_ms: u64::MAX,
                capture_overhead_ms: 1,
                ..Estimates::default()
            }
        ),
        Err(Error::EstimateOverflow)
    ));
}

#[test]
fn assignment_replacement_without_selected_goal_latches_until_boundary() {
    let mut r = request();
    let original = r.assignment.clone();
    let mut p = preparation(&r, context());
    let command = run(&mut p, &r);
    r.assignment.goals.remove(0);
    r.assignment.revision += 1;
    assert_eq!(p.next(&r).unwrap(), Next::InFlight { ordinal: 1 });
    // Reverting the input cannot undo the observed replacement.
    r.assignment = original;
    finish(&mut p, &mut r, &command);
    assert!(
        matches!(p.next(&r).unwrap(), Next::Decision(Decision::CheckIn { reason }) if reason == "preparation_assignment_changed")
    );
}

#[test]
fn configuration_change_is_not_forgotten_during_in_flight_operation() {
    let mut r = request();
    let mut p = preparation(&r, context());
    let command = run(&mut p, &r);
    r.state.configuration_id = "new-profile".into();
    assert_eq!(p.next(&r).unwrap(), Next::InFlight { ordinal: 1 });
    r.state.configuration_id = "config-1".into();
    finish(&mut p, &mut r, &command);
    assert!(
        matches!(p.next(&r).unwrap(), Next::Decision(Decision::CheckIn { reason }) if reason == "configuration_mismatch")
    );
}

#[test]
fn safety_and_operator_stop_override_pending_or_halted_check_in() {
    for safety in [Safety::Safe, Safety::Unsafe, Safety::Unknown] {
        let mut r = request();
        let mut p = preparation(&r, context());
        let command = run(&mut p, &r);
        r.assignment.revision += 1;
        assert_eq!(p.next(&r).unwrap(), Next::InFlight { ordinal: 1 });
        r.state.safety = safety;
        r.state.operator_stop = safety == Safety::Safe;
        assert!(matches!(
            p.next(&r).unwrap(),
            Next::Decision(Decision::Stop { .. })
        ));
        finish(&mut p, &mut r, &command);
        r.state.safety = Safety::Safe;
        r.state.operator_stop = false;
        assert!(matches!(
            p.next(&r).unwrap(),
            Next::Decision(Decision::Stop { .. })
        ));
    }
}

#[test]
fn failed_or_uncertain_operations_never_retry_or_advance() {
    for outcome in [
        Outcome::Failed {
            reason: "native_failure".into(),
        },
        Outcome::Uncertain {
            reason: "lost_connection".into(),
        },
    ] {
        let r = request();
        let mut p = preparation(&r, context());
        let command = run(&mut p, &r);
        p.complete(completion(&command, r.state.now_ms, outcome))
            .unwrap();
        assert!(matches!(
            p.next(&r).unwrap(),
            Next::Decision(Decision::CheckIn { .. })
        ));
        assert!(matches!(
            p.next(&r).unwrap(),
            Next::Decision(Decision::CheckIn { .. })
        ));
        assert_eq!(p.observations().len(), 1);
    }
}

#[test]
fn stale_conditions_and_assignment_expiry_block_next_operation() {
    for expiry in [false, true] {
        let mut r = request();
        let mut p = preparation(&r, context());
        let command = run(&mut p, &r);
        if expiry {
            r.state.now_ms = r.assignment.expires_at_ms;
        } else {
            r.state.conditions_valid_until_ms = r.state.now_ms;
        }
        assert_eq!(p.next(&r).unwrap(), Next::InFlight { ordinal: 1 });
        finish(&mut p, &mut r, &command);
        assert!(matches!(
            p.next(&r).unwrap(),
            Next::Decision(Decision::CheckIn { .. })
        ));
    }
}

#[test]
fn invalid_inputs_do_not_advance_time_or_issue_operations() {
    let r = request();
    let mut p = preparation(&r, context());
    let mut invalid = r.clone();
    invalid.state.now_ms += 1000;
    invalid.contract_version = 0;
    assert!(matches!(p.next(&invalid), Err(Error::Core(_))));
    assert_eq!(run(&mut p, &r).ordinal, 1);
    let mut c = context();
    c.readout_mode = -1;
    assert!(matches!(
        Preparation::new("prep-1".into(), &r, c, Estimates::default()),
        Err(Error::InvalidContext)
    ));
}

#[test]
fn nested_native_action_blocks_next_step_until_its_boundary() {
    let mut r = request();
    let mut p = preparation(&r, context());
    let command = run(&mut p, &r);
    finish(&mut p, &mut r, &command);
    r.state.at_boundary = false;
    assert!(matches!(
        p.next(&r).unwrap(),
        Next::Decision(Decision::Continue { .. })
    ));
    r.state.at_boundary = true;
    assert_eq!(run(&mut p, &r).ordinal, 2);
}

#[test]
fn readiness_is_rechecked_and_never_a_stored_dispatch_permit() {
    let mut r = request();
    let mut c = context();
    c.previous_target_id = Some(c.target_id.clone());
    c.mount_parked = false;
    c.dither_override = Some(0);
    let mut p = preparation(&r, c);
    for _ in 0..2 {
        let command = run(&mut p, &r);
        finish(&mut p, &mut r, &command);
    }
    assert_eq!(
        p.next(&r).unwrap(),
        Next::ReadyToReserve {
            goal_id: "short-ha".into()
        }
    );
    r.state.conditions_valid_until_ms = r.state.now_ms;
    assert!(
        matches!(p.next(&r).unwrap(), Next::Decision(Decision::CheckIn { reason }) if reason == "conditions_stale")
    );
}

#[test]
fn preparations_cannot_start_at_an_in_flight_or_unsafe_boundary() {
    let mut r = request();
    r.state.at_boundary = false;
    assert!(matches!(
        Preparation::new("prep-1".into(), &r, context(), Estimates::default()),
        Err(Error::NotSelected)
    ));
    r.state.at_boundary = true;
    r.state.safety = Safety::Unknown;
    assert!(matches!(
        Preparation::new("prep-1".into(), &r, context(), Estimates::default()),
        Err(Error::NotSelected)
    ));
}

#[test]
fn checkpoints_restore_each_boundary_without_reissuing_commands() {
    let mut r = request();
    let mut p = preparation(&r, context());
    loop {
        let bytes = p.checkpoint().unwrap();
        p = Preparation::restore(&bytes).unwrap();
        assert_eq!(p.checkpoint().unwrap(), bytes);
        match p.next(&r).unwrap() {
            Next::Run(command) => {
                p = Preparation::restore(&p.checkpoint().unwrap()).unwrap();
                assert_eq!(
                    p.next(&r).unwrap(),
                    Next::InFlight {
                        ordinal: command.ordinal
                    }
                );
                finish(&mut p, &mut r, &command);
            }
            Next::ReadyToReserve { .. } => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(p.steps_completed());
    assert_eq!(p.observations().len(), 5);
}

#[test]
fn checkpoints_preserve_stops_in_flight_and_delayed_receipt_clock() {
    let mut r = request();
    let mut p = preparation(&r, context());
    let command = run(&mut p, &r);
    r.state.now_ms += 100;
    r.state.operator_stop = true;
    p.next(&r).unwrap();
    p = Preparation::restore(&p.checkpoint().unwrap()).unwrap();
    p.complete(completion(
        &command,
        r.state.now_ms - 50,
        Outcome::Succeeded,
    ))
    .unwrap();
    p = Preparation::restore(&p.checkpoint().unwrap()).unwrap();
    r.state.operator_stop = false;
    assert!(matches!(
        p.next(&r).unwrap(),
        Next::Decision(Decision::Stop { .. })
    ));
    r.state.now_ms -= 1;
    assert_eq!(p.next(&r), Err(Error::ClockRegression));
}

#[test]
fn malformed_checkpoint_cannot_create_readiness_or_erase_failed_evidence() {
    let r = request();
    let mut p = preparation(&r, context());
    let command = run(&mut p, &r);
    p.complete(completion(
        &command,
        r.state.now_ms,
        Outcome::Failed {
            reason: "native_failure".into(),
        },
    ))
    .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&p.checkpoint().unwrap()).unwrap();
    for (pointer, replacement) in [
        ("/format_version", serde_json::json!(99)),
        ("/engine_version", serde_json::json!("future")),
        ("/halted", serde_json::Value::Null),
        (
            "/halted",
            serde_json::json!({"action":"acquire","goal_id":"short-ha","reason":"forged"}),
        ),
        ("/last_time_ms", serde_json::json!(0)),
        ("/observations/0/command/ordinal", serde_json::json!(2)),
        (
            "/observations/0/command/goal_id",
            serde_json::json!("other"),
        ),
        ("/observations/0/issued_at_ms", serde_json::json!(0)),
        ("/pending_issued_at_ms", serde_json::json!(10000)),
        ("/initial/contract_version", serde_json::json!(99)),
    ] {
        let mut invalid = value.clone();
        *invalid.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            matches!(
                Preparation::restore(&serde_json::to_vec(&invalid).unwrap()),
                Err(Error::InvalidCheckpoint)
            ),
            "{pointer}"
        );
    }
    for field in ["pending_issued_at_ms", "halted"] {
        let mut invalid = value.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(matches!(
            Preparation::restore(&serde_json::to_vec(&invalid).unwrap()),
            Err(Error::InvalidCheckpoint)
        ));
    }
    let mut invalid = value;
    invalid["extra"] = serde_json::json!(true);
    assert!(matches!(
        Preparation::restore(&serde_json::to_vec(&invalid).unwrap()),
        Err(Error::InvalidCheckpoint)
    ));
    assert!(matches!(
        Preparation::restore(&vec![0; 400_000]),
        Err(Error::InvalidCheckpoint)
    ));
}
