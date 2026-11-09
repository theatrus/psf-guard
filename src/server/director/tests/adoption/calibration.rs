use super::super::pairing::{client_call, credential};
use super::*;

#[tokio::test]
async fn calibration_keeps_completed_plan_recipes_and_round_trips_voids_without_reviving_coverage()
{
    let a = activation::activated().await;
    let instance = a.f.state.director.as_ref().unwrap().instance_id;
    let profile = Uuid::new_v4();
    let paired = credential(&a.f.app, instance, a.rig, a.rig, profile).await;
    let token = paired["token"].as_str().unwrap();
    let (_, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest":preview["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    let program: Value = serde_json::from_str(include_str!(
        "../../../../../crates/director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    let mut configuration = program["configuration"].clone();
    configuration["rig_id"] = json!(a.rig);
    configuration["filters"][0]["id"] = json!("filter-2");
    configuration["offset"] = json!({"support":"range","minimum":0,"maximum":100});
    let (status, reported) = call(&a.f.app, "PUT", &format!("/rigs/{}/equipment", a.rig), json!({
        "coordinator_instance_id": instance, "catalog_id": a.rig, "configuration": configuration,
        "filter_names": {"filter-2":"Ha"},
        "optics": {"sensor_width_px":6248,"sensor_height_px":4176,"pixel_size_um":3.76,"focal_length_mm":250.0,"aperture_mm":51.0,"rotation":{"mode":"rotator"}},
        "site": {"latitude_degrees":34.2,"longitude_degrees":-118.3,"elevation_meters":400.0},
        "horizon":null,"limits":null,"reported_at_ms":1700000000000u64
    }), None).await;
    assert_eq!(status, StatusCode::OK, "{reported}");
    a.db.execute("UPDATE exposureplan SET accepted=desired", [])
        .unwrap();
    let path = format!("/rigs/{}/calibration", a.rig);
    let record = json!({"source_row_id":42,"fingerprint":"a".repeat(64),"profile_id":profile,
        "target_guid":null,"target_name":null,"flats_type":"Director panel","filter_name":"Ha",
        "light_session_date":1700000000,"light_session_id":0,"flats_taken_date":1700000000,
        "gain":100,"offset":30,"bin":1,"readout_mode":0,"rotation":15.0,"roi":null});
    let mut request = json!({"protocol_version":1,"coordinator_instance_id":instance,"catalog_id":a.rig,
        "records":[record],"acknowledgements":[]});
    let (status, answer) = client_call(
        &a.f.app,
        "POST",
        &path,
        request.clone(),
        token,
        Some(profile),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["data"]["recipes"].as_array().unwrap().len(), 1);
    assert_eq!(answer["data"]["origin_id"], profile.to_string());
    assert_eq!(answer["data"]["received"], 1);
    assert!(answer["data"].get("assignment").is_none());
    let ctx = a.f.state.databases.read().unwrap()["rig"].clone();
    let id: String =
        a.db.query_row(
            "SELECT record_id FROM psf_guard_scheduler_flat_history",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let _ = crate::server::flat_history::invalidate(
        crate::server::extract::DbContext(ctx.clone()),
        Json(crate::server::flat_history::InvalidateRequest {
            record_ids: vec![id.clone()],
            reason: "Panel reflection".into(),
        }),
    )
    .await
    .unwrap();
    let (_, pending) = client_call(
        &a.f.app,
        "POST",
        &path,
        request.clone(),
        token,
        Some(profile),
    )
    .await;
    assert_eq!(pending["data"]["decisions"][0]["record_id"], id);
    request["acknowledgements"] = json!([{"record_id":id,"source_row_id":42,"fingerprint":"a".repeat(64),"status":"removed","detail":"Image files kept"}]);
    let (status, removed) = client_call(
        &a.f.app,
        "POST",
        &path,
        request.clone(),
        token,
        Some(profile),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed["data"]["acknowledged"], 1);
    assert_eq!(removed["data"]["decisions"], json!([]));
    assert_eq!(removed["data"]["retired"][0]["record_id"], id);
    // Retry a lost response, then restore an old client snapshot: neither revives it.
    assert_eq!(
        client_call(
            &a.f.app,
            "POST",
            &path,
            request.clone(),
            token,
            Some(profile)
        )
        .await
        .0,
        StatusCode::OK
    );
    request["acknowledgements"] = json!([]);
    let (_, replay) = client_call(
        &a.f.app,
        "POST",
        &path,
        request.clone(),
        token,
        Some(profile),
    )
    .await;
    assert_eq!(replay["data"]["retired"][0]["record_id"], id);
    let state: String =
        a.db.query_row(
            "SELECT state FROM psf_guard_scheduler_flat_history",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "removed");
    for field in ["catalog_id", "coordinator_instance_id"] {
        let mut bad = request.clone();
        bad[field] = json!(Uuid::new_v4());
        assert_eq!(
            client_call(&a.f.app, "POST", &path, bad, token, Some(profile))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    let mut bad = request.clone();
    bad["records"][0]["profile_id"] = json!(Uuid::new_v4());
    assert_eq!(
        client_call(&a.f.app, "POST", &path, bad, token, Some(profile))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        client_call(
            &a.f.app,
            "POST",
            &path,
            request.clone(),
            token,
            Some(Uuid::new_v4())
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&a.f.app, "POST", &path, request, None).await.0,
        StatusCode::BAD_REQUEST
    );
}
