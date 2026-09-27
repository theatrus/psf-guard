use super::*;
use psf_guard_director_core::{
    framing::{Mosaic, PanelSize},
    visibility::IcrsPosition,
};
use psf_guard_director_meta::{
    framing::FramingDraft,
    plan::{Contribution, Goal, Objective, PlanDraft, TemplateChoice},
};

/// A real Target Scheduler schema with one profile's H-alpha template, bound
/// to a rig, plus a global project with a two-panel framing and an H-alpha plan.
struct Activated {
    f: Fixture,
    rig: Uuid,
    project: Uuid,
    objective: Uuid,
    db: Connection,
}

async fn activated() -> Activated {
    let f = Fixture::new();
    let path = f._dir.path().join("rig.sqlite");
    let db = crate::ts_schema::create_fresh_db(&path).unwrap();
    db.execute(
        "INSERT INTO exposuretemplate (profileId, name, filtername, gain, offset, bin, readoutmode, twilightlevel, moonavoidanceenabled,
            moonavoidanceseparation, moonavoidancewidth, maximumhumidity, defaultexposure, moonrelaxscale, moonrelaxmaxaltitude,
            moonrelaxminaltitude, moondownenabled, ditherevery, minutesOffset, guid)
         VALUES ('profile-a', 'Ha 300', 'Ha', 100, 30, 1, -1, 0, 0, 60, 7, 0, 300, 0, 5, -15, 0, -1, 0, '7a7a7a7a-1111-4111-8111-111111111111')",
        [],
    )
    .unwrap();
    let context = crate::server::database_context::DatabaseContext::new(
        "rig".into(),
        "RedCat rig".into(),
        path.to_string_lossy().into(),
        vec![f._dir.path().to_string_lossy().into()],
        None,
        None,
        None,
        f._dir.path().join("cache").to_string_lossy().into(),
    )
    .unwrap();
    f.state
        .databases
        .write()
        .unwrap()
        .insert("rig".into(), Arc::new(context));
    let catalog = Uuid::new_v4();
    let (status, reviewed) = call(
        &f.app,
        "POST",
        "/catalogs/rig/rig/preview",
        json!({"catalog_id": catalog}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reviewed}");
    let (status, applied) = call(&f.app, "POST", "/catalogs/rig/rig/apply",
        json!({"plan": {"catalog_id": catalog}, "preview_digest": reviewed["data"]["preview_digest"]}), None).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    let rig = Uuid::parse_str(applied["data"]["binding"]["rig"]["id"].as_str().unwrap()).unwrap();
    let objective = Uuid::new_v4();
    let project = {
        let mut store = f.state.director.as_ref().unwrap().store.lock().unwrap();
        let project = store
            .create_project(Uuid::new_v4(), "Heart Nebula")
            .unwrap()
            .id;
        store
            .save_framing_draft(
                &FramingDraft {
                    project_id: project,
                    revision: 0,
                    target_name: "IC 1805".into(),
                    center: IcrsPosition {
                        ra_degrees: 38.2,
                        dec_degrees: 61.45,
                    },
                    position_angle_degrees: 15.0,
                    mosaic: Mosaic {
                        rows: 2,
                        columns: 1,
                        overlap_percent: 20,
                    },
                    panel_rig_id: Some(rig),
                    panel: Some(PanelSize {
                        width_degrees: 2.0,
                        height_degrees: 1.5,
                    }),
                    shown_rig_ids: vec![],
                    survey_id: "dss2_color".into(),
                    view_fov_degrees: 5.0,
                    updated_at_ms: 1,
                },
                0,
            )
            .unwrap();
        store
            .save_plan_draft(
                &PlanDraft {
                    project_id: project,
                    revision: 0,
                    objectives: vec![Objective {
                        id: objective,
                        bandpass_id: "h_alpha".into(),
                        purpose: "faint_detail".into(),
                        goal: Goal::Hours { value: 6.0 },
                        priority: 1,
                    }],
                    contributions: vec![Contribution {
                        id: Uuid::new_v4(),
                        objective_id: objective,
                        rig_id: rig,
                        template: TemplateChoice {
                            template_guid: Some(
                                Uuid::parse_str("7a7a7a7a-1111-4111-8111-111111111111").unwrap(),
                            ),
                            template_id: Some(1),
                            name: "Ha 300".into(),
                            filter_name: "Ha".into(),
                            gain: Some(100),
                            offset: Some(30),
                            bin: Some(1),
                            readout_mode: None,
                        },
                        exposure_seconds: 300.0,
                        panel_ids: vec![],
                        enabled: true,
                    }],
                    updated_at_ms: 1,
                },
                0,
            )
            .unwrap();
        project
    };
    Activated {
        f,
        rig,
        project,
        objective,
        db,
    }
}

fn actions(report: &Value, kind: &str) -> Vec<String> {
    report["rigs"][0]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == kind)
        .map(|c| c["action"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn activation_previews_without_writing_then_applies_and_updates_in_place() {
    let a = activated().await;
    let preview_path = format!("/projects/{}/activation/preview", a.project);
    let apply_path = format!("/projects/{}/activation/apply", a.project);
    let count = |sql: &str| a.db.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();

    let (status, preview) = call(&a.f.app, "POST", &preview_path, json!({}), None).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let data = &preview["data"];
    assert_eq!(data["applied"], false);
    assert_eq!(data["panels"], 2);
    assert_eq!(data["rigs"][0]["catalog_slug"], "rig");
    assert_eq!(data["rigs"][0]["profile_id"], "profile-a");
    assert_eq!(actions(data, "project"), ["create"]);
    assert_eq!(actions(data, "target"), ["create", "create"]);
    assert_eq!(actions(data, "plan"), ["create", "create"]);
    assert!(data["rigs"][0]["warnings"][0]
        .as_str()
        .unwrap()
        .contains("has not reported this rig's camera"));
    // A preview leaves the rig database untouched.
    assert_eq!(count("SELECT count(*) FROM project"), 0);
    assert_eq!(
        count("SELECT count(*) FROM sqlite_master WHERE name LIKE 'psf_guard_director_%'"),
        0
    );
    let digest = data["preview_digest"].as_str().unwrap().to_owned();

    assert_eq!(
        call(
            &a.f.app,
            "POST",
            &apply_path,
            json!({"preview_digest": "0".repeat(64)}),
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(count("SELECT count(*) FROM project"), 0);

    let (status, applied) = call(
        &a.f.app,
        "POST",
        &apply_path,
        json!({"preview_digest": digest}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["data"]["applied"], true);
    assert_eq!(applied["data"]["activation_revision"], 1);
    assert_eq!(applied["data"]["rigs"][0]["applied"], true);
    assert_eq!(count("SELECT count(*) FROM project WHERE name='Heart Nebula' AND state=1 AND isMosaic=1 AND profileId='profile-a'"), 1);
    assert_eq!(count("SELECT count(*) FROM target WHERE name IN ('IC 1805 r1c1','IC 1805 r2c1') AND rotation=15.0"), 2);
    assert_eq!(count("SELECT count(*) FROM exposureplan WHERE desired=72 AND exposure=300.0 AND exposureTemplateId=1 AND acquired=0 AND accepted=0"), 2);
    assert_eq!(count("SELECT count(*) FROM ruleweight"), 8);
    assert_eq!(count("SELECT count(*) FROM psf_guard_director_target"), 2);
    assert_eq!(count("SELECT count(*) FROM psf_guard_director_plan WHERE required_frames=72 AND bandpass_id='h_alpha'"), 2);
    // Panels sit symmetrically about the center along the tilted camera axis,
    // so their mean is the project center and their declinations straddle it.
    let (mean_ra, min_dec, max_dec): (f64, f64, f64) =
        a.db.query_row("SELECT avg(ra), min(dec), max(dec) FROM target", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert!((mean_ra - 38.2 / 15.0).abs() < 1e-3, "{mean_ra}");
    assert!(min_dec < 61.45 && max_dec > 61.45, "{min_dec} {max_dec}");
    // The new project is linked to the global project for the Overview flow.
    let (_, mappings) = call(&a.f.app, "GET", "/catalogs/rig/mappings", Value::Null, None).await;
    assert_eq!(
        mappings["data"]["items"][0]["project_id"],
        a.project.to_string()
    );
    let (_, last) = call(
        &a.f.app,
        "GET",
        &format!("/projects/{}/activation", a.project),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(last["data"]["activation"]["revision"], 1);
    assert_eq!(
        last["data"]["activation"]["rigs"][0]["targets"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // Nothing changed: a second preview reports every row unchanged.
    let (_, again) = call(&a.f.app, "POST", &preview_path, json!({}), None).await;
    assert_eq!(actions(&again["data"], "project"), ["unchanged"]);
    assert_eq!(
        actions(&again["data"], "target"),
        ["unchanged", "unchanged"]
    );
    assert_eq!(actions(&again["data"], "plan"), ["unchanged", "unchanged"]);

    // Turn the camera and ask for frames instead of hours: rows update in place,
    // and a rename by the operator survives.
    a.db.execute(
        "UPDATE target SET name='Heart top' WHERE name='IC 1805 r1c1'",
        [],
    )
    .unwrap();
    a.db.execute("UPDATE exposureplan SET acquired=5, accepted=3", [])
        .unwrap();
    {
        let mut store = a.f.state.director.as_ref().unwrap().store.lock().unwrap();
        let mut framing = store.framing_draft(a.project).unwrap().unwrap();
        framing.position_angle_degrees = 95.0;
        store.save_framing_draft(&framing, 1).unwrap();
        let mut plan = store.plan_draft(a.project).unwrap().unwrap();
        plan.objectives[0].goal = Goal::Frames { value: 40 };
        store.save_plan_draft(&plan, 1).unwrap();
    }
    let (_, third) = call(&a.f.app, "POST", &preview_path, json!({}), None).await;
    assert_eq!(actions(&third["data"], "target"), ["update", "update"]);
    assert_eq!(actions(&third["data"], "plan"), ["update", "update"]);
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &apply_path,
        json!({"preview_digest": third["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["data"]["activation_revision"], 2);
    assert_eq!(count("SELECT count(*) FROM project"), 1);
    assert_eq!(count("SELECT count(*) FROM target"), 2);
    assert_eq!(
        count("SELECT count(*) FROM target WHERE name='Heart top' AND rotation=95.0"),
        1
    );
    assert_eq!(
        count("SELECT count(*) FROM exposureplan WHERE desired=40 AND acquired=5 AND accepted=3"),
        2
    );
    let _ = a.objective;
}

#[tokio::test]
async fn activation_names_what_is_missing_and_skips_rigs_it_cannot_write() {
    let a = activated().await;
    let other = {
        let mut store = a.f.state.director.as_ref().unwrap().store.lock().unwrap();
        store.create_project(Uuid::new_v4(), "Bare").unwrap().id
    };
    let (status, body) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{other}/activation/preview"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("framing"));
    assert_eq!(
        call(
            &a.f.app,
            "POST",
            &format!("/projects/{}/activation/preview", Uuid::new_v4()),
            json!({}),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // A rig with no registered database is reported, not failed.
    let stray = {
        let mut store = a.f.state.director.as_ref().unwrap().store.lock().unwrap();
        let stray = store.create_rig(Uuid::new_v4(), "Remote").unwrap().id;
        let mut plan = store.plan_draft(a.project).unwrap().unwrap();
        let mut copy = plan.contributions[0].clone();
        copy.id = Uuid::new_v4();
        copy.rig_id = stray;
        plan.contributions.push(copy);
        store.save_plan_draft(&plan, 1).unwrap();
        stray
    };
    let (status, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let rigs = preview["data"]["rigs"].as_array().unwrap();
    assert_eq!(rigs.len(), 2);
    let remote = rigs
        .iter()
        .find(|r| r["rig"]["id"] == stray.to_string())
        .unwrap();
    assert!(remote["warnings"][0]
        .as_str()
        .unwrap()
        .contains("no registered database"));
    assert_eq!(remote["changes"], json!([]));
    let _ = a.rig;
}
