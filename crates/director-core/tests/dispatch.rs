use psf_guard_director_core::{dispatch::evaluate_dispatch, windows::*, *};

fn request() -> Request {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/decisions.json")).unwrap();
    let mut request: Request = serde_json::from_value(fixture["base"].clone()).unwrap();
    request.assignment.goals.truncate(1);
    request
}

#[test]
fn latest_start_includes_all_overhead_and_allows_exact_finish() {
    let mut request = request();
    let checked = evaluate_dispatch(&request).unwrap();
    assert_eq!(checked.evaluated_at_ms, 10_000);
    assert_eq!(checked.latest_start_ms, Some(60_000));
    request.state.now_ms = checked.latest_start_ms.unwrap();
    assert!(matches!(evaluate(&request), Ok(Decision::Acquire { .. })));
    request.state.now_ms += 1;
    assert!(!matches!(evaluate(&request), Ok(Decision::Acquire { .. })));
}

#[test]
fn bound_stops_at_current_window_not_later_work_after_gap() {
    let mut request = request();
    request.assignment.goals[0].eligible_windows.push(Interval {
        start_ms: 200_000,
        end_ms: 500_000,
    });
    assert_eq!(
        evaluate_dispatch(&request).unwrap().latest_start_ms,
        Some(60_000)
    );
    request.state.now_ms = 200_000;
    assert_eq!(
        evaluate_dispatch(&request).unwrap().latest_start_ms,
        Some(460_000)
    );
}

#[test]
fn assignment_expiry_and_exclusive_condition_expiry_bound_dispatch() {
    let mut request = request();
    request.assignment.expires_at_ms = 80_000;
    assert_eq!(
        evaluate_dispatch(&request).unwrap().latest_start_ms,
        Some(40_000)
    );
    request.state.conditions_valid_until_ms = 20_000;
    assert_eq!(
        evaluate_dispatch(&request).unwrap().latest_start_ms,
        Some(19_999)
    );
    request.state.now_ms = 20_000;
    assert_eq!(evaluate_dispatch(&request).unwrap().latest_start_ms, None);
}

#[test]
fn meridian_exclusion_shortens_latest_start_without_bridging() {
    let mut request = request();
    request.state.meridian_exclusion = MeridianExclusion {
        before_ms: 5_000,
        after_ms: 10_000,
    };
    request.assignment.goals[0].transits = Some(TransitCoverage {
        searched: Interval {
            start_ms: 0,
            end_ms: 1_005_000,
        },
        transits_ms: vec![60_000],
    });
    // Keep pre-assignment search representable with the after margin.
    request.assignment.valid_from_ms = 10_000;
    assert_eq!(
        evaluate_dispatch(&request).unwrap().latest_start_ms,
        Some(15_000)
    );
}

#[test]
fn non_acquisition_decisions_never_have_dispatch_time() {
    for variant in 0..6 {
        let mut request = request();
        match variant {
            0 => request.state.safety = Safety::Unsafe,
            1 => request.state.operator_stop = true,
            2 => request.state.at_boundary = false,
            3 => request.assignment.goals[0].pending = 8,
            4 => request.assignment.goals[0].accepted = 10,
            _ => request.state.configuration_id = "other".into(),
        }
        let checked = evaluate_dispatch(&request).unwrap();
        assert!(!matches!(checked.decision, Decision::Acquire { .. }));
        assert_eq!(checked.latest_start_ms, None);
    }
}

#[test]
fn zero_slack_and_maximum_time_remain_exact() {
    let mut request = request();
    request.assignment.valid_from_ms = u64::MAX - 40_000;
    request.assignment.expires_at_ms = u64::MAX;
    request.assignment.goals[0].eligible_windows = vec![Interval {
        start_ms: u64::MAX - 40_000,
        end_ms: u64::MAX,
    }];
    request.state.now_ms = u64::MAX - 40_000;
    request.state.conditions_valid_until_ms = u64::MAX;
    assert_eq!(
        evaluate_dispatch(&request).unwrap().latest_start_ms,
        Some(request.state.now_ms)
    );
    request.state.now_ms += 1;
    assert_eq!(evaluate_dispatch(&request).unwrap().latest_start_ms, None);
}

#[test]
fn invalid_duration_and_windows_do_not_produce_a_bound() {
    let mut request = request();
    request.assignment.goals[0].overhead_ms = u64::MAX;
    assert!(evaluate_dispatch(&request).is_err());
    request.assignment.goals[0].overhead_ms = 0;
    request.assignment.goals[0].eligible_windows[0].start_ms = 200_000;
    assert!(evaluate_dispatch(&request).is_err());
}
