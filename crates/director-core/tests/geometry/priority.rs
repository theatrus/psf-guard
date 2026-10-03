use super::*;
use psf_guard_director_core::geometry::PriorityError;
use psf_guard_director_core::priority::{self, Factor, Policy, Preset, Scope, Source};
use std::collections::BTreeMap;

fn policies(program: &Program) -> BTreeMap<String, priority::ResolvedPolicy> {
    program
        .assignment
        .goals
        .iter()
        .map(|g| {
            let mut p = Policy::preset(Preset::Balanced);
            for (f, w) in &mut p.weights {
                *w = if *f == Factor::Importance { 100 } else { 0 };
            }
            p.importance = if g.id == "other" { 0 } else { 100 };
            (
                g.id.clone(),
                priority::resolve(
                    p,
                    Source {
                        scope: Scope::Global,
                        id: "default".into(),
                        revision: 1,
                    },
                    &[],
                )
                .unwrap(),
            )
        })
        .collect()
}

#[test]
fn weighted_preview_cannot_reenable_moon_blocked_goal_or_erase_wait() {
    let (mut program, mut request, mut constraints) = fixture();
    let mut goal = program.assignment.goals[0].clone();
    goal.id = "other".into();
    program.assignment.goals.push(goal);
    let mut recipe = program.recipes[0].clone();
    recipe.id = "other-recipe".into();
    program.recipes.push(recipe);
    program.bindings.push(program::Binding {
        goal_id: "other".into(),
        target_id: "target".into(),
        recipe_id: "other-recipe".into(),
    });
    let mut limit = constraints.goals[0].clone();
    limit.goal_id = "other".into();
    constraints.goals.push(limit);
    program.recipes[0].moon = Some(moon::MoonPolicy {
        enabled: true,
        separation_degrees: 180.0,
        width_days: 14.0,
        ..Default::default()
    });
    request.assignment = program.assignment.clone();
    let policies = policies(&program);
    let geometry = compile(program.clone(), &request, constraints.clone()).unwrap();
    let preview = geometry
        .preview_priority(&request, &constraints, &policies, None)
        .unwrap();
    assert!(matches!(preview.decision, Decision::Acquire { goal_id, .. } if goal_id == "other"));
    assert_eq!(preview.candidates.len(), 1);
    assert_eq!(
        preview.candidates[0].contributions[&Factor::Importance].value,
        0
    );
    assert!(!preview.candidates[0].contributions[&Factor::Altitude].missing);
    program.recipes[1].moon = program.recipes[0].moon.clone();
    let geometry = compile(program, &request, constraints.clone()).unwrap();
    assert!(
        matches!(geometry.preview_priority(&request, &constraints, &policies, None).unwrap().decision,
        Decision::Wait { reason } if reason == "moon_avoidance")
    );
}

#[test]
fn priority_preview_checks_geometry_scope_and_never_changes_existing_evaluation() {
    let (program, request, constraints) = fixture();
    let policies = policies(&program);
    let geometry = compile(program, &request, constraints.clone()).unwrap();
    let before = geometry.evaluate(&request, &constraints).unwrap();
    let preview = geometry
        .preview_priority(&request, &constraints, &policies, None)
        .unwrap();
    assert!(
        matches!(preview.decision, Decision::Acquire { reason, .. } if reason == "observing_preference_score")
    );
    assert_eq!(geometry.evaluate(&request, &constraints).unwrap(), before);
    let mut changed = constraints.clone();
    changed.rig.minimum_altitude_degrees += 1.0;
    assert_eq!(
        geometry.preview_priority(&request, &changed, &policies, None),
        Err(PriorityError::Geometry(Error::ConstraintsChanged))
    );
    assert_eq!(
        geometry.preview_priority(&request, &constraints, &BTreeMap::new(), None),
        Err(PriorityError::Preferences(
            priority::Error::InvalidCandidates
        ))
    );
}
