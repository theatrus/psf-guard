use super::*;
use psf_guard_director_core::priority::{
    self, Factor, Policy, Preset, ProgramPreferences, Scope, Source,
};

fn preferred_fixture() -> Fixture {
    let mut f = Fixture::new();
    f.constraints.rig.horizon = Horizon::FixedMinimum {};
    let mut goal = f.program.assignment.goals[0].clone();
    goal.id = "preferred".into();
    goal.priority = 0;
    f.program.assignment.goals.push(goal);
    let mut binding = f.program.bindings[0].clone();
    binding.goal_id = "preferred".into();
    f.program.bindings.push(binding);
    let mut limit = f.constraints.goals[0].clone();
    limit.goal_id = "preferred".into();
    f.constraints.goals.push(limit);
    let policies = [("short-ha", 10), ("preferred", 90)]
        .map(|(id, importance)| {
            let mut p = Policy::preset(Preset::Balanced);
            for (factor, weight) in &mut p.weights {
                *weight = if *factor == Factor::Importance {
                    100
                } else {
                    0
                };
            }
            p.importance = importance;
            (
                id.into(),
                priority::resolve(
                    p,
                    Source {
                        scope: Scope::Global,
                        id: "global".into(),
                        revision: 1,
                    },
                    &[],
                )
                .unwrap(),
            )
        })
        .into();
    f.program.observing_preferences = Some(ProgramPreferences {
        schema_version: 1,
        policies,
        bindings: [
            ("short-ha".into(), "short-ha".into()),
            ("preferred".into(), "preferred".into()),
        ]
        .into(),
    });
    f
}

#[test]
fn weighted_selection_executes_and_reopens_without_losing_selection() {
    let f = preferred_fixture();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("ledger.sqlite");
    let mut ledger = f.open(&path);
    assert!(
        matches!(ledger.evaluate_geometry(f.state.clone(), &f.constraints).unwrap(), Decision::Acquire {goal_id, ..} if goal_id == "preferred")
    );
    assert!(ledger
        .begin_geometry_preparation(
            "wrong",
            "short-ha",
            f.local(),
            Estimates::default(),
            f.state.clone(),
            &f.constraints
        )
        .is_err());
    ledger
        .begin_geometry_preparation(
            "prep",
            "preferred",
            f.local(),
            Estimates::default(),
            f.state.clone(),
            &f.constraints,
        )
        .unwrap();
    drop(ledger);
    let db = Connection::open(&path).unwrap();
    let selected: String = db
        .query_row("SELECT payload FROM observing_selection", [], |r| r.get(0))
        .unwrap();
    assert!(selected.contains("preferred"));
    let mut ledger = f.open(&path);
    f.ready(&mut ledger);
    assert!(f.reserve(&mut ledger).is_ok());
}

#[test]
fn selection_corruption_or_deletion_cannot_reset_dwell() {
    for sql in [
        "DELETE FROM observing_selection",
        "UPDATE observing_selection SET payload='null'",
    ] {
        let f = preferred_fixture();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ledger.sqlite");
        let mut ledger = f.open(&path);
        ledger
            .begin_geometry_preparation(
                "prep",
                "preferred",
                f.local(),
                Estimates::default(),
                f.state.clone(),
                &f.constraints,
            )
            .unwrap();
        Connection::open(&path).unwrap().execute_batch(sql).unwrap();
        assert!(ledger
            .evaluate_geometry(f.state.clone(), &f.constraints)
            .is_err());
    }
}

#[test]
fn restart_retains_dwell_when_a_more_important_target_becomes_available() {
    let mut f = preferred_fixture();
    f.program.assignment.goals[0].eligible_windows[0].start_ms = START + 1000;
    let preferences = f.program.observing_preferences.as_mut().unwrap();
    let mut policy = preferences.policies["short-ha"].policy().clone();
    policy.importance = 100;
    preferences.policies.insert(
        "short-ha".into(),
        priority::resolve(
            policy,
            Source {
                scope: Scope::Global,
                id: "global".into(),
                revision: 1,
            },
            &[],
        )
        .unwrap(),
    );
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("ledger.sqlite");
    let mut ledger = f.open(&path);
    ledger
        .begin_geometry_preparation(
            "prep",
            "preferred",
            f.local(),
            Estimates::default(),
            f.state.clone(),
            &f.constraints,
        )
        .unwrap();
    ledger.close_preparation("prep").unwrap();
    drop(ledger);
    let mut ledger = f.open(&path);
    let mut later = f.state.clone();
    later.now_ms += 2000;
    assert!(
        matches!(ledger.evaluate_geometry(later.clone(), &f.constraints).unwrap(), Decision::Acquire { goal_id, .. } if goal_id == "preferred")
    );
    later.safety = Safety::Unsafe;
    assert!(matches!(
        ledger.evaluate_geometry(later, &f.constraints).unwrap(),
        Decision::Stop { .. }
    ));
}
