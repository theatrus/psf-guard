use psf_guard_director_core::{
    geometry::{BoundGeometry, Constraints, GoalLimits, RigConstraints, CONSTRAINTS_VERSION},
    night::{night_preview, planning_orientation, NightRequest},
    program::{BoundProgram, Program, MAS_PER_DEGREE, PROGRAM_VERSION},
    schedule::{simulate, GapReason, Overheads, ScheduleNight, ScheduleRequest},
    visibility::*,
    windows::*,
    *,
};
use serde_json::json;
use std::collections::BTreeMap;

/// 2026-09-25 00:00 UTC, the evening of the 24th in Los Angeles.
const START: u64 = 1_790_294_400_000;

fn site() -> Site {
    Site {
        latitude_degrees: 34.2,
        longitude_degrees: -118.3,
        elevation_meters: 400.0,
    }
}

/// Nights from the shared core's night module: noon to noon, dark below −12°.
fn nights(count: u32) -> Vec<ScheduleNight> {
    night_preview(&NightRequest {
        site: site(),
        horizon: Horizon::FixedMinimum {},
        limits: AltitudeLimits {
            rig_minimum_degrees: 0.0,
            project_minimum_degrees: 0.0,
            horizon_offset_degrees: 0.0,
            rig_maximum_degrees: 90.0,
            project_maximum_degrees: 90.0,
        },
        targets: vec![],
        start_ms: START,
        nights: count,
        step_ms: 300_000,
        dark_below_degrees: -12.0,
        meridian_exclusion: MeridianExclusion {
            before_ms: 0,
            after_ms: 0,
        },
        meridian_window_ms: 0,
        available: None,
    })
    .unwrap()
    .into_iter()
    .map(|night| ScheduleNight {
        span: Interval {
            start_ms: night.noon_ms,
            end_ms: night.noon_ms + 86_400_000,
        },
        dark: night
            .dusk_ms
            .zip(night.dawn_ms)
            .map(|(start_ms, end_ms)| Interval { start_ms, end_ms }),
    })
    .collect()
}

/// Two goals: the Heart in H-alpha, ranked first, and the Soul in OIII.
fn program(requested: [u32; 2]) -> (Program, Constraints) {
    let goal = |id: &str, priority: u32, requested: u32| {
        json!({
            "id": id, "priority": priority, "requested": requested, "accepted": 0, "pending": 0,
            "attempts_remaining": requested * 2, "exposure_ms": 300_000, "overhead_ms": 15_000,
            "eligible_windows": [], "transits": null
        })
    };
    let program: Program = serde_json::from_value(json!({
        "schema_version": PROGRAM_VERSION,
        "assignment": {"id": "assignment", "revision": 1, "rig_id": "rig-1", "configuration_id": "config-1",
            "valid_from_ms": START, "expires_at_ms": START + 1,
            "goals": [goal("heart", 2, requested[0]), goal("soul", 1, requested[1])]},
        "configuration": {
            "id":"config-1", "rig_id":"rig-1", "camera_id":"camera", "filter_wheel_id":"wheel",
            "filters":[{"id":"Ha","position":0},{"id":"OIII","position":1}], "binning_modes":[{"x":1,"y":1}],
            "readout_modes":[0], "gain":{"support":"unsupported"}, "offset":{"support":"unsupported"},
            "exposure_min_ms":1,"exposure_max_ms":3600000,"enable_slew_center":true,"dither_every":3
        },
        "targets":[
            {"id":"heart","name":"IC 1805","icrs_ra_mas":(38.2 * MAS_PER_DEGREE as f64) as u32,
             "icrs_dec_mas":(61.45 * MAS_PER_DEGREE as f64) as i32,"position_angle_mas":null},
            {"id":"soul","name":"IC 1848","icrs_ra_mas":(42.8 * MAS_PER_DEGREE as f64) as u32,
             "icrs_dec_mas":(60.4 * MAS_PER_DEGREE as f64) as i32,"position_angle_mas":null}],
        "recipes":[
            {"id":"ha","exposure_ms":300000,"filter_id":"Ha","binning":{"x":1,"y":1},
             "gain":null,"offset":null,"readout_mode":0,"dither_override":null},
            {"id":"oiii","exposure_ms":300000,"filter_id":"OIII","binning":{"x":1,"y":1},
             "gain":null,"offset":null,"readout_mode":0,"dither_override":null}],
        "bindings":[{"goal_id":"heart","target_id":"heart","recipe_id":"ha"},
                    {"goal_id":"soul","target_id":"soul","recipe_id":"oiii"}]
    }))
    .unwrap();
    let range = nights(3);
    let constraints = Constraints {
        schema_version: CONSTRAINTS_VERSION,
        rig: RigConstraints {
            rig_id: "rig-1".into(),
            configuration_id: "config-1".into(),
            revision: 1,
            site: site(),
            orientation: planning_orientation(range[0].span.start_ms, range[2].span.end_ms),
            horizon: Horizon::FixedMinimum {},
            minimum_altitude_degrees: 30.0,
            maximum_altitude_degrees: 90.0,
            meridian_exclusion: MeridianExclusion {
                before_ms: 0,
                after_ms: 0,
            },
        },
        goals: ["heart", "soul"]
            .map(|id| GoalLimits {
                goal_id: id.into(),
                minimum_altitude_degrees: 0.0,
                maximum_altitude_degrees: 90.0,
                horizon_offset_degrees: 0.0,
            })
            .to_vec(),
    };
    (program, constraints)
}

fn run(
    program: &Program,
    constraints: &Constraints,
    count: u32,
    available: Option<&[Interval]>,
) -> schedule::Schedule {
    let range = nights(count);
    simulate(&ScheduleRequest {
        program,
        constraints,
        nights: &range,
        start_ms: START,
        available,
        overheads: Overheads::default(),
        acceptance: &BTreeMap::new(),
    })
    .unwrap()
}

#[test]
fn every_step_is_the_geometrys_own_decision() {
    let (program, constraints) = program([200, 200]);
    let schedule = run(&program, &constraints, 1, None);
    let night = &schedule.nights[0];
    let dark = nights(1)[0].dark.unwrap();
    assert!(!night.exposures.is_empty());
    // The Heart outranks the Soul, so it takes every exposure it can.
    assert_eq!(night.exposures[0].goal_id, "heart");
    assert!(night.exposures[0].target_change && night.exposures[0].filter_change);
    // Replay: at each exposure's start, a freshly bound geometry with the
    // same progress chooses the same goal.
    let span = nights(1)[0].span;
    let mut bound = program.clone();
    bound.assignment.valid_from_ms = span.start_ms;
    bound.assignment.expires_at_ms = span.end_ms;
    for goal in &mut bound.assignment.goals {
        goal.eligible_windows = vec![span];
    }
    let state = |now_ms: u64| State {
        rig_id: "rig-1".into(),
        configuration_id: "config-1".into(),
        now_ms,
        conditions_valid_until_ms: span.end_ms,
        completion_deadline_ms: Some(dark.end_ms),
        safety: Safety::Safe,
        at_boundary: true,
        operator_stop: false,
        meridian_exclusion: constraints.rig.meridian_exclusion,
    };
    let geometry = BoundGeometry::new(
        BoundProgram::new(bound.clone(), &state(span.start_ms)).unwrap(),
        constraints.clone(),
        &state(span.start_ms),
    )
    .unwrap();
    let mut goals = bound.assignment.goals.clone();
    for exposure in &night.exposures {
        assert!(
            exposure.start_ms >= dark.start_ms && exposure.end_ms <= dark.end_ms + 600_000,
            "{exposure:?}"
        );
        let decision = geometry
            .evaluate(
                &Request {
                    contract_version: CONTRACT_VERSION,
                    assignment: geometry.assignment_with_progress(&goals),
                    state: state(exposure.start_ms),
                },
                &constraints,
            )
            .unwrap();
        assert_eq!(
            decision,
            Decision::Acquire {
                goal_id: exposure.goal_id.clone(),
                reason: exposure.reason.clone()
            }
        );
        let goal = goals.iter_mut().find(|g| g.id == exposure.goal_id).unwrap();
        goal.accepted += 1;
        goal.attempts_remaining -= 1;
    }
    // Dither every third frame on a target, with its time added.
    assert!(
        night.exposures.iter().take(4).filter(|e| e.dither).count() == 1,
        "{:?}",
        night
            .exposures
            .iter()
            .take(4)
            .map(|e| e.dither)
            .collect::<Vec<_>>()
    );
    assert_eq!(night.gaps.first().unwrap().reason, GapReason::Twilight);
    assert!(night.gaps.iter().all(|gap| gap.end_ms > gap.start_ms));
}

#[test]
fn a_goal_that_completes_hands_the_night_to_the_next_and_carries_over() {
    let (program, constraints) = program([20, 400]);
    let schedule = run(&program, &constraints, 2, None);
    let first = &schedule.nights[0];
    let heart = first
        .exposures
        .iter()
        .filter(|e| e.goal_id == "heart")
        .count();
    assert_eq!(heart, 20);
    let switch = first
        .exposures
        .iter()
        .position(|e| e.goal_id == "soul")
        .unwrap();
    assert!(first.exposures[switch].target_change);
    assert!(first.exposures[..switch]
        .iter()
        .all(|e| e.goal_id == "heart"));
    let heart = &schedule.progress[0];
    assert_eq!(
        (heart.planned.clone(), heart.completes_on),
        (vec![20, 0], Some(0))
    );
    let soul = &schedule.progress[1];
    assert!(soul.planned[1] > 0 && soul.completes_on.is_none());
    // Done goals stay done: the second night is the Soul's alone.
    assert!(schedule.nights[1]
        .exposures
        .iter()
        .all(|e| e.goal_id == "soul"));
    let (small, constraints) = self::program([3, 3]);
    let done = run(&small, &constraints, 1, None);
    assert!(done.nights[0]
        .gaps
        .iter()
        .any(|g| g.reason == GapReason::Complete));
}

#[test]
fn work_stays_inside_the_available_windows() {
    let (program, constraints) = program([400, 400]);
    let dark = nights(1)[0].dark.unwrap();
    let window = [Interval {
        start_ms: dark.start_ms + 3_600_000,
        end_ms: dark.start_ms + 3 * 3_600_000,
    }];
    let schedule = run(&program, &constraints, 1, Some(&window));
    let night = &schedule.nights[0];
    assert!(!night.exposures.is_empty());
    assert!(
        night
            .exposures
            .iter()
            .all(|e| e.start_ms >= window[0].start_ms && e.end_ms <= window[0].end_ms),
        "{:?}",
        night.exposures.last()
    );
    assert!(night
        .gaps
        .iter()
        .any(|g| g.reason == GapReason::Unavailable));
}
