use super::rig_profile::bound_fixture;
use super::*;

fn draft(project: Uuid, revision: u64) -> Value {
    json!({
        "project_id": project, "revision": revision, "target_name": "M31",
        "center": {"ra_degrees": 10.6847, "dec_degrees": 41.269},
        "position_angle_degrees": 35.0,
        "mosaic": {"rows": 2, "columns": 1, "overlap_percent": 15},
        "panel_rig_id": null, "panel": {"width_degrees": 2.0, "height_degrees": 1.5},
        "shown_rig_ids": [], "survey_id": "dss2_color", "view_fov_degrees": 6.0,
        "updated_at_ms": 0,
    })
}

#[tokio::test]
async fn framing_previews_are_stateless_and_drafts_use_compare_and_set() {
    let f = Fixture::new();
    let request = json!({
        "center": {"ra_degrees": 10.6847, "dec_degrees": 41.269},
        "position_angle_degrees": 0.0,
        "panel": {"width_degrees": 2.0, "height_degrees": 1.5},
        "mosaic": {"rows": 2, "columns": 2, "overlap_percent": 20},
        "overlays": [{"id": "rig-b", "size": {"width_degrees": 5.0, "height_degrees": 3.3}, "position_angle_degrees": 90.0}],
        "view": {"center": {"ra_degrees": 10.6847, "dec_degrees": 41.269}, "rotation_degrees": 0.0},
    });
    let (status, preview) = call(&f.app, "POST", "/framing/preview", request.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["data"]["panels"].as_array().unwrap().len(), 4);
    assert_eq!(preview["data"]["panels"][3]["id"], "r2c2");
    assert!(preview["data"]["panels"][0]["view_corners"].is_array());
    assert_eq!(preview["data"]["overlays"][0]["id"], "rig-b");
    let mut bad = request.clone();
    bad["mosaic"]["rows"] = json!(0);
    assert_eq!(
        call(&f.app, "POST", "/framing/preview", bad, None).await.0,
        StatusCode::BAD_REQUEST
    );

    let project = Uuid::parse_str(f.plan["mappings"][0]["project_id"].as_str().unwrap()).unwrap();
    let path = format!("/projects/{project}/framing");
    let (status, empty) = call(&f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["data"]["project"]["name"], "Global project");
    assert_eq!(empty["data"]["draft"], Value::Null);
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/projects/{}/framing", Uuid::new_v4()),
            Value::Null,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    let (status, saved) = call(&f.app, "PUT", &path, draft(project, 0), None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["data"]["draft"]["revision"], 1);
    assert!(saved["data"]["draft"]["updated_at_ms"].as_u64().unwrap() > 1_600_000_000_000);
    assert_eq!(
        call(&f.app, "PUT", &path, draft(project, 0), None).await.0,
        StatusCode::CONFLICT
    );
    let mut other_project = draft(project, 1);
    other_project["project_id"] = json!(Uuid::new_v4());
    assert_eq!(
        call(&f.app, "PUT", &path, other_project, None).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut turned = draft(project, 1);
    turned["position_angle_degrees"] = json!(40.0);
    let (status, second) = call(&f.app, "PUT", &path, turned, None).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["data"]["draft"]["revision"], 2);
    let (_, read) = call(&f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(read["data"]["draft"]["position_angle_degrees"], 40.0);
}

#[tokio::test]
async fn rig_profiles_list_every_bound_database_with_its_field() {
    let (f, rig) = bound_fixture().await;
    let (status, listed) = call(&f.app, "GET", "/rigs/profiles", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let rows = listed["data"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["rig"]["id"], rig.to_string());
    assert_eq!(rows[0]["catalog_slug"], "catalog");
    assert_eq!(rows[0]["profile"], Value::Null);
    assert_eq!(rows[0]["field_of_view"], Value::Null);

    let (_, view) = call(
        &f.app,
        "GET",
        "/catalogs/catalog/rig/profile",
        Value::Null,
        None,
    )
    .await;
    let optics = &view["data"]["defaults"]["optics"];
    let edit = json!({
        "expected_revision": 0,
        "optics": {"value": optics["value"], "source": optics["source"]},
        "site": null, "horizon": null, "sky_quality": null,
        "limits": {"value": {"minimum_altitude_degrees": 20.0, "maximum_altitude_degrees": 90.0,
            "meridian_exclusion": {"before_ms": 0, "after_ms": 0}}, "source": {"kind":"manual"}},
    });
    assert_eq!(
        call(&f.app, "PUT", "/catalogs/catalog/rig/profile", edit, None)
            .await
            .0,
        StatusCode::OK
    );
    let (_, listed) = call(&f.app, "GET", "/rigs/profiles", Value::Null, None).await;
    let rows = listed["data"].as_array().unwrap();
    assert_eq!(rows[0]["profile"]["revision"], 1);
    assert!((rows[0]["field_of_view"]["width_degrees"].as_f64().unwrap() - 5.384).abs() < 0.01);

    // An unbound database is left out rather than failing the list.
    let unbound = Fixture::new();
    let (status, listed) = call(&unbound.app, "GET", "/rigs/profiles", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["data"], json!([]));
}
