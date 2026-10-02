use super::pairing::{client_call, credential, fixture};
use super::*;

#[test]
fn prepared_executor_rejects_unsupported_work_before_admission() {
    let mut p: psf_guard_director_core::program::Program = serde_json::from_str(include_str!(
        "../../../../crates/director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    let supported = crate::server::director::workload::supports_prepared_target;
    assert!(!supported(&p));
    p.configuration.enable_slew_center = false;
    assert!(supported(&p));
    let mut invalid = p.clone();
    invalid.targets.push(p.targets[0].clone());
    assert!(!supported(&invalid));
    assert!(crate::server::director::workload::supports_local_sequence(
        &invalid
    ));
    invalid.targets[1].position_angle_mas = Some(0);
    assert!(!crate::server::director::workload::supports_local_sequence(
        &invalid
    ));
    let mut invalid = p.clone();
    invalid.targets[0].position_angle_mas = Some(0);
    assert!(!supported(&invalid));
    let mut invalid = p.clone();
    invalid.configuration.dither_every = 1;
    assert!(!supported(&invalid));
    let mut invalid = p;
    invalid.recipes[0].dither_override = Some(1);
    assert!(!supported(&invalid));
}

#[tokio::test]
async fn paired_clients_cannot_commission_or_self_authorize_work() {
    let (_dir, _state, app, instance, catalog, rig) = fixture().await;
    let profile = Uuid::new_v4();
    let c = credential(&app, instance, catalog, rig, profile).await;
    let token = c["token"].as_str().unwrap();
    let policy = json!({"coordinator_instance_id":instance,"expected_revision":0,"policy":{
        "rig_id":rig,"catalog_id":catalog,"client_id":c["client_id"],"profile_id":profile,
        "profile_revision":1,"configuration_id":"config-1","project_ids":[Uuid::new_v4()],"enabled":true,"revision":1}});
    assert_eq!(
        client_call(
            &app,
            "PUT",
            &format!("/rigs/{rig}/workload-policy"),
            policy,
            token,
            Some(profile)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let input = json!({"coordinator_instance_id":instance,"catalog_id":catalog,"request_id":Uuid::new_v4(),"configuration_id":"config-1","execution_mode":"prepared_target_v1"});
    assert_eq!(
        client_call(
            &app,
            "POST",
            &format!("/rigs/{rig}/workloads/request"),
            input.clone(),
            token,
            Some(profile)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        client_call(
            &app,
            "POST",
            &format!("/rigs/{rig}/workloads/request"),
            input.clone(),
            token,
            Some(Uuid::new_v4())
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/rigs/{rig}/workloads/request"),
            input,
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let release = json!({"coordinator_instance_id":instance,"catalog_id":catalog,"allocation_id":Uuid::new_v4(),"ledger_id":Uuid::new_v4(),"terminal_sequence":0,"operations_quiescent":false,"parked":true});
    assert_eq!(
        client_call(
            &app,
            "POST",
            &format!("/rigs/{rig}/workloads/release"),
            release,
            token,
            Some(profile)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn policy_requires_interactive_editor_not_a_personal_access_token() {
    let (_dir, state, app, instance, catalog, rig) = fixture().await;
    let mut registry = AuthRegistry::default();
    registry
        .add(
            AuthUserRecord::new("editor", AccessRole::ReadWrite, "test-password-not-real").unwrap(),
            false,
        )
        .unwrap();
    let (pat, record) = AuthTokenRecord::mint("editor", "operator", false, None).unwrap();
    registry.tokens.push(record);
    state.set_server_auth(auth::ServerAuth::from_sources(None, &registry, 3000).unwrap());
    let input = json!({"coordinator_instance_id":instance,"expected_revision":0,"policy":{
        "rig_id":rig,"catalog_id":catalog,"client_id":Uuid::new_v4(),"profile_id":Uuid::new_v4(),
        "profile_revision":1,"configuration_id":"config-1","project_ids":[Uuid::new_v4()],"enabled":true,"revision":1}});
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/rigs/{rig}/workload-policy"),
            input,
            Some(&pat)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}
