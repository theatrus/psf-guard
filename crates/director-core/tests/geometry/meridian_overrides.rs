use super::*;

fn at_transit() -> (Program, Request, Constraints) {
    let (mut program, mut request, mut constraints) = fixture();
    request.assignment.expires_at_ms = START + 3_600_000;
    request.assignment.goals[0].eligible_windows = vec![interval(START, START + 3_600_000)];
    request.assignment.goals[0].transits = None;
    request.state.conditions_valid_until_ms = START + 3_600_001;
    constraints.rig.orientation.valid_from_ms = START - 21_600_000;
    constraints.rig.orientation.valid_until_ms = START + 25_200_001;
    let mut position = IcrsPosition {
        ra_degrees: 83.0,
        dec_degrees: -5.0,
    };
    for _ in 0..8 {
        let ha = observe(
            position,
            constraints.rig.site,
            constraints.rig.orientation,
            START + 1_800_000,
        )
        .unwrap()
        .hour_angle_degrees;
        position.ra_degrees = (position.ra_degrees + ha).rem_euclid(360.0);
    }
    program.targets[0].icrs_ra_mas =
        (position.ra_degrees * f64::from(MAS_PER_DEGREE)).round() as u32;
    constraints.rig.meridian_exclusion = MeridianExclusion {
        before_ms: 3_600_000,
        after_ms: 0,
    };
    request.state.meridian_exclusion = constraints.rig.meridian_exclusion;
    program.assignment = request.assignment.clone();
    (program, request, constraints)
}

#[test]
fn server_program_without_transit_list_uses_computed_session_avoidance() {
    let (program, mut request, constraints) = at_transit();
    // Binding intent is allowed; a non-geometric executor still cannot dispatch.
    assert_eq!(
        evaluate(&request),
        Err(psf_guard_director_core::Error::IncompleteTransitCoverage)
    );
    let bound = compile(program, &request, constraints.clone()).unwrap();
    assert!(!matches!(
        bound.evaluate(&request, &constraints).unwrap(),
        Decision::Acquire { .. }
    ));
    request.state.now_ms = START + 1_801_000;
    assert!(matches!(
        bound.evaluate(&request, &constraints).unwrap(),
        Decision::Acquire { .. }
    ));
}

#[test]
fn independent_project_sides_zero_and_geometry_cache_are_respected() {
    let (mut program, mut request, mut constraints) = at_transit();
    let mut other = request.assignment.goals[0].clone();
    other.id = "other-project".into();
    other.meridian_avoidance = Some(MeridianOverride {
        before_ms: Some(0),
        after_ms: Some(600_000),
    });
    request.assignment.goals.push(other);
    let mut binding = program.bindings[0].clone();
    binding.goal_id = "other-project".into();
    program.bindings.push(binding);
    let mut limits = constraints.goals[0].clone();
    limits.goal_id = "other-project".into();
    constraints.goals.push(limits);
    program.assignment = request.assignment.clone();
    let bound = compile(program, &request, constraints.clone()).unwrap();
    assert!(
        matches!(bound.evaluate(&request, &constraints).unwrap(), Decision::Acquire { goal_id, .. } if goal_id == "other-project")
    );
    assert_ne!(bound.windows("goal"), bound.windows("other-project"));
    request.state.now_ms = START + 1_801_000;
    assert!(
        matches!(bound.evaluate(&request, &constraints).unwrap(), Decision::Acquire { goal_id, .. } if goal_id == "goal")
    );
    // Altering an issued project's policy cannot widen the bound program.
    request.assignment.goals[0].meridian_avoidance = Some(MeridianOverride {
        before_ms: Some(0),
        after_ms: None,
    });
    assert!(bound.evaluate(&request, &constraints).is_err());
}

#[test]
fn one_side_inherits_and_exposure_must_finish_before_exclusion() {
    let (mut program, mut request, mut constraints) = at_transit();
    constraints.rig.meridian_exclusion.after_ms = 300_000;
    request.state.meridian_exclusion = constraints.rig.meridian_exclusion;
    request.assignment.goals[0].meridian_avoidance = Some(MeridianOverride {
        before_ms: Some(600_000),
        after_ms: None,
    });
    program.assignment = request.assignment.clone();
    let bound = compile(program, &request, constraints.clone()).unwrap();
    assert!(matches!(
        bound.evaluate(&request, &constraints).unwrap(),
        Decision::Acquire { .. }
    ));
    request.state.now_ms = bound.windows("goal").unwrap()[0].end_ms - 29_999;
    assert!(!matches!(
        bound.evaluate(&request, &constraints).unwrap(),
        Decision::Acquire { .. }
    ));
    request.state.now_ms = START + 2_099_000;
    assert!(!matches!(
        bound.evaluate(&request, &constraints).unwrap(),
        Decision::Acquire { .. }
    ));
    request.state.now_ms = bound.windows("goal").unwrap()[1].start_ms;
    assert!(
        matches!(
            bound.evaluate(&request, &constraints).unwrap(),
            Decision::Acquire { .. }
        ),
        "windows={:?}; decision={:?}",
        bound.windows("goal"),
        bound.evaluate(&request, &constraints)
    );
}

#[test]
fn invalid_project_override_is_rejected_even_without_transits() {
    let (mut program, mut request, _) = at_transit();
    request.assignment.goals[0].meridian_avoidance = Some(MeridianOverride {
        before_ms: Some(21_600_001),
        after_ms: None,
    });
    program.assignment = request.assignment.clone();
    assert!(BoundProgram::new(program, &request.state).is_err());
}

#[test]
fn full_day_assignment_supports_both_transit_search_margins() {
    let (mut program, mut request, mut constraints) = at_transit();
    request.assignment.expires_at_ms = START + 86_400_000;
    request.assignment.goals[0].eligible_windows = vec![interval(START, START + 86_400_000)];
    constraints.rig.orientation.valid_until_ms = START + 108_000_001;
    constraints.rig.meridian_exclusion = MeridianExclusion {
        before_ms: 21_600_000,
        after_ms: 21_600_000,
    };
    request.state.meridian_exclusion = constraints.rig.meridian_exclusion;
    program.assignment = request.assignment.clone();
    let bound = compile(program.clone(), &request, constraints.clone()).unwrap();
    assert!(bound
        .windows("goal")
        .unwrap()
        .iter()
        .all(|window| window.start_ms >= START + 23_400_000));
    constraints.rig.orientation.valid_until_ms = request.assignment.expires_at_ms + 1;
    assert!(
        compile(program, &request, constraints).is_err(),
        "search margins need their own valid astronomy evidence"
    );
}
