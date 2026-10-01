use super::super::pairing::{client_call, credential};
use super::activation::activated;
use super::*;

#[tokio::test]
async fn only_operator_admits_exact_client_and_retries_never_rebuild_or_replace_the_allocation() {
    let a = activated().await;
    let instance = a.f.state.director.as_ref().unwrap().instance_id;
    let catalog = a.rig;
    let profile = Uuid::new_v4();
    let paired = credential(&a.f.app, instance, catalog, a.rig, profile).await;
    let token = paired["token"].as_str().unwrap();
    let route = format!("/rigs/{}/allocation", a.rig);
    let query = format!("?coordinator_instance_id={instance}&catalog_id={catalog}");
    assert_eq!(
        client_call(
            &a.f.app,
            "GET",
            &format!("{route}{query}"),
            json!(null),
            token,
            Some(profile)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (_, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(
        call(
            &a.f.app,
            "POST",
            &format!("/projects/{}/activation/apply", a.project),
            json!({"preview_digest":preview["data"]["preview_digest"]}),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    let p: Value = serde_json::from_str(include_str!(
        "../../../../../crates/director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    let mut config = p["configuration"].clone();
    config["rig_id"] = json!(a.rig);
    config["filters"][0]["id"] = json!("filter-2");
    config["offset"] = json!({"support":"range","minimum":0,"maximum":100});
    let (status, body) = call(&a.f.app,"PUT",&format!("/rigs/{}/equipment",a.rig),json!({
        "coordinator_instance_id":instance,"catalog_id":catalog,"configuration":config,"filter_names":{"filter-2":"Ha"},
        "optics":{"sensor_width_px":6248,"sensor_height_px":4176,"pixel_size_um":3.76,"focal_length_mm":250.0,"aperture_mm":51.0,"rotation":{"mode":"rotator"}},
        "site":null,"horizon":null,"limits":null,"reported_at_ms":1_700_000_000_000u64}),None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, preview) = call(
        &a.f.app,
        "GET",
        &format!("/rigs/{}/program{query}", a.rig),
        json!(null),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let mut request = json!({"coordinator_instance_id":instance,"catalog_id":catalog,"client_id":paired["client_id"],
        "allocation_id":Uuid::new_v4(),"preview_revision":"0".repeat(64)});
    assert_eq!(
        call(&a.f.app, "POST", &route, request.clone(), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    request["preview_revision"] = preview["data"]["revision"].clone();
    assert_eq!(
        client_call(
            &a.f.app,
            "POST",
            &route,
            request.clone(),
            token,
            Some(profile)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, admitted) = call(&a.f.app, "POST", &route, request.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{admitted}");
    assert_ne!(
        admitted["data"]["snapshot"]["program"]["assignment"]["id"],
        preview["data"]["program"]["assignment"]["id"]
    );
    assert_eq!(
        client_call(
            &a.f.app,
            "GET",
            &format!("{route}{query}"),
            json!(null),
            token,
            Some(profile)
        )
        .await
        .1,
        admitted
    );
    // A second valid pairing for the same rig/profile receives no implicit grant.
    let other = credential(&a.f.app, instance, catalog, a.rig, profile).await;
    assert_eq!(
        client_call(
            &a.f.app,
            "GET",
            &format!("{route}{query}"),
            json!(null),
            other["token"].as_str().unwrap(),
            Some(profile)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    // Even losing the source preview cannot rebuild the already committed grant.
    Connection::open(&a.f.state.director.as_ref().unwrap().path)
        .unwrap()
        .execute(
            "DELETE FROM program_issue WHERE rig_id=?1",
            [a.rig.to_string()],
        )
        .unwrap();
    let mut replacement = request.clone();
    replacement["allocation_id"] = json!(Uuid::new_v4());
    assert_eq!(
        call(&a.f.app, "POST", &route, replacement, None).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&a.f.app, "POST", &route, request.clone(), None)
            .await
            .1,
        admitted
    );
    assert_eq!(
        call(
            &a.f.app,
            "DELETE",
            &format!(
                "/rigs/{}/clients/{}",
                a.rig,
                paired["client_id"].as_str().unwrap()
            ),
            json!(null),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        client_call(
            &a.f.app,
            "GET",
            &format!("{route}{query}"),
            json!(null),
            token,
            Some(profile)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&a.f.app, "POST", &route, request, None).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &a.f.app,
            "GET",
            &format!("{route}{query}"),
            json!(null),
            None
        )
        .await
        .1,
        admitted
    );
}
