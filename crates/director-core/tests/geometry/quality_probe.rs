use super::*;
use psf_guard_director_core::recovery as r;

fn recovering() -> r::Snapshot {
    let id = r::Identity {
        rig_id: "rig-1".into(),
        configuration_id: "config-1".into(),
        night_id: "night".into(),
        starts_at_ms: START,
        ends_at_ms: START + 60_000,
    };
    let policy = r::Policy {
        revision: 1,
        quality_mode: r::QualityMode::Pause,
        bad_samples: 1,
        good_probes: 2,
        cooldown_ms: 10,
        maximum_hold_ms: 50_000,
        maximum_probes: 3,
        operation_timeout_ms: 40_000,
        evidence_max_age_ms: 1000,
        latest_resume_ms: START + 59_000,
        maximum_consecutive_failures: 2,
        maximum_total_failures: 3,
        park_on_stop: true,
        weather: None,
    };
    let safe = r::Conditions {
        safety: Safety::Safe,
        motion: r::Motion::Permitted,
    };
    r::Snapshot::new(id, policy, START)
        .unwrap()
        .apply(
            START + 1,
            safe,
            &r::Event::Quality {
                sample: r::QualitySample {
                    rig_id: "rig-1".into(),
                    configuration_id: "config-1".into(),
                    capture_id: "saved".into(),
                    observed_at_ms: START + 1,
                    context: r::QualityContext {
                        target_id: "target".into(),
                        filter_id: "L".into(),
                        exposure_ms: 20_000,
                        bin_x: 1,
                        bin_y: 1,
                        reference_id: "approved".into(),
                        source: "local".into(),
                        algorithm_revision: "1".into(),
                    },
                    verdict: r::Verdict::CorroboratedPoor,
                },
            },
        )
        .unwrap()
        .apply(
            START + 11,
            safe,
            &r::Event::BeginRecovery {
                attempt_id: "probe".into(),
            },
        )
        .unwrap()
}

#[test]
fn probe_feasibility_does_not_borrow_science_budget_or_change_source() {
    let (mut program, mut request, constraints) = fixture();
    program.assignment.goals[0].attempts_remaining = 0;
    program.assignment.goals[0].accepted = program.assignment.goals[0].requested;
    request.assignment = program.assignment.clone();
    request.state.now_ms = START + 11;
    let original = program.clone();
    let bound = compile(program.clone(), &request, constraints.clone()).unwrap();
    assert!(matches!(
        bound.evaluate(&request, &constraints).unwrap(),
        Decision::Complete { .. }
    ));
    let result = bound
        .check_quality_probe(
            "goal",
            &constraints,
            &request.state,
            &recovering(),
            "probe",
            &program.recipes[0],
        )
        .unwrap();
    assert!(matches!(result.decision, Decision::Acquire { .. }));
    assert!(result.latest_start_ms.unwrap() <= START + 10_011);
    assert_eq!(program, original);
    assert!(matches!(
        bound.evaluate(&request, &constraints).unwrap(),
        Decision::Complete { .. }
    ));
}

#[test]
fn probe_retains_safety_geometry_and_full_exposure_deadline() {
    let (program, mut request, constraints) = fixture();
    request.state.now_ms = START + 11;
    let recipe = program.recipes[0].clone();
    let bound = compile(program, &request, constraints.clone()).unwrap();
    let snapshot = recovering();
    request.state.safety = Safety::Unsafe;
    assert!(matches!(
        bound
            .check_quality_probe(
                "goal",
                &constraints,
                &request.state,
                &snapshot,
                "probe",
                &recipe
            )
            .unwrap()
            .decision,
        Decision::Stop { .. }
    ));
    request.state.safety = Safety::Safe;
    request.state.completion_deadline_ms = Some(START + 10_000);
    assert!(bound
        .check_quality_probe(
            "goal",
            &constraints,
            &request.state,
            &snapshot,
            "probe",
            &recipe
        )
        .unwrap()
        .latest_start_ms
        .is_none());
    let mut changed = constraints.clone();
    changed.rig.site.longitude_degrees += 1.0;
    assert_eq!(
        bound.check_quality_probe(
            "goal",
            &changed,
            &request.state,
            &snapshot,
            "probe",
            &recipe
        ),
        Err(Error::ConstraintsChanged)
    );
    assert_eq!(
        bound.check_quality_probe(
            "goal",
            &constraints,
            &request.state,
            &snapshot,
            "wrong",
            &recipe
        ),
        Err(Error::InvalidCheckpoint)
    );
}

#[test]
fn probe_cannot_use_a_different_recipe_context_or_stale_recovery() {
    let (mut program, mut request, constraints) = fixture();
    request.state.now_ms = START + 11;
    program.recipes[0].filter_id = "Ha".into();
    program.configuration.filters[0].id = "Ha".into();
    let recipe = program.recipes[0].clone();
    let bound = compile(program, &request, constraints.clone()).unwrap();
    assert_eq!(
        bound.check_quality_probe(
            "goal",
            &constraints,
            &request.state,
            &recovering(),
            "probe",
            &recipe
        ),
        Err(Error::InvalidCheckpoint)
    );
    let (program, mut request, constraints) = fixture();
    request.state.now_ms = START + 1012;
    let recipe = program.recipes[0].clone();
    let bound = compile(program, &request, constraints.clone()).unwrap();
    assert_eq!(
        bound.check_quality_probe(
            "goal",
            &constraints,
            &request.state,
            &recovering(),
            "probe",
            &recipe
        ),
        Err(Error::InvalidCheckpoint)
    );
}

#[test]
fn probe_refuses_changed_gain_even_with_same_filter_exposure_and_recipe_id() {
    let (program, mut request, constraints) = fixture();
    let mut reference_recipe = program.recipes[0].clone();
    reference_recipe.gain = Some(100);
    request.state.now_ms = START + 11;
    let bound = compile(program, &request, constraints.clone()).unwrap();
    assert_eq!(
        bound.check_quality_probe(
            "goal",
            &constraints,
            &request.state,
            &recovering(),
            "probe",
            &reference_recipe
        ),
        Err(Error::InvalidCheckpoint)
    );
}
