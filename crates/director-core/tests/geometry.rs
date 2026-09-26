use psf_guard_director_core::{
    geometry::{
        BoundGeometry, Constraints, Error, GoalLimits, RigConstraints, CONSTRAINTS_VERSION,
    },
    program::{BoundProgram, Program, MAS_PER_DEGREE, PROGRAM_VERSION},
    visibility::*,
    windows::*,
    *,
};
use serde_json::json;

const START: u64 = 1_790_409_600_000;

fn interval(start: u64, end: u64) -> Interval {
    Interval {
        start_ms: start,
        end_ms: end,
    }
}

fn fixture() -> (Program, Request, Constraints) {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/decisions.json")).unwrap();
    let mut request: Request = serde_json::from_value(fixture["base"].clone()).unwrap();
    request.assignment.goals.truncate(1);
    request.assignment.valid_from_ms = START;
    request.assignment.expires_at_ms = START + 60_000;
    let goal = &mut request.assignment.goals[0];
    goal.id = "goal".into();
    goal.exposure_ms = 20_000;
    goal.overhead_ms = 10_000;
    goal.eligible_windows = vec![interval(START, START + 60_000)];
    request.state.now_ms = START;
    request.state.conditions_valid_until_ms = START + 60_001;
    let program = serde_json::from_value(json!({
        "schema_version": PROGRAM_VERSION, "assignment": request.assignment,
        "configuration": {
            "id":"config-1", "rig_id":"rig-1", "camera_id":"camera", "filter_wheel_id":null,
            "filters":[{"id":"L","position":null}], "binning_modes":[{"x":1,"y":1}],
            "readout_modes":[0], "gain":{"support":"unsupported"}, "offset":{"support":"unsupported"},
            "exposure_min_ms":1,"exposure_max_ms":100000,"enable_slew_center":true,"dither_every":0
        },
        "targets":[{"id":"target","name":"M42","icrs_ra_mas":83 * MAS_PER_DEGREE,
            "icrs_dec_mas":-5 * MAS_PER_DEGREE as i32,"position_angle_mas":null}],
        "recipes":[{"id":"recipe","exposure_ms":20000,"filter_id":"L","binning":{"x":1,"y":1},
            "gain":null,"offset":null,"readout_mode":0,"dither_override":null}],
        "bindings":[{"goal_id":"goal","target_id":"target","recipe_id":"recipe"}]
    })).unwrap();
    let constraints = Constraints {
        schema_version: CONSTRAINTS_VERSION,
        rig: RigConstraints {
            rig_id: "rig-1".into(),
            configuration_id: "config-1".into(),
            revision: 1,
            site: Site {
                latitude_degrees: 35.0,
                longitude_degrees: -120.0,
                elevation_meters: 1000.0,
            },
            orientation: EarthOrientation {
                ut1_minus_utc_seconds: 0.0,
                polar_motion_x_radians: 0.0,
                polar_motion_y_radians: 0.0,
                valid_from_ms: START - 60_000,
                valid_until_ms: START + 120_001,
            },
            horizon: Horizon::FixedMinimum {},
            minimum_altitude_degrees: -89.0,
            maximum_altitude_degrees: 89.0,
            meridian_exclusion: MeridianExclusion {
                before_ms: 0,
                after_ms: 0,
            },
        },
        goals: vec![GoalLimits {
            goal_id: "goal".into(),
            minimum_altitude_degrees: -89.0,
            maximum_altitude_degrees: 89.0,
            horizon_offset_degrees: 0.0,
        }],
    };
    (program, request, constraints)
}

fn compile(
    program: Program,
    request: &Request,
    constraints: Constraints,
) -> Result<BoundGeometry, Error> {
    BoundGeometry::new(
        BoundProgram::new(program, &request.state).unwrap(),
        constraints,
        &request.state,
    )
}

fn spike(constraints: &mut Constraints) {
    let az = observe(
        IcrsPosition {
            ra_degrees: 83.0,
            dec_degrees: -5.0,
        },
        constraints.rig.site,
        constraints.rig.orientation,
        START + 21_777,
    )
    .unwrap()
    .azimuth_degrees;
    constraints.rig.horizon = Horizon::Custom {
        points: [
            (0.0, -89.0),
            (az.next_down(), -89.0),
            (az, 89.0),
            (az.next_up(), -89.0),
            (360.0, -89.0),
        ]
        .into_iter()
        .map(|(azimuth_degrees, altitude_degrees)| HorizonPoint {
            azimuth_degrees,
            altitude_degrees,
        })
        .collect(),
    };
}

#[test]
fn real_horizon_geometry_drives_selection_and_slow_work_cannot_bridge_a_gap() {
    let (program, mut request, mut constraints) = fixture();
    spike(&mut constraints);
    let bound = compile(program, &request, constraints.clone()).unwrap();
    assert!(matches!(evaluate(&request), Ok(Decision::Acquire { .. })));
    assert!(matches!(
        bound.evaluate(&request, &constraints),
        Ok(Decision::Wait { .. })
    ));
    let windows = bound.windows("goal").unwrap();
    assert_eq!(windows.len(), 2);
    request.state.now_ms = windows[1].start_ms;
    assert!(matches!(
        bound.evaluate(&request, &constraints),
        Ok(Decision::Acquire { .. })
    ));
    request.state.now_ms = windows[1].end_ms - 29_999;
    assert!(matches!(
        bound.evaluate(&request, &constraints),
        Ok(Decision::CheckIn { .. })
    ));
    assert!(bound.windows("unknown").is_none());
}

#[test]
fn computed_meridian_constraint_cannot_be_relaxed_by_empty_claimed_transits() {
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
        before_ms: 1000,
        after_ms: 2000,
    };
    request.state.meridian_exclusion = constraints.rig.meridian_exclusion;
    request.assignment.goals[0].transits = Some(TransitCoverage {
        searched: interval(START - 2000, START + 61_000),
        transits_ms: vec![],
    });
    program.assignment = request.assignment.clone();
    let bound = compile(program, &request, constraints.clone()).unwrap();
    assert!(matches!(evaluate(&request), Ok(Decision::Acquire { .. })));
    assert!(matches!(
        bound.evaluate(&request, &constraints),
        Ok(Decision::CheckIn { .. })
    ));
    assert_eq!(bound.windows("goal").unwrap().len(), 2);
    assert!(bound
        .windows("goal")
        .unwrap()
        .iter()
        .all(|w| w.end_ms <= transit - 1000 || w.start_ms >= transit + 2000));
    request.state.meridian_exclusion = MeridianExclusion {
        before_ms: 0,
        after_ms: 0,
    };
    assert_eq!(
        bound.evaluate(&request, &constraints),
        Err(Error::ConstraintsChanged)
    );
}

#[test]
fn same_ids_never_hide_changed_constraint_content() {
    let (program, request, constraints) = fixture();
    let bound = compile(program, &request, constraints.clone()).unwrap();
    for fault in 0..17 {
        let mut current = constraints.clone();
        match fault {
            0 => current.rig.site.latitude_degrees += 0.001,
            1 => current.rig.site.longitude_degrees += 0.001,
            2 => current.rig.site.elevation_meters += 1.0,
            3 => current.rig.orientation.ut1_minus_utc_seconds += 0.1,
            4 => current.rig.orientation.polar_motion_x_radians += 0.000001,
            5 => current.rig.orientation.valid_until_ms -= 1,
            6 => spike(&mut current),
            7 => current.rig.minimum_altitude_degrees += 1.0,
            8 => current.rig.maximum_altitude_degrees -= 1.0,
            9 => current.rig.meridian_exclusion.before_ms = 1,
            10 => current.goals[0].minimum_altitude_degrees += 1.0,
            11 => current.goals[0].maximum_altitude_degrees -= 1.0,
            12 => current.goals[0].horizon_offset_degrees += 1.0,
            13 => current.goals[0].goal_id = "other".into(),
            14 => current.rig.revision += 1,
            15 => current.rig.configuration_id = "other".into(),
            _ => current.schema_version += 1,
        }
        assert_eq!(
            bound.evaluate(&request, &current),
            Err(Error::ConstraintsChanged),
            "fault {fault}"
        );
    }
}

#[test]
fn stale_geometry_does_not_block_safety_stop_or_interrupt_native_operation() {
    let (program, mut request, constraints) = fixture();
    let bound = compile(program, &request, constraints.clone()).unwrap();
    let mut changed = constraints.clone();
    changed.rig.revision += 1;
    request.state.at_boundary = false;
    assert!(matches!(
        bound.evaluate(&request, &changed),
        Ok(Decision::Continue { .. })
    ));
    request.state.safety = Safety::Unsafe;
    assert!(matches!(
        bound.evaluate(&request, &changed),
        Ok(Decision::Stop { .. })
    ));
    request.state.safety = Safety::Safe;
    request.state.operator_stop = true;
    assert!(matches!(
        bound.evaluate(&request, &changed),
        Ok(Decision::Stop { .. })
    ));
    request.state.operator_stop = false;
    request.state.at_boundary = true;
    assert_eq!(
        bound.evaluate(&request, &changed),
        Err(Error::ConstraintsChanged)
    );
}

#[test]
fn full_goal_coverage_and_correct_rig_are_required_before_geometry_work() {
    let (program, request, constraints) = fixture();
    for fault in 0..7 {
        let mut invalid = constraints.clone();
        match fault {
            0 => invalid.goals.clear(),
            1 => invalid.goals.push(invalid.goals[0].clone()),
            2 => invalid.goals[0].goal_id = "other".into(),
            3 => invalid.schema_version += 1,
            4 => invalid.rig.revision = 0,
            5 => invalid.rig.rig_id = "other".into(),
            _ => invalid.rig.configuration_id = "other".into(),
        }
        assert!(matches!(
            compile(program.clone(), &request, invalid),
            Err(Error::InvalidConstraints | Error::WrongScope)
        ));
    }
}

#[test]
fn invalid_or_stale_geometry_cannot_compile_even_when_allocation_is_empty() {
    let (mut program, mut request, constraints) = fixture();
    request.assignment.goals[0].eligible_windows.clear();
    program.assignment = request.assignment.clone();
    for fault in 0..4 {
        let mut invalid = constraints.clone();
        match fault {
            0 => invalid.rig.orientation.valid_until_ms = START + 60_000,
            1 => invalid.rig.horizon = Horizon::Custom { points: vec![] },
            2 => invalid.rig.site.latitude_degrees = f64::NAN,
            _ => invalid.goals[0].horizon_offset_degrees = -1.0,
        }
        assert!(matches!(
            compile(program.clone(), &request, invalid),
            Err(Error::Geometry(_))
        ));
    }
}

#[test]
fn geometry_never_expands_intent_or_allows_project_preferences_to_relax_rig_limits() {
    let (mut program, mut request, mut constraints) = fixture();
    request.assignment.goals[0].eligible_windows = vec![
        interval(START + 1, START + 10_000),
        interval(START + 20_000, START + 50_000),
    ];
    program.assignment = request.assignment.clone();
    let bound = compile(program.clone(), &request, constraints.clone()).unwrap();
    assert_eq!(
        bound.windows("goal").unwrap(),
        request.assignment.goals[0].eligible_windows
    );
    constraints.rig.minimum_altitude_degrees = 80.0;
    let blocked = compile(program, &request, constraints.clone()).unwrap();
    assert!(blocked.windows("goal").unwrap().is_empty());
    assert!(matches!(
        blocked.evaluate(&request, &constraints),
        Ok(Decision::CheckIn { .. })
    ));
}

#[test]
fn progress_projects_but_changes_to_source_intent_are_refused() {
    let (program, request, constraints) = fixture();
    let bound = compile(program, &request, constraints.clone()).unwrap();
    for fault in 0..6 {
        let mut changed = request.clone();
        match fault {
            0 => changed.assignment.goals[0].eligible_windows[0].end_ms += 1,
            1 => changed.assignment.goals[0].exposure_ms += 1,
            2 => changed.assignment.goals[0].priority += 1,
            3 => changed.assignment.revision += 1,
            4 => changed.assignment.goals[0].attempts_remaining += 1,
            _ => changed.assignment.goals[0].id = "other".into(),
        }
        assert!(matches!(
            bound.evaluate(&changed, &constraints),
            Err(Error::Program(_))
        ));
    }
    let mut progress = request;
    progress.assignment.goals[0].accepted += 1;
    progress.assignment.goals[0].pending += 1;
    progress.assignment.goals[0].attempts_remaining -= 1;
    assert!(matches!(
        bound.evaluate(&progress, &constraints),
        Ok(Decision::Acquire { .. })
    ));
    progress.assignment.goals[0].pending = progress.assignment.goals[0].requested;
    assert!(matches!(
        bound.evaluate(&progress, &constraints),
        Ok(Decision::Wait { reason }) if reason == "pending_assessment"
    ));
}

#[test]
fn shared_target_geometry_keeps_goal_limits_and_recipe_identity_separate() {
    let (mut program, mut request, mut constraints) = fixture();
    let mut second = request.assignment.goals[0].clone();
    second.id = "second".into();
    second.priority -= 1;
    request.assignment.goals.push(second);
    program.assignment = request.assignment.clone();
    let mut binding = program.bindings[0].clone();
    binding.goal_id = "second".into();
    program.bindings.push(binding);
    let mut preferences = constraints.goals[0].clone();
    preferences.goal_id = "second".into();
    constraints.goals.push(preferences);
    let both = compile(program.clone(), &request, constraints.clone()).unwrap();
    assert_eq!(both.windows("goal"), both.windows("second"));
    let mut duplicate = constraints.clone();
    duplicate.goals[1] = duplicate.goals[0].clone();
    assert!(matches!(
        compile(program.clone(), &request, duplicate),
        Err(Error::InvalidConstraints)
    ));
    constraints.goals[0].minimum_altitude_degrees = 80.0;
    let different = compile(program, &request, constraints.clone()).unwrap();
    assert!(different.windows("goal").unwrap().is_empty());
    assert!(!different.windows("second").unwrap().is_empty());
    constraints.goals.reverse();
    assert!(
        matches!(different.evaluate(&request, &constraints), Ok(Decision::Acquire { goal_id, .. }) if goal_id == "second")
    );
}

#[test]
fn constraint_json_is_explicit_and_rejects_unknown_fields() {
    let (_, _, constraints) = fixture();
    let value = serde_json::to_value(&constraints).unwrap();
    assert_eq!(
        serde_json::from_value::<Constraints>(value.clone()).unwrap(),
        constraints
    );
    for path in ["schema_version", "rig", "goals"] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(path);
        assert!(serde_json::from_value::<Constraints>(missing).is_err());
    }
    let mut extra = value;
    extra["rig"]["ignore_horizon"] = json!(true);
    assert!(serde_json::from_value::<Constraints>(extra).is_err());
}
