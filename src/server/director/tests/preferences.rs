use super::pairing::{client_call, credential, fixture};
use super::*;

#[tokio::test]
async fn preferences_are_operator_owned_revisioned_and_scoped() {
    let (_dir, _state, app, instance, catalog, rig) = fixture().await;
    let (_, defaults) = call(&app, "GET", "/preferences", Value::Null, None).await;
    assert_eq!(defaults["data"]["global_id"], instance.to_string());
    let path = format!("/preferences/rig/{rig}");
    let (_, value) = call(&app, "GET", &path, Value::Null, None).await;
    let mut settings = value["data"].clone();
    settings["enabled"] = true.into();
    settings["overrides"]["importance"] = 0.into();
    let profile = Uuid::new_v4();
    let c = credential(&app, instance, catalog, rig, profile).await;
    assert_eq!(
        client_call(
            &app,
            "PUT",
            &path,
            settings.clone(),
            c["token"].as_str().unwrap(),
            Some(profile)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, saved) = call(&app, "PUT", &path, settings.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["data"]["revision"], 1);
    assert_eq!(
        call(&app, "PUT", &path, settings, None).await.0,
        StatusCode::CONFLICT
    );
    let (_, effective) = call(
        &app,
        "GET",
        &format!("/rigs/{rig}/preferences"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(effective["data"]["enabled"], true);
    assert_eq!(effective["data"]["resolved"]["policy"]["importance"], 0);
    assert_eq!(
        effective["data"]["resolved"]["provenance"]["importance"]["scope"],
        "rig"
    );
}

#[tokio::test]
async fn global_project_order_accepts_a_full_list_and_rejects_stale_replacements() {
    let (_dir, state, app, instance, _catalog, rig) = fixture().await;
    let order: Vec<Uuid> = {
        let mut store = state.director.as_ref().unwrap().writer.lock().unwrap();
        // Every Target Scheduler project is a plan, so an order ranks as
        // many as the plan list shows.
        (0..psf_guard_director_meta::MAX_PLANS)
            .map(|i| {
                store
                    .create_project(Uuid::new_v4(), &format!("Project {i}"))
                    .unwrap()
                    .id
            })
            .collect()
    };
    let path = format!("/preferences/global/{instance}");
    let (_, value) = call(&app, "GET", &path, Value::Null, None).await;
    let mut settings = value["data"].clone();
    settings["project_order"] = json!(order);
    assert!(serde_json::to_vec(&settings).unwrap().len() > 16384);
    let (status, saved) = call(&app, "PUT", &path, settings.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        call(&app, "PUT", &path, settings, None).await.0,
        StatusCode::CONFLICT
    );
    let (_, effective) = call(
        &app,
        "GET",
        &format!("/rigs/{rig}/preferences"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(effective["data"]["project_order"], json!(order));
    assert_eq!(effective["data"]["order_source"]["scope"], "global");
}

#[test]
fn ranked_projects_dominate_objective_scores_but_never_eligibility_or_safety() {
    use psf_guard_director_core::{
        evaluate, program::Program, Decision, Request, Safety, State, CONTRACT_VERSION,
    };
    use std::collections::BTreeMap;
    let program: Program = serde_json::from_str(include_str!(
        "../../../../crates/director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut assignment = program.assignment;
    let template = assignment.goals[0].clone();
    assignment.goals = [0, 1, u32::MAX]
        .into_iter()
        .enumerate()
        .map(|(i, priority)| psf_guard_director_core::Goal {
            id: format!("goal-{i}"),
            priority,
            ..template.clone()
        })
        .collect();
    let bindings = BTreeMap::from([
        ("goal-0".into(), first),
        ("goal-1".into(), first),
        ("goal-2".into(), second),
    ]);
    super::super::program::apply_project_order(&mut assignment.goals, &bindings, &[first, second])
        .unwrap();
    assert!(assignment.goals[0].priority > assignment.goals[2].priority);
    assert!(assignment.goals[1].priority > assignment.goals[0].priority);
    let state = State {
        rig_id: assignment.rig_id.clone(),
        configuration_id: assignment.configuration_id.clone(),
        now_ms: 1000,
        conditions_valid_until_ms: 100000,
        completion_deadline_ms: None,
        safety: Safety::Safe,
        at_boundary: true,
        operator_stop: false,
        meridian_exclusion: psf_guard_director_core::windows::MeridianExclusion {
            before_ms: 0,
            after_ms: 0,
        },
    };
    let mut request = Request {
        contract_version: CONTRACT_VERSION,
        assignment,
        state,
    };
    let picked = |request: &Request| match evaluate(request).unwrap() {
        Decision::Acquire { goal_id, .. } => goal_id,
        other => panic!("{other:?}"),
    };
    assert_eq!(picked(&request), "goal-1");
    request.assignment.goals[1].accepted = 1;
    assert_eq!(picked(&request), "goal-0");
    request.assignment.goals[0].eligible_windows.clear();
    assert_eq!(picked(&request), "goal-2");
    request.state.safety = Safety::Unsafe;
    assert!(matches!(evaluate(&request).unwrap(), Decision::Stop { .. }));
    assert!(super::super::program::apply_project_order(
        &mut request.assignment.goals,
        &bindings,
        &[first]
    )
    .is_err());
}
