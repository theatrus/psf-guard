use super::pairing::{client_call, credential, fixture};
use super::*;

#[tokio::test]
async fn paired_recovery_confirms_absence_and_retires_only_its_own_quiescent_work() {
    let (_dir, state, app, instance, catalog, rig) = fixture().await;
    let profile = Uuid::new_v4();
    let paired = credential(&app, instance, catalog, rig, profile).await;
    let token = paired["token"].as_str().unwrap();
    let client = Uuid::parse_str(paired["client_id"].as_str().unwrap()).unwrap();
    state.set_anonymous_access_trusted(false);
    let path =
        format!("/rigs/{rig}/allocation?coordinator_instance_id={instance}&catalog_id={catalog}");
    let (code, absent) = client_call(&app, "GET", &path, Value::Null, token, Some(profile)).await;
    assert_eq!(code, StatusCode::NOT_FOUND, "{absent}");
    assert_eq!(absent["error"], "Director record not found");
    let id = Uuid::new_v4();
    let mut program: Value = serde_json::from_str(include_str!(
        "../../../../crates/director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    program["assignment"]["id"] = json!(format!("allocation-{id}"));
    program["assignment"]["rig_id"] = json!(rig);
    program["configuration"]["rig_id"] = json!(rig);
    let allocation = psf_guard_director_meta::allocation::Allocation {
        schema_version: 1,
        allocation_id: id,
        coordinator_instance_id: instance,
        catalog_id: catalog,
        rig_id: rig,
        client_id: client,
        profile_id: profile,
        preview_revision: "a".repeat(64),
        admitted_at_ms: 1001,
        snapshot: json!({"coordinator_instance_id":instance,"catalog_id":catalog,"rig_id":rig,"program":program}),
    };
    let service = state.director.clone().unwrap();
    service
        .clone()
        .with_writer(move |store| Ok::<_, Error>(store.admit_allocation(&allocation)?))
        .await
        .unwrap();
    let path = format!("/rigs/{rig}/workloads/{id}/reconcile");
    let input = json!({"coordinator_instance_id":instance,"catalog_id":catalog,"camera_idle":true,"mount_stopped":true,"guider_stopped":true});
    for key in ["camera_idle", "mount_stopped", "guider_stopped"] {
        let mut unsafe_input = input.clone();
        unsafe_input[key] = json!(false);
        assert_eq!(
            client_call(&app, "POST", &path, unsafe_input, token, Some(profile))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        client_call(
            &app,
            "POST",
            &path,
            input.clone(),
            token,
            Some(Uuid::new_v4())
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    for _ in 0..2 {
        let (code, ack) =
            client_call(&app, "POST", &path, input.clone(), token, Some(profile)).await;
        assert_eq!(code, StatusCode::OK, "{ack}");
        assert_eq!(ack["data"], json!({"allocation_id":id,"retired":true}));
    }
    let history = service
        .clone()
        .query(move |store| store.workload(rig, client, id))
        .await
        .unwrap()
        .unwrap();
    assert!(history.released);
    assert!(history.terminal_sequence.is_none());
}

#[test]
fn native_imaging_requires_explicit_capability_and_centering_for_rotation() {
    use crate::server::director::workload::ExecutionMode;
    let mut p: psf_guard_director_core::program::Program = serde_json::from_str(include_str!(
        "../../../../crates/director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    p.configuration.enable_slew_center = true;
    p.configuration.dither_every = 3;
    p.targets[0].position_angle_mas = Some(90000000);
    assert!(ExecutionMode::LocalSequenceV3.validate(&p).is_err());
    assert!(ExecutionMode::NativeImagingV1.validate(&p).is_ok());
    assert!(ExecutionMode::NativeSingleTargetV1.validate(&p).is_ok());
    p.targets.push(p.targets[0].clone());
    assert!(ExecutionMode::NativeImagingV1.validate(&p).is_ok());
    assert!(ExecutionMode::NativeSingleTargetV1.validate(&p).is_err());
    p.targets.pop();
    p.configuration.enable_slew_center = false;
    assert!(ExecutionMode::NativeImagingV1.validate(&p).is_err());
    p.targets[0].position_angle_mas = None;
    assert!(ExecutionMode::NativeImagingV1.validate(&p).is_ok());
    p.targets.clear();
    assert!(ExecutionMode::NativeImagingV1.validate(&p).is_err());
}

#[test]
fn moon_rules_require_a_capable_executor_even_on_historical_requests() {
    use crate::server::director::workload::ExecutionMode;
    let mut p: psf_guard_director_core::program::Program = serde_json::from_str(include_str!(
        "../../../../crates/director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    p.configuration.enable_slew_center = false;
    p.recipes[0].moon = Some(psf_guard_director_core::moon::MoonPolicy {
        enabled: true,
        ..Default::default()
    });
    assert!(ExecutionMode::PreparedTargetV1.validate(&p).is_err());
    assert!(ExecutionMode::LocalSequenceV1.validate(&p).is_err());
    assert!(ExecutionMode::PreparedTargetV2.validate(&p).is_ok());
    assert!(ExecutionMode::LocalSequenceV2.validate(&p).is_ok());
    use psf_guard_director_core::priority::{
        self, Policy, Preset, ProgramPreferences, Scope, Source,
    };
    let policy = priority::resolve(
        Policy::preset(Preset::Balanced),
        Source {
            scope: Scope::Global,
            id: "global".into(),
            revision: 1,
        },
        &[],
    )
    .unwrap();
    p.observing_preferences = Some(ProgramPreferences {
        schema_version: 1,
        policies: [("policy".into(), policy)].into(),
        bindings: [(p.assignment.goals[0].id.clone(), "policy".into())].into(),
    });
    assert!(ExecutionMode::PreparedTargetV2.validate(&p).is_err());
    assert!(ExecutionMode::LocalSequenceV2.validate(&p).is_err());
    assert!(ExecutionMode::PreparedTargetV3.validate(&p).is_ok());
    assert!(ExecutionMode::LocalSequenceV3.validate(&p).is_ok());
}

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
