use super::pairing::{client_call, credential, fixture};
use super::*;

#[tokio::test]
async fn personal_access_tokens_cannot_accept_equipment_reports() {
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
    let (code, _) = call(&app,"POST",&format!("/rigs/{rig}/equipment-reports/{}/accept",Uuid::new_v4()),
        json!({"coordinator_instance_id":instance,"catalog_id":catalog,"report_id":Uuid::new_v4(),"expected_revision":0}),Some(&pat)).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn paired_native_reports_apply_automatically_and_preserve_work_permission() {
    let (_dir, state, app, instance, catalog, rig) = fixture().await;
    let profile = Uuid::new_v4();
    let paired = credential(&app, instance, catalog, rig, profile).await;
    let token = paired["token"].as_str().unwrap();
    let client = paired["client_id"].as_str().unwrap();
    let mut program: psf_guard_director_core::program::Program = serde_json::from_str(
        include_str!("../../../../crates/director-core/tests/fixtures/execution-program.json"),
    )
    .unwrap();
    program.configuration.rig_id = rig.to_string();
    let report = Uuid::new_v4();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let input = json!({"coordinator_instance_id":instance,"catalog_id":catalog,"report_id":report,"observed_at_ms":now,
        "configuration":program.configuration,"filter_names":{"ha":"H-alpha"}});
    let path = format!("/rigs/{rig}/equipment-reports");
    let accept = format!("{path}/{client}/accept");
    // Pairing made the rig's profile, to set its mode to Director.
    let review = json!({"coordinator_instance_id":instance,"catalog_id":catalog,"report_id":report,"expected_revision":1});
    assert_ne!(
        call(&app, "POST", &path, input.clone(), None).await.0,
        StatusCode::OK
    );
    state.set_anonymous_access_trusted(false);
    for bad in [None, Some(Uuid::new_v4())] {
        assert_eq!(
            client_call(&app, "POST", &path, input.clone(), token, bad)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    let (code, ack) = client_call(&app, "POST", &path, input.clone(), token, Some(profile)).await;
    assert_eq!(code, StatusCode::OK, "{ack}");
    assert_eq!(ack["data"]["report_id"], report.to_string());
    assert_eq!(ack["data"]["accepted_revision"], 2);
    let service = state.director.clone().unwrap();
    // Pairing authorizes reports for this exact rig, catalog and profile.
    let paired = service
        .clone()
        .query(move |s| s.rig_profile(rig))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (paired.revision, paired.configuration.is_some(), paired.mode),
        (
            2,
            true,
            Some(psf_guard_director_meta::profile::RigMode::Director)
        )
    );
    let policy = service
        .clone()
        .query(move |s| s.workload_policy(rig))
        .await
        .unwrap()
        .unwrap();
    assert!(policy.enabled);
    assert_eq!(policy.profile_revision, paired.revision);
    assert!(policy.project_ids.is_empty());
    // Explicitly turning work off survives both repeated and changed reports.
    let mut disabled = policy.clone();
    disabled.enabled = false;
    disabled.revision += 1;
    service
        .clone()
        .with_writer(move |s| Ok::<_, Error>(s.save_workload_policy(&disabled, policy.revision)?))
        .await
        .unwrap();
    assert_eq!(
        client_call(&app, "GET", &path, Value::Null, token, Some(profile))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client_call(&app, "POST", &accept, review.clone(), token, Some(profile))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let mut wrong = input.clone();
    wrong["catalog_id"] = json!(Uuid::new_v4());
    assert_eq!(
        client_call(&app, "POST", &path, wrong, token, Some(profile))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    state.set_anonymous_access_trusted(true);
    let (code, listed) = call(&app, "GET", &path, Value::Null, None).await;
    assert_eq!(code, StatusCode::OK);
    assert!(!listed.to_string().contains(token));
    assert_eq!(listed["data"].as_array().unwrap().len(), 1);
    let (code, accepted) = call(&app, "POST", &accept, review.clone(), None).await;
    assert_eq!(code, StatusCode::OK, "{accepted}");
    assert_eq!(accepted["data"]["revision"], 2);
    assert_eq!(
        call(&app, "POST", &accept, review, None).await.0,
        StatusCode::OK
    );
    let (_, retry) = client_call(&app, "POST", &path, input, token, Some(profile)).await;
    assert_eq!(retry["data"]["accepted_revision"], 2);
    let mut changed = json!({"coordinator_instance_id":instance,"catalog_id":catalog,"report_id":Uuid::new_v4(),"observed_at_ms":now+1,
        "configuration":program.configuration,"filter_names":{"ha":"New filter name"}});
    changed["configuration"]["id"] = json!("changed-equipment");
    let (code, ack) = client_call(&app, "POST", &path, changed, token, Some(profile)).await;
    assert_eq!(code, StatusCode::OK, "{ack}");
    let policy = service
        .clone()
        .query(move |s| s.workload_policy(rig))
        .await
        .unwrap()
        .unwrap();
    assert!(!policy.enabled);
    assert_eq!(
        policy.profile_revision,
        ack["data"]["accepted_revision"].as_u64().unwrap()
    );
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/rigs/{rig}/clients/{client}"),
            Value::Null,
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        client_call(&app, "POST", &path, Value::Null, token, Some(profile))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "GET", &path, Value::Null, None).await.1["data"],
        json!([])
    );
}
