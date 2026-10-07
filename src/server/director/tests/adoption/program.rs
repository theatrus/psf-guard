use super::activation::activated;
use super::*;
use axum::body::to_bytes;

async fn raw_get(
    app: &Router,
    path: &str,
    etag: Option<&str>,
) -> (StatusCode, Option<String>, Value) {
    let mut request = Request::builder().uri(format!("/api/director/v1{path}"));
    if let Some(etag) = etag {
        request = request.header("if-none-match", etag);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    (
        status,
        etag,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn the_plugin_pulls_a_program_built_from_activation_and_its_own_equipment() {
    let a = activated().await;
    let instance = a.f.state.director.as_ref().unwrap().instance_id;
    let catalog = {
        let store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut found = None;
        // The rig binding minted the rig id from the catalog id; find the catalog that maps to it.
        for id in [a.rig] {
            if store.catalog_rig(id).unwrap().is_some() {
                found = Some(id);
            }
        }
        found.expect("catalog bound to rig")
    };
    let path = format!(
        "/rigs/{}/program?coordinator_instance_id={instance}&catalog_id={catalog}",
        a.rig
    );

    // Activate first (the fixture only prepares the drafts).
    let (status, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest": preview["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");

    // No equipment report yet: named, not guessed.
    let (status, _, body) = raw_get(&a.f.app, &path, None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"].as_str().unwrap().contains("equipment"));

    let program: Value = serde_json::from_str(include_str!(
        "../../../../../crates/director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    let mut configuration = program["configuration"].clone();
    configuration["rig_id"] = json!(a.rig.to_string());
    configuration["filters"][0]["id"] = json!("filter-2");
    configuration["offset"] = json!({"support":"range","minimum":0,"maximum":100});
    let (status, reported) = call(&a.f.app, "PUT", &format!("/rigs/{}/equipment", a.rig), json!({
        "coordinator_instance_id": instance, "catalog_id": catalog, "configuration": configuration,
        "filter_names": {"filter-2":"Ha"},
        "optics": {"sensor_width_px": 6248, "sensor_height_px": 4176, "pixel_size_um": 3.76, "focal_length_mm": 250.0, "aperture_mm": 51.0, "rotation": {"mode":"rotator"}},
        "site": {"latitude_degrees": 34.2, "longitude_degrees": -118.3, "elevation_meters": 400.0},
        "horizon": null, "limits": null, "reported_at_ms": 1_700_000_000_000u64,
    }), None).await;
    assert_eq!(status, StatusCode::OK, "{reported}");

    let (status, etag, body) = raw_get(&a.f.app, &path, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let etag = etag.expect("etag");
    let data = &body["data"];
    assert_eq!(data["rig_id"], a.rig.to_string());
    assert_eq!(data["catalog_id"], catalog.to_string());
    assert_eq!(format!("\"{}\"", data["revision"].as_str().unwrap()), etag);
    let goals = data["program"]["assignment"]["goals"].as_array().unwrap();
    assert_eq!(goals.len(), 2, "{goals:?}");
    for goal in goals {
        assert_eq!(goal["requested"], 72);
        assert_eq!(goal["accepted"], 0);
        assert_eq!(goal["attempts_remaining"], 108);
        assert_eq!(goal["exposure_ms"], 300_000);
        assert_eq!(goal["eligible_windows"].as_array().unwrap().len(), 1);
    }
    // A row left at -1 runs at its template's default length, and the
    // program says so instead of asking for a -1 s exposure.
    a.db.execute("UPDATE exposureplan SET exposure=-1", [])
        .unwrap();
    let (status, _, defaulted) = raw_get(&a.f.app, &path, None).await;
    assert_eq!(status, StatusCode::OK, "{defaulted}");
    for goal in defaulted["data"]["program"]["assignment"]["goals"]
        .as_array()
        .unwrap()
    {
        assert_eq!(goal["exposure_ms"], 300_000, "{goal}");
    }
    a.db.execute("UPDATE exposureplan SET exposure=300", [])
        .unwrap();
    let assignment = &data["program"]["assignment"];
    assert_eq!(assignment["rig_id"], a.rig.to_string());
    assert_eq!(assignment["configuration_id"], configuration["id"]);
    assert!(
        assignment["expires_at_ms"].as_u64().unwrap()
            - assignment["valid_from_ms"].as_u64().unwrap()
            == 24 * 3600 * 1000
    );
    let targets = data["program"]["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 2);
    assert!(targets.iter().any(|t| t["name"] == "IC 1805 r1c1"));
    // A rotator was reported, so the camera angle travels with the target.
    assert_eq!(targets[0]["position_angle_mas"], 15 * 3_600_000);
    assert!(
        (targets[0]["icrs_dec_mas"].as_i64().unwrap() as f64 / 3_600_000.0 - 61.45).abs() < 1.0
    );
    let recipes = data["program"]["recipes"].as_array().unwrap();
    assert_eq!(recipes.len(), 1);
    assert_eq!(recipes[0]["filter_id"], "filter-2");
    assert_eq!(recipes[0]["exposure_ms"], 300_000);
    assert_eq!(
        recipes[0]["gain"],
        configuration["gain"]["maximum"]
            .as_i64()
            .map(|max| json!(max.min(100)))
            .unwrap_or(json!(100))
    );
    assert_eq!(data["program"]["bindings"].as_array().unwrap().len(), 2);
    let links = data["links"].as_array().unwrap();
    assert_eq!(links.len(), 2);
    assert_eq!(links[0]["project_id"], a.project.to_string());
    assert_eq!(links[0]["objective_id"], a.objective.to_string());
    assert_eq!(links[0]["bandpass_id"], "h_alpha");
    assert!(
        links.iter().any(|l| l["panel_id"] == "r1c1")
            && links.iter().any(|l| l["panel_id"] == "r2c1")
    );
    assert_eq!(data["omitted"], json!([]));
    assert_eq!(data["rig"]["site"]["latitude_degrees"], 34.2);
    assert_eq!(data["rig"]["rotation"], json!({"mode":"rotator"}));

    // The actual server output must fit the same geometry core the sidecar
    // uses, not just the less restrictive program-shape validator.
    {
        use psf_guard_director_core::{
            geometry::{BoundGeometry, Constraints, GoalLimits, RigConstraints},
            program::{BoundProgram, Program},
            visibility::{EarthOrientation, Horizon, Site},
            Safety, State,
        };
        let program: Program = serde_json::from_value(data["program"].clone()).unwrap();
        let assignment = &program.assignment;
        let state = State {
            rig_id: assignment.rig_id.clone(),
            configuration_id: assignment.configuration_id.clone(),
            now_ms: assignment.valid_from_ms,
            conditions_valid_until_ms: assignment.expires_at_ms,
            completion_deadline_ms: None,
            safety: Safety::Unknown,
            at_boundary: true,
            operator_stop: false,
            meridian_exclusion: psf_guard_director_core::windows::MeridianExclusion {
                before_ms: 0,
                after_ms: 0,
            },
        };
        let constraints = Constraints {
            schema_version: 1,
            rig: RigConstraints {
                rig_id: state.rig_id.clone(),
                configuration_id: state.configuration_id.clone(),
                revision: 1,
                site: Site {
                    latitude_degrees: 34.2,
                    longitude_degrees: -118.3,
                    elevation_meters: 400.0,
                },
                orientation: EarthOrientation {
                    ut1_minus_utc_seconds: 0.0,
                    polar_motion_x_radians: 0.0,
                    polar_motion_y_radians: 0.0,
                    valid_from_ms: assignment.valid_from_ms,
                    valid_until_ms: assignment.expires_at_ms + 1,
                },
                horizon: Horizon::FixedMinimum {},
                minimum_altitude_degrees: 20.0,
                maximum_altitude_degrees: 89.0,
                meridian_exclusion: state.meridian_exclusion,
            },
            goals: assignment
                .goals
                .iter()
                .map(|g| GoalLimits {
                    goal_id: g.id.clone(),
                    minimum_altitude_degrees: 20.0,
                    maximum_altitude_degrees: 89.0,
                    horizon_offset_degrees: 0.0,
                })
                .collect(),
        };
        BoundGeometry::new(
            BoundProgram::new(program, &state).unwrap(),
            constraints,
            &state,
        )
        .unwrap();
    }

    // Unchanged inputs answer 304 with the same tag.
    let (status, again, _) = raw_get(&a.f.app, &path, Some(&etag)).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert_eq!(again.as_deref(), Some(etag.as_str()));
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let (_, _, repeated) = raw_get(&a.f.app, &path, None).await;
    assert_eq!(
        repeated, body,
        "unconditional retries must preserve the entire envelope"
    );

    for (change, restore, reason) in [
        (
            "UPDATE exposuretemplate SET gain=101",
            "UPDATE exposuretemplate SET gain=100",
            "gain",
        ),
        (
            "UPDATE exposuretemplate SET offset=101",
            "UPDATE exposuretemplate SET offset=30",
            "offset",
        ),
        (
            "UPDATE exposuretemplate SET bin=2",
            "UPDATE exposuretemplate SET bin=1",
            "binning",
        ),
        (
            "UPDATE exposuretemplate SET bin=65537",
            "UPDATE exposuretemplate SET bin=1",
            "binning",
        ),
        (
            "UPDATE exposuretemplate SET readoutmode=65536",
            "UPDATE exposuretemplate SET readoutmode=-1",
            "readout",
        ),
        (
            "UPDATE exposureplan SET exposure=0",
            "UPDATE exposureplan SET exposure=300",
            "exposure",
        ),
        (
            "UPDATE exposureplan SET exposure=601",
            "UPDATE exposureplan SET exposure=300",
            "exposure",
        ),
        (
            "UPDATE target SET active=0",
            "UPDATE target SET active=1",
            "inactive",
        ),
        (
            "UPDATE project SET state=2",
            "UPDATE project SET state=1",
            "inactive",
        ),
        (
            "UPDATE target SET dec=91",
            "UPDATE target SET dec=61.45",
            "target",
        ),
        (
            "UPDATE target SET epochcode=1",
            "UPDATE target SET epochcode=2",
            "target",
        ),
    ] {
        a.db.execute(change, []).unwrap();
        let (status, _, rejected) = raw_get(&a.f.app, &path, None).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{change}: {rejected}"
        );
        assert!(
            rejected["error"].as_str().unwrap().contains(reason),
            "{rejected}"
        );
        a.db.execute(restore, []).unwrap();
    }

    // Progress in the rig database changes the goal and the tag.
    a.db.execute("UPDATE exposureplan SET accepted=10 WHERE Id=(SELECT min(Id) FROM exposureplan WHERE desired=72)", []).unwrap();
    let (status, fresh, body) = raw_get(&a.f.app, &path, Some(&etag)).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(fresh.as_deref(), Some(etag.as_str()));
    let goals = body["data"]["program"]["assignment"]["goals"]
        .as_array()
        .unwrap();
    assert!(
        goals
            .iter()
            .any(|g| g["accepted"] == 10 && g["attempts_remaining"] == 93),
        "{goals:?}"
    );

    // A row the program cannot read costs that goal only: an enabled Moon
    // rule out of range on one template, or a NULL count.
    a.db.execute("UPDATE exposureplan SET desired=NULL WHERE Id=(SELECT max(Id) FROM exposureplan WHERE desired=72)", []).unwrap();
    let (status, _, body) = raw_get(&a.f.app, &path, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["program"]["assignment"]["goals"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "a NULL desired is nothing owed: {body}"
    );
    a.db.execute(
        "UPDATE exposureplan SET desired=72 WHERE desired IS NULL",
        [],
    )
    .unwrap();

    // A plan edited after activation leaves the program as activated: the
    // rig keeps working the reviewed plan until the next activation.
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut draft = store.plan_draft(a.project).unwrap().unwrap();
        let revision = draft.revision;
        draft.objectives[0].priority = 7;
        store.save_plan_draft(&draft, revision).unwrap();
    }
    let (status, _, body) = raw_get(&a.f.app, &path, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let goals = body["data"]["program"]["assignment"]["goals"]
        .as_array()
        .unwrap();
    assert_eq!(goals.len(), 2, "{body}");
    assert_eq!(body["data"]["omitted"], json!([]), "{body}");
    assert_eq!(body["data"]["links"][0]["bandpass_id"], "h_alpha");
    // A finished goal is left out, and said so.
    a.db.execute("UPDATE exposureplan SET accepted=desired WHERE Id=(SELECT min(Id) FROM exposureplan WHERE desired=72)", []).unwrap();
    let (status, _, body) = raw_get(&a.f.app, &path, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["program"]["assignment"]["goals"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{body}"
    );
    assert_eq!(
        body["data"]["program"]["bindings"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{body}"
    );
    assert!(
        body["data"]["omitted"]
            .to_string()
            .contains("1 finished goal left out"),
        "{body}"
    );
    a.db.execute(
        "UPDATE exposureplan SET accepted=10 WHERE accepted=desired AND desired=72",
        [],
    )
    .unwrap();

    // The tuple is checked before anything is read.
    let wrong_instance = format!(
        "/rigs/{}/program?coordinator_instance_id={}&catalog_id={catalog}",
        a.rig,
        Uuid::new_v4()
    );
    assert_eq!(
        raw_get(&a.f.app, &wrong_instance, None).await.0,
        StatusCode::FORBIDDEN
    );
    let wrong_catalog = format!(
        "/rigs/{}/program?coordinator_instance_id={instance}&catalog_id={}",
        a.rig,
        Uuid::new_v4()
    );
    assert_eq!(
        raw_get(&a.f.app, &wrong_catalog, None).await.0,
        StatusCode::FORBIDDEN
    );
    let wrong_rig = format!(
        "/rigs/{}/program?coordinator_instance_id={instance}&catalog_id={catalog}",
        Uuid::new_v4()
    );
    assert_eq!(
        raw_get(&a.f.app, &wrong_rig, None).await.0,
        StatusCode::FORBIDDEN
    );
    // Saving an order replaces score policies in newly issued programs, but
    // never rewrites the already-issued allocation snapshots above.
    {
        use psf_guard_director_core::priority::Scope;
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut settings = store.observing_settings(Scope::Global, instance).unwrap();
        settings.enabled = Some(true);
        settings.project_order = Some(vec![a.project]);
        store.save_observing_settings(&settings).unwrap();
    }
    let (status, ranked_etag, body) = raw_get(&a.f.app, &path, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_ne!(ranked_etag, Some(etag.clone()));
    assert!(body["data"]["program"]["observing_preferences"].is_null());
    let ranked_program: psf_guard_director_core::program::Program =
        serde_json::from_value(body["data"]["program"].clone()).unwrap();
    assert!(ranked_program
        .assignment
        .goals
        .iter()
        .all(|goal| goal.priority > 0));

    // An unactivated draft edit must not change priorities in an active
    // program: the rig keeps serving the activation as reviewed.
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut plan = store.plan_draft(a.project).unwrap().unwrap();
        plan.objectives[0].priority += 1;
        store.save_plan_draft(&plan, plan.revision).unwrap();
    }
    let (status, _, body) = raw_get(&a.f.app, &path, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after: psf_guard_director_core::program::Program =
        serde_json::from_value(body["data"]["program"].clone()).unwrap();
    let priorities = |program: &psf_guard_director_core::program::Program| {
        program
            .assignment
            .goals
            .iter()
            .map(|g| (g.id.clone(), g.priority))
            .collect::<Vec<_>>()
    };
    assert_eq!(priorities(&after), priorities(&ranked_program));
}
