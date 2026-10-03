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
pub(super) struct Activated {
    pub(super) f: Fixture,
    pub(super) rig: Uuid,
    pub(super) project: Uuid,
    pub(super) objective: Uuid,
    pub(super) db: Connection,
}

pub(super) async fn activated() -> Activated {
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
        f._dir.path().join("cache"),
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
        let mut store = f.state.director.as_ref().unwrap().writer.lock().unwrap();
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
                    rig_framings: vec![],
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
                            moon: None,
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
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
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
async fn a_rig_that_owns_one_panel_gets_only_that_target_and_gaps_are_named() {
    let a = activated().await;
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut plan = store.plan_draft(a.project).unwrap().unwrap();
        plan.contributions[0].panel_ids = vec!["r2c1".into()];
        store.save_plan_draft(&plan, 1).unwrap();
    }
    let path = format!("/projects/{}/activation/preview", a.project);
    let (status, preview) = call(&a.f.app, "POST", &path, json!({}), None).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let changes = preview["data"]["rigs"][0]["changes"].as_array().unwrap();
    let targets: Vec<&str> = changes
        .iter()
        .filter(|c| c["kind"] == "target")
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(targets, ["IC 1805 r2c1"]);
    assert_eq!(changes.iter().filter(|c| c["kind"] == "plan").count(), 1);
    let warnings = preview["data"]["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w == "No rig shoots h_alpha on panel r1c1."),
        "{warnings:?}"
    );
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest": preview["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    let count = |sql: &str| a.db.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
    assert_eq!(count("SELECT count(*) FROM target"), 1);
    assert_eq!(count("SELECT count(*) FROM exposureplan"), 1);
    // Widening back to every panel adds the missing panel without touching the first.
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut plan = store.plan_draft(a.project).unwrap().unwrap();
        plan.contributions[0].panel_ids = vec![];
        store.save_plan_draft(&plan, 2).unwrap();
    }
    let (_, again) = call(&a.f.app, "POST", &path, json!({}), None).await;
    let actions: Vec<(&str, &str)> = again["data"]["rigs"][0]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "target")
        .map(|c| (c["name"].as_str().unwrap(), c["action"].as_str().unwrap()))
        .collect();
    assert!(
        actions.contains(&("IC 1805 r2c1", "unchanged"))
            && actions.contains(&("IC 1805 r1c1", "create")),
        "{actions:?}"
    );
    assert!(again["data"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .all(|w| !w.as_str().unwrap().starts_with("No rig shoots")));
    let _ = (a.rig, a.objective);
}

#[tokio::test]
async fn activation_names_what_is_missing_and_skips_rigs_it_cannot_write() {
    let a = activated().await;
    let other = {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
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
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
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

#[tokio::test]
async fn side_tables_an_earlier_build_made_strict_are_rebuilt_with_their_rows() {
    let a = activated().await;
    // The shape the previous build wrote, with a row that must survive.
    a.db.execute_batch(
        "CREATE TABLE main.psf_guard_director_plan(
            exposureplan_guid TEXT PRIMARY KEY NOT NULL,
            target_guid TEXT NOT NULL,
            contribution_id TEXT NOT NULL,
            objective_id TEXT NOT NULL,
            bandpass_id TEXT NOT NULL,
            purpose TEXT NOT NULL,
            required_frames INTEGER NOT NULL,
            plan_revision INTEGER NOT NULL,
            UNIQUE(target_guid, contribution_id)) STRICT;
         INSERT INTO psf_guard_director_plan VALUES('old-plan','old-target','c','o','h_alpha','faint_detail',10,1);",
    )
    .unwrap();
    let strict = |name: &str| -> bool {
        let sql: String =
            a.db.query_row("SELECT sql FROM sqlite_schema WHERE name=?1", [name], |r| {
                r.get(0)
            })
            .unwrap();
        sql.to_ascii_uppercase().contains("STRICT")
    };
    assert!(strict("psf_guard_director_plan"));
    let (status, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    // A preview rolls its repair back with everything else.
    assert!(strict("psf_guard_director_plan"));
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest": preview["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    for name in [
        "psf_guard_director_project",
        "psf_guard_director_target",
        "psf_guard_director_plan",
    ] {
        assert!(!strict(name), "{name} is still STRICT");
    }
    let old: i64 = a.db.query_row("SELECT required_frames FROM psf_guard_director_plan WHERE exposureplan_guid='old-plan'", [], |r| r.get(0)).unwrap();
    assert_eq!(old, 10);
    assert_eq!(
        a.db.query_row("SELECT count(*) FROM psf_guard_director_plan", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    // The type checks that replaced STRICT still hold.
    assert!(a
        .db
        .execute(
            "INSERT INTO psf_guard_director_target VALUES('t','p','r9c9','x')",
            []
        )
        .is_err());
    let _ = (a.rig, a.objective);
}

/// A plan adopted from a Target Scheduler project already has that project's
/// targets: activation takes them over by name, by place, or as the only
/// target of a single-panel framing, and never writes a twin beside them.
#[tokio::test]
async fn activation_takes_over_the_linked_projects_existing_targets_instead_of_doubling_them() {
    let a = activated().await;
    let count = |sql: &str| a.db.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
    // The source project, as imported: one profile, two hand-made targets,
    // one named like the first panel and one sitting where the second
    // panel will land, under a name of its own, plus a stranger.
    let source = Uuid::new_v4();
    a.db.execute(
        "INSERT INTO project (Id, profileId, name, description, state, priority, isMosaic, flatsHandling, guid)
         VALUES (1, 'profile-a', 'Heart by hand', '', 1, 1, 0, 0, ?1)",
        [source.to_string()],
    )
    .unwrap();
    // Where the panels land, laid out the way activation lays them out.
    let places: Vec<(f64, f64)> = psf_guard_director_core::framing::FramingRequest {
        center: IcrsPosition {
            ra_degrees: 38.2,
            dec_degrees: 61.45,
        },
        position_angle_degrees: 15.0,
        panel: PanelSize {
            width_degrees: 2.0,
            height_degrees: 1.5,
        },
        mosaic: Mosaic {
            rows: 2,
            columns: 1,
            overlap_percent: 20,
        },
        overlays: vec![],
        view: None,
    }
    .preview()
    .unwrap()
    .panels
    .iter()
    .map(|panel| {
        (
            panel.footprint.center.ra_degrees / 15.0,
            panel.footprint.center.dec_degrees,
        )
    })
    .collect();
    assert_eq!(places.len(), 2);
    a.db.execute(
        "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid, guid)
         VALUES ('IC 1805 r1c1', 1, 0.0, 0.0, 2, 0.0, 100, 1, NULL)",
        [],
    )
    .unwrap();
    a.db.execute(
        "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid, guid)
         VALUES ('Lower half', 1, ?1, ?2, 2, 15.0, 100, 1, ?3)",
        rusqlite::params![places[1].0, places[1].1, Uuid::new_v4().to_string()],
    )
    .unwrap();
    a.db.execute(
        "INSERT INTO project (Id, profileId, name, description, state, priority, isMosaic, flatsHandling, guid)
         VALUES (2, 'profile-a', 'Somewhere else', '', 1, 1, 0, 0, ?1)",
        [Uuid::new_v4().to_string()],
    )
    .unwrap();
    a.db.execute(
        "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid, guid)
         VALUES ('IC 1805 r2c1', 1, 0.0, 0.0, 2, 0.0, 100, 2, ?1)",
        [Uuid::new_v4().to_string()],
    )
    .unwrap();
    // Link the source project to the plan, as adoption from the database does.
    let catalog = crate::catalog_identity::read(&a.db).unwrap().unwrap().id;
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        store
            .link_catalog_project(&psf_guard_director_meta::catalog::ProjectMapping {
                catalog_id: catalog,
                source_project_guid: source,
                source_profile_id: "profile-a".into(),
                project_id: a.project,
                rig_id: a.rig,
            })
            .unwrap();
    }

    let (status, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let data = &preview["data"];
    assert_eq!(actions(data, "project"), ["update"]);
    // The named target moves to its panel; the one already in place is kept as it is.
    assert_eq!(actions(data, "target"), ["update", "adopt"], "{data}");
    let digest = data["preview_digest"].as_str().unwrap().to_owned();
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest": digest}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(
        count("SELECT count(*) FROM target WHERE projectid=1"),
        2,
        "no twin targets"
    );
    assert_eq!(
        count("SELECT count(*) FROM target"),
        3,
        "the other project is untouched"
    );
    assert_eq!(
        count("SELECT count(*) FROM target WHERE name='Lower half' AND rotation=15.0"),
        1
    );
    assert_eq!(count("SELECT count(*) FROM target WHERE name='IC 1805 r1c1' AND projectid=1 AND rotation=15.0 AND ra>2.0 AND guid IS NOT NULL"), 1, "named target moved and given a GUID");
    assert_eq!(
        count("SELECT count(*) FROM target WHERE projectid=2 AND ra=0.0"),
        1,
        "the stranger keeps its place"
    );
    assert_eq!(count("SELECT count(*) FROM psf_guard_director_target"), 2);
    assert_eq!(count("SELECT count(*) FROM exposureplan"), 2);

    // Activating again changes nothing.
    let (_, again) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(
        actions(&again["data"], "target"),
        ["unchanged", "unchanged"]
    );
}

/// A single-panel plan takes over the project's only target whatever it is called.
#[tokio::test]
async fn a_single_panel_plan_takes_over_the_projects_only_target_by_any_name() {
    let a = activated().await;
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut framing = store.framing_draft(a.project).unwrap().unwrap();
        framing.mosaic = Mosaic {
            rows: 1,
            columns: 1,
            overlap_percent: 20,
        };
        let revision = framing.revision;
        store.save_framing_draft(&framing, revision).unwrap();
    }
    let source = Uuid::new_v4();
    a.db.execute(
        "INSERT INTO project (Id, profileId, name, description, state, priority, isMosaic, flatsHandling, guid)
         VALUES (1, 'profile-a', 'Heart by hand', '', 1, 1, 0, 0, ?1)",
        [source.to_string()],
    )
    .unwrap();
    a.db.execute(
        "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid, guid)
         VALUES ('My heart', 1, 1.0, 1.0, 2, 0.0, 100, 1, ?1)",
        [Uuid::new_v4().to_string()],
    )
    .unwrap();
    let catalog = crate::catalog_identity::read(&a.db).unwrap().unwrap().id;
    a.f.state
        .director
        .as_ref()
        .unwrap()
        .writer
        .lock()
        .unwrap()
        .link_catalog_project(&psf_guard_director_meta::catalog::ProjectMapping {
            catalog_id: catalog,
            source_project_guid: source,
            source_profile_id: "profile-a".into(),
            project_id: a.project,
            rig_id: a.rig,
        })
        .unwrap();
    let (status, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(actions(&preview["data"], "target"), ["update"], "{preview}");
    let digest = preview["data"]["preview_digest"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest": digest}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    let count = |sql: &str| a.db.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
    assert_eq!(count("SELECT count(*) FROM target"), 1);
    assert_eq!(count("SELECT count(*) FROM target WHERE name='My heart' AND rotation=15.0 AND abs(ra - 38.2/15.0) < 1e-6 AND abs(dec - 61.45) < 1e-6"), 1);
}

/// A contribution bound to a library template, one the rig database has never
/// seen, gets that template written into the database under the library's own
/// GUID, and a second activation finds it again instead of making another.
#[tokio::test]
async fn a_library_template_is_written_into_the_rig_database_under_its_own_guid() {
    let a = activated().await;
    let library = Uuid::new_v4();
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut plan = store.plan_draft(a.project).unwrap().unwrap();
        plan.contributions[0].template = TemplateChoice {
            template_guid: Some(library),
            template_id: None,
            name: "Ha 600 library".into(),
            moon: Some(psf_guard_director_core::moon::MoonPolicy {
                enabled: true,
                separation_degrees: 80.0,
                width_days: 9.0,
                relax_degrees_per_degree: 2.0,
                moon_down: true,
                ..Default::default()
            }),
            filter_name: "Ha".into(),
            gain: Some(200),
            offset: Some(50),
            bin: Some(2),
            readout_mode: None,
        };
        plan.contributions[0].exposure_seconds = 600.0;
        let revision = plan.revision;
        store.save_plan_draft(&plan, revision).unwrap();
    }
    let count = |sql: &str| a.db.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
    for iteration in 0..4 {
        if iteration == 2 {
            let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
            let mut plan = store.plan_draft(a.project).unwrap().unwrap();
            plan.contributions[0]
                .template
                .moon
                .as_mut()
                .unwrap()
                .separation_degrees = 90.0;
            let revision = plan.revision;
            store.save_plan_draft(&plan, revision).unwrap();
        }
        let (status, preview) = call(
            &a.f.app,
            "POST",
            &format!("/projects/{}/activation/preview", a.project),
            json!({}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        let digest = preview["data"]["preview_digest"]
            .as_str()
            .unwrap()
            .to_owned();
        let (status, applied) = call(
            &a.f.app,
            "POST",
            &format!("/projects/{}/activation/apply", a.project),
            json!({"preview_digest": digest}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{applied}");
        assert_eq!(
            count("SELECT count(*) FROM exposuretemplate"),
            if iteration < 2 { 2 } else { 3 },
            "changed Moon settings clone a shared template once"
        );
        assert_eq!(count(&format!("SELECT count(*) FROM exposuretemplate WHERE guid='{library}' AND name='Ha 600 library' AND filtername='Ha' AND gain=200 AND offset=50 AND bin=2 AND profileId='profile-a'")), 1);
        assert_eq!(count(&format!("SELECT count(*) FROM exposuretemplate WHERE guid='{library}' AND moonavoidanceenabled=1 AND moonavoidanceseparation=80 AND moonavoidancewidth=9 AND moonrelaxscale=2 AND moondownenabled=1")), 1);
        let separation = if iteration < 2 { 80 } else { 90 };
        assert_eq!(count(&format!("SELECT count(*) FROM exposureplan WHERE exposure=600.0 AND exposureTemplateId IN (SELECT Id FROM exposuretemplate WHERE moonavoidanceseparation={separation})")), 2);
    }
}

/// A rig framed on its own shoots its own grid at its own angle over the same
/// target; the shared framing's coverage check leaves it alone.
#[tokio::test]
async fn a_rig_framed_on_its_own_gets_its_own_panels_and_angle() {
    let a = activated().await;
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut framing = store.framing_draft(a.project).unwrap().unwrap();
        framing.rig_framings = vec![psf_guard_director_meta::framing::RigFraming {
            rig_id: a.rig,
            // Its own center, a degree east of the shared one.
            center: Some(IcrsPosition {
                ra_degrees: 39.2,
                dec_degrees: 61.45,
            }),
            position_angle_degrees: Some(90.0),
            mosaic: Mosaic {
                rows: 1,
                columns: 3,
                overlap_percent: 10,
            },
            panel: Some(PanelSize {
                width_degrees: 1.0,
                height_degrees: 0.8,
            }),
        }];
        let revision = framing.revision;
        store.save_framing_draft(&framing, revision).unwrap();
    }
    let count = |sql: &str| a.db.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
    let (status, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(
        actions(&preview["data"], "target"),
        ["create", "create", "create"],
        "{preview}"
    );
    assert_eq!(
        preview["data"]["rigs"][0]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|w| w.as_str().unwrap().contains("No rig shoots"))
            .count(),
        0,
        "{preview}"
    );
    let digest = preview["data"]["preview_digest"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest": digest}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(count("SELECT count(*) FROM target WHERE rotation=90.0"), 3);
    assert_eq!(count("SELECT count(*) FROM target WHERE name IN ('IC 1805 r1c1','IC 1805 r1c2','IC 1805 r1c3')"), 3);
    assert_eq!(
        count("SELECT count(*) FROM exposureplan WHERE desired=72"),
        3
    );
    // The three panels lie along the camera's row, which at 90° runs north-south.
    let (min_dec, max_dec, spread_ra, mean_ra): (f64, f64, f64, f64) =
        a.db.query_row(
            "SELECT min(dec), max(dec), max(ra)-min(ra), avg(ra) FROM target",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert!(max_dec - min_dec > 1.5, "{min_dec} {max_dec}");
    assert!(spread_ra.abs() < 0.05, "{spread_ra}");
    // The row sits on the rig's own center, not the shared one.
    assert!((mean_ra - 39.2 / 15.0).abs() < 1e-3, "{mean_ra}");
}
