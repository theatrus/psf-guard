use super::rig_profile::bound_fixture;
use super::*;

#[tokio::test]
async fn templates_list_every_profile_with_a_bandpass_and_plans_use_compare_and_set() {
    let (f, rig) = bound_fixture().await;
    f.source
        .execute_batch(
            "CREATE TABLE exposuretemplate(Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT, filtername TEXT,
                gain INTEGER, offset INTEGER, bin INTEGER, readoutmode INTEGER, defaultexposure REAL, guid TEXT);
             INSERT INTO exposuretemplate VALUES(1,'profile-a','Ha 300','Ha',100,30,1,-1,300.0,'6f1a1a1a-1111-4111-8111-111111111111');
             INSERT INTO exposuretemplate VALUES(2,'profile-a','Lum','L',-1,-1,1,NULL,120.0,NULL);
             INSERT INTO exposuretemplate VALUES(3,'profile-b','O3 6.5nm','OIII 6.5nm',100,30,2,0,NULL,'6f1a1a1a-2222-4222-8222-222222222222');
             INSERT INTO exposuretemplate VALUES(4,'profile-a','Blank','',0,0,1,0,60.0,NULL);",
        )
        .unwrap();
    let (status, listed) = call(
        &f.app,
        "GET",
        "/catalogs/catalog/templates",
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let data = &listed["data"];
    assert_eq!(data["rig"]["id"], rig.to_string());
    let templates = data["templates"].as_array().unwrap();
    assert_eq!(templates.len(), 3, "{templates:?}");
    let by_name = |name: &str| templates.iter().find(|t| t["name"] == name).unwrap();
    assert_eq!(
        by_name("Ha 300")["bandpass"],
        json!({"id":"h_alpha","name":"H-alpha","kind":"narrowband"})
    );
    assert_eq!(
        by_name("Ha 300")["guid"],
        "6f1a1a1a-1111-4111-8111-111111111111"
    );
    assert_eq!(by_name("Lum")["bandpass"]["id"], "luminance");
    assert_eq!(by_name("Lum")["gain"], Value::Null);
    assert_eq!(by_name("O3 6.5nm")["bandpass"]["id"], "oiii");
    assert_eq!(by_name("O3 6.5nm")["default_exposure"], 60.0);
    assert_eq!(by_name("O3 6.5nm")["profile_id"], "profile-b");
    assert_eq!(
        call(
            &f.app,
            "GET",
            "/catalogs/missing/templates",
            Value::Null,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    let (_, rigs) = call(&f.app, "GET", "/rigs/profiles", Value::Null, None).await;
    let defaults = &rigs["data"][0]["default_exposure_seconds"];
    assert_eq!(defaults["broadband"], 120.0);
    assert_eq!(defaults["narrowband"], 300.0);

    let project = {
        let mut store = f.state.director.as_ref().unwrap().writer.lock().unwrap();
        store.create_project(Uuid::new_v4(), "Heart").unwrap().id
    };
    let path = format!("/projects/{project}/plan");
    let (status, empty) = call(&f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["data"]["plan"], Value::Null);
    let objective = Uuid::new_v4();
    let plan = |revision: u64| {
        json!({
            "project_id": project, "revision": revision, "updated_at_ms": 0,
            "objectives": [{"id": objective, "bandpass_id": "h_alpha", "purpose": "faint_detail", "goal": {"kind":"hours","value":6.0}, "priority": 1}],
            "contributions": [{"id": Uuid::new_v4(), "objective_id": objective, "rig_id": rig,
                "template": {"template_guid": "6f1a1a1a-1111-4111-8111-111111111111", "template_id": 1, "name": "Ha 300", "filter_name": "Ha", "gain": 100, "offset": 30, "bin": 1, "readout_mode": null},
                "exposure_seconds": 300.0, "panel_ids": [], "enabled": true}],
        })
    };
    let (status, saved) = call(&f.app, "PUT", &path, plan(0), None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["data"]["plan"]["revision"], 1);
    assert_eq!(
        call(&f.app, "PUT", &path, plan(0), None).await.0,
        StatusCode::CONFLICT
    );
    let mut wrong = plan(1);
    wrong["project_id"] = json!(Uuid::new_v4());
    assert_eq!(
        call(&f.app, "PUT", &path, wrong, None).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut dangling = plan(1);
    dangling["contributions"][0]["rig_id"] = json!(Uuid::new_v4());
    assert_eq!(
        call(&f.app, "PUT", &path, dangling, None).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, read) = call(&f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(
        read["data"]["plan"]["objectives"][0]["goal"],
        json!({"kind":"hours","value":6.0})
    );
    assert_eq!(
        call(
            &f.app,
            "GET",
            &format!("/projects/{}/plan", Uuid::new_v4()),
            Value::Null,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

/// A template whose Moon rules Director cannot plan with is set apart by
/// name; it fails neither the rig's template list nor the plans of the rig's
/// other projects. With avoidance off its other Moon values mean nothing,
/// as in Target Scheduler, and the template is used.
#[tokio::test]
async fn a_template_with_unusable_moon_rules_is_named_and_left_out_alone() {
    let f = Fixture::new();
    let path = f._dir.path().join("moon.sqlite");
    let db = crate::ts_schema::create_fresh_db(&path).unwrap();
    db.execute_batch(
        "INSERT INTO project (Id, profileId, name, description, state, priority, isMosaic, flatsHandling, guid)
         VALUES (1, 'profile-x', 'Pelican', '', 1, 1, 0, 0, '0b0b0b0b-1111-4111-8111-111111111111');
         INSERT INTO exposuretemplate (Id, profileId, name, filtername, gain, offset, bin, readoutmode, twilightlevel, moonavoidanceenabled,
            moonavoidanceseparation, moonavoidancewidth, maximumhumidity, defaultexposure, moonrelaxscale, moonrelaxmaxaltitude,
            moonrelaxminaltitude, moondownenabled, ditherevery, minutesOffset, guid)
         VALUES (1, 'profile-x', 'Ha 300', 'Ha', 100, 30, 1, -1, 0, 1, 60, 7, 0, 300, 0, 5, -15, 0, -1, 0, 'tmpl-ha'),
                (2, 'profile-x', 'OIII off', 'OIII', 100, 30, 1, -1, 0, 0, 60, 30, 0, 180, 0, -15, 5, 1, -1, 0, 'tmpl-o3'),
                (3, 'profile-x', 'SII wide', 'SII', 100, 30, 1, -1, 0, 1, 60, 30, 0, 180, 0, 5, -15, 0, -1, 0, 'tmpl-s2');
         INSERT INTO target (Id, name, active, ra, dec, epochcode, rotation, roi, projectid, guid)
         VALUES (1, 'IC 5070', 1, 20.85, 44.35, 2, 0.0, 100, 1, 'tgt-1');
         INSERT INTO exposureplan (profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, enabled, guid)
         VALUES ('profile-x', 300, 30, 0, 0, 1, 1, 1, 'ep-1'), ('profile-x', 180, 20, 0, 0, 1, 2, 1, 'ep-2'),
                ('profile-x', 180, 10, 0, 0, 1, 3, 1, 'ep-3');",
    )
    .unwrap();
    let context = crate::server::database_context::DatabaseContext::new(
        "moon".into(),
        "Moon rig".into(),
        path.to_string_lossy().into(),
        vec![f._dir.path().to_string_lossy().into()],
        None,
        None,
        None,
        f._dir.path().join("cache"),
    )
    .unwrap();
    f.state
        .databases
        .write()
        .unwrap()
        .insert("moon".into(), Arc::new(context));

    let (status, listed) = call(&f.app, "GET", "/catalogs/moon/templates", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let names: Vec<&str> = listed["data"]["templates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Ha 300", "OIII off"], "{listed}");
    let off = &listed["data"]["templates"][1]["moon"];
    assert_eq!(off["enabled"], false, "{off}");
    assert_eq!(off["moon_down"], true);
    assert_eq!(off["width_days"], 7.0);
    assert_eq!(
        listed["data"]["warnings"],
        json!(["Template SII wide: its Moon avoidance settings are outside what Director plans with, so it is left out. Fix them in Target Scheduler."])
    );

    // The project still gets its plan, from the templates Director can use.
    let (status, plans) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{plans}");
    assert!(
        plans["data"]["warnings"].as_array().unwrap().iter().any(|w| w
            .as_str()
            .unwrap()
            .contains("Moon rig: Pelican: its exposure plans with template SII wide were left out of its plan")),
        "{plans}"
    );
    let row = plans["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["project"]["name"] == "Pelican")
        .unwrap()
        .clone();
    assert_eq!(row["plan"]["objectives"], 2, "{row}");
}
