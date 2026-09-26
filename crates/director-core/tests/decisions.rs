use psf_guard_director_core::*;
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/decisions.json")).unwrap()
}

fn base() -> Request {
    serde_json::from_value(fixture()["base"].clone()).unwrap()
}

fn merge(value: &mut Value, patch: &Value) {
    if let (Some(target), Some(source)) = (value.as_object_mut(), patch.as_object()) {
        for (key, item) in source {
            merge(target.entry(key).or_insert(Value::Null), item);
        }
    } else {
        *value = patch.clone();
    }
}

#[test]
fn shared_cross_host_vectors() {
    let fixture = fixture();
    for case in fixture["cases"].as_array().unwrap() {
        let mut input = fixture["base"].clone();
        merge(&mut input, &case["patch"]);
        let response = evaluate_json(&serde_json::to_vec(&input).unwrap());
        assert_eq!(response.contract_version, CONTRACT_VERSION);
        assert_eq!(response.engine_version, ENGINE_VERSION);
        assert_eq!(
            serde_json::to_value(response.outcome).unwrap(),
            case["expected"],
            "{}",
            case["name"]
        );
    }
}

#[test]
fn pending_grades_reserve_work_without_completing_objectives() {
    let mut r = base();
    for g in &mut r.assignment.goals {
        g.pending = g.requested - g.accepted;
    }
    assert!(
        matches!(evaluate(&r), Ok(Decision::Wait { reason }) if reason == "pending_assessment")
    );
    for g in &mut r.assignment.goals {
        g.accepted = g.requested;
        g.pending = 0;
    }
    assert!(matches!(evaluate(&r), Ok(Decision::Complete { .. })));
}

#[test]
fn rejected_work_cannot_bypass_attempt_limits() {
    let mut r = base();
    for g in &mut r.assignment.goals {
        g.accepted = 0;
        g.pending = 0;
        g.attempts_remaining = 0;
    }
    assert!(matches!(evaluate(&r), Ok(Decision::CheckIn { .. })));
}

#[test]
fn pending_counts_do_not_overflow() {
    let mut r = base();
    for g in &mut r.assignment.goals {
        g.accepted = 1;
        g.pending = u32::MAX;
    }
    assert!(
        matches!(evaluate(&r), Ok(Decision::Wait { reason }) if reason == "pending_assessment")
    );
}

#[test]
fn priority_ties_use_stable_identity_not_input_order() {
    let mut r = base();
    for g in &mut r.assignment.goals {
        g.priority = 1;
    }
    let first = evaluate(&r).unwrap();
    r.assignment.goals.reverse();
    assert_eq!(evaluate(&r).unwrap(), first);
    assert!(matches!(first, Decision::Acquire { goal_id, .. } if goal_id == "long-ha"));
}

#[test]
fn future_windows_wait_only_if_exposure_can_fit() {
    let mut r = base();
    for g in &mut r.assignment.goals {
        g.eligible_from_ms = 80000;
    }
    assert!(matches!(evaluate(&r), Ok(Decision::Wait { reason }) if reason == "future_window"));
    r.assignment.expires_at_ms = 85000;
    assert!(matches!(evaluate(&r), Ok(Decision::CheckIn { .. })));
}

#[test]
fn invalid_or_duplicate_goals_fail_closed() {
    let mut r = base();
    r.assignment.goals.push(r.assignment.goals[0].clone());
    assert_eq!(evaluate(&r), Err(Error::DuplicateGoal));
    let mut r = base();
    r.assignment.goals[0].exposure_ms = 0;
    assert_eq!(evaluate(&r), Err(Error::InvalidGoal));
    r.assignment.goals[0].exposure_ms = u64::MAX;
    assert_eq!(evaluate(&r), Err(Error::InvalidGoal));
}

#[test]
fn near_timestamp_limit_does_not_wrap_to_feasible_work() {
    let mut r = base();
    r.assignment.expires_at_ms = u64::MAX;
    r.state.now_ms = u64::MAX - 1;
    r.state.conditions_valid_until_ms = u64::MAX;
    for g in &mut r.assignment.goals {
        g.eligible_until_ms = u64::MAX;
    }
    assert!(
        matches!(evaluate(&r), Ok(Decision::CheckIn { reason }) if reason == "no_authorized_feasible_work")
    );
}

#[test]
fn malformed_and_oversized_json_are_errors() {
    for bytes in [b"{".as_slice(), b"\xff".as_slice(), b"null".as_slice()] {
        assert_eq!(
            evaluate_json(bytes).outcome,
            Outcome::Error {
                code: Error::InvalidJson
            }
        );
    }
    assert_eq!(
        evaluate_json(&vec![b' '; MAX_REQUEST_BYTES + 1]).outcome,
        Outcome::Error {
            code: Error::RequestTooLarge
        }
    );
}

#[test]
fn responses_identify_the_evaluated_revision() {
    let r = base();
    let response = evaluate_json(&serde_json::to_vec(&r).unwrap());
    assert_eq!(response.assignment_id.as_deref(), Some("assignment-1"));
    assert_eq!(response.assignment_revision, Some(1));
}
