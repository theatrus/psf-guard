use super::activation::{activated, add_rig};
use super::*;

/// The GUID of the first database's project in a deletion preview.
fn guid_of(preview: &Value) -> String {
    let preview = preview.get("data").unwrap_or(preview);
    preview["databases"][0]["source_project_guid"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn activate(a: &super::activation::Activated) {
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
        json!({"preview_digest": preview["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
}

/// A plan activated on a rig that captured nothing: the preview lists the
/// rig's project, and deleting removes it and the plan, and no plan adopts
/// anything in its place.
#[tokio::test]
async fn deleting_a_plan_deletes_its_rig_projects_and_the_plan() {
    let a = activated().await;
    activate(&a).await;
    let path = format!("/projects/{}/deletion", a.project);
    let (status, preview) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let preview = &preview["data"];
    assert_eq!(preview["name"], "Heart Nebula");
    assert_eq!(preview["refusal"], Value::Null);
    let database = &preview["databases"][0];
    assert_eq!(database["catalog_slug"], "rig");
    assert_eq!(database["project"]["targets"], 2);
    assert_eq!(database["project"]["frames"], 0);
    let digest = database["project"]["digest"].clone();
    let guid = database["source_project_guid"]
        .as_str()
        .unwrap()
        .to_string();

    // Without the preview's digest nothing is deleted.
    let (status, unseen) = call(&a.f.app, "POST", &path, json!({"digests": {}}), None).await;
    assert_eq!(status, StatusCode::OK, "{unseen}");
    assert_eq!(unseen["data"]["state"], "changed");
    // A preview older than the project changes nothing.
    let (status, stale) = call(
        &a.f.app,
        "POST",
        &path,
        json!({"digests": {&guid: "old"}}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stale}");
    assert_eq!(stale["data"]["state"], "changed");
    assert_eq!(stale["data"]["deleted"], json!([]));
    let projects: i64 =
        a.db.query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0))
            .unwrap();
    assert_eq!(projects, 1);

    let (status, deleted) = call(
        &a.f.app,
        "POST",
        &path,
        json!({"digests": {guid_of(preview): digest}}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    assert_eq!(
        deleted["data"],
        json!({"state": "deleted", "name": "Heart Nebula", "databases": ["RedCat rig"]})
    );
    for table in [
        "project",
        "target",
        "exposureplan",
        "psf_guard_director_project",
    ] {
        let left: i64 =
            a.db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(left, 0, "{table} kept rows");
    }
    let (status, _) = call(
        &a.f.app,
        "GET",
        &format!("/projects/{}", a.project),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, plans) = call(&a.f.app, "GET", "/plans", Value::Null, None).await;
    let rows = plans["data"]["rows"].as_array().unwrap();
    assert!(
        rows.iter()
            .all(|row| row["project"]["name"] != "Heart Nebula"),
        "{plans}"
    );
    assert!(
        rows.iter().all(|row| row["links"]
            .as_array()
            .unwrap()
            .iter()
            .all(|link| link["catalog_slug"] != "rig")),
        "{plans}"
    );
}

/// A rig that captured frames keeps the plan; deleting needs database
/// management.
#[tokio::test]
async fn a_plan_with_frames_or_without_management_stays() {
    let a = activated().await;
    activate(&a).await;
    let (project, target): (i64, i64) =
        a.db.query_row("SELECT projectid, Id FROM target LIMIT 1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    a.db.execute(
        "INSERT INTO acquiredimage (projectId, targetId, acquireddate, filtername, gradingStatus, metadata)
         VALUES (?1, ?2, 0, 'Ha', 0, '{}')",
        rusqlite::params![project, target],
    )
    .unwrap();
    let path = format!("/projects/{}/deletion", a.project);
    let (_, preview) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(
        preview["data"]["refusal"],
        "RedCat rig has 1 frame of this plan. Delete that project from the Library first."
    );
    let digest = preview["data"]["databases"][0]["project"]["digest"].clone();
    let (status, refused) = call(
        &a.f.app,
        "POST",
        &path,
        json!({"digests": {guid_of(&preview): digest}}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");

    a.db.execute("DELETE FROM acquiredimage", []).unwrap();
    a.f.state.set_allow_database_management(false);
    let (_, preview) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    let digest = preview["data"]["databases"][0]["project"]["digest"].clone();
    let (status, _) = call(
        &a.f.app,
        "POST",
        &path,
        json!({"digests": {guid_of(&preview): digest}}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let projects: i64 =
        a.db.query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0))
            .unwrap();
    assert_eq!(projects, 1);
}

/// A plan on two rigs whose second project changed after the preview: the
/// first rig's project is deleted, the plan stays, and it no longer lists
/// the first rig's database.
#[tokio::test]
async fn a_change_part_way_keeps_the_plan_without_the_deleted_project() {
    let a = activated().await;
    let (other_rig, _other_db) = add_rig(&a.f, "rig-b").await;
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut draft = store.plan_draft(a.project).unwrap().unwrap();
        let revision = draft.revision;
        let mut second = draft.contributions[0].clone();
        second.id = Uuid::new_v4();
        second.rig_id = other_rig;
        draft.contributions.push(second);
        store.save_plan_draft(&draft, revision).unwrap();
    }
    activate(&a).await;
    let path = format!("/projects/{}/deletion", a.project);
    let (_, preview) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    let databases = preview["data"]["databases"].as_array().unwrap().clone();
    assert_eq!(databases.len(), 2, "{preview}");
    let guid = |index: usize| {
        databases[index]["source_project_guid"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let digests = json!({"digests": {guid(0): databases[0]["project"]["digest"], guid(1): "old"}});
    let (status, outcome) = call(&a.f.app, "POST", &path, digests, None).await;
    assert_eq!(status, StatusCode::OK, "{outcome}");
    assert_eq!(outcome["data"]["state"], "changed", "{outcome}");
    assert_eq!(
        outcome["data"]["deleted"],
        json!([databases[0]["catalog_name"]])
    );
    let left = outcome["data"]["preview"]["databases"].as_array().unwrap();
    assert_eq!(left.len(), 1, "{outcome}");
    assert_eq!(left[0]["catalog_slug"], databases[1]["catalog_slug"]);
    let (status, _) = call(
        &a.f.app,
        "GET",
        &format!("/projects/{}", a.project),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// A database the plan links but this server no longer has is named by
/// its rig, with what to do.
#[tokio::test]
async fn an_unregistered_database_keeps_the_plan() {
    let a = activated().await;
    activate(&a).await;
    a.f.state.databases.write().unwrap().remove("rig");
    let (_, preview) = call(
        &a.f.app,
        "GET",
        &format!("/projects/{}/deletion", a.project),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(
        preview["data"]["refusal"],
        "The database for RedCat rig is not registered on this server. Register it again in Settings, then delete the plan."
    );
}

#[path = "../../../../../crates/director-interop/tests/support/mod.rs"]
mod support;
use psf_guard_director_interop::collaboration::PreparedImport;

/// Import the fixtures' nightly share and activate it on the rig: the rig's
/// database then has a project with six collaboration assignments.
async fn imported(a: &super::activation::Activated) -> PreparedImport {
    use psf_guard_director_interop::{astrocollab::Source, collaboration::prepare_import};
    use psf_guard_director_meta::plan::{Contribution, TemplateChoice};
    let p = {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let source = Source::new("https://collab.example", "000000000001", false).unwrap();
        let p = prepare_import(
            include_bytes!(
                "../../../../../crates/director-interop/tests/fixtures/starfront-tonight.json"
            ),
            &source,
            "2026-10-05",
            "000000000004",
        )
        .unwrap();
        let preview = store.preview_collaboration_import(&p, a.rig).unwrap();
        store
            .apply_collaboration_import(&p, a.rig, &preview.review_digest, 1000)
            .unwrap();
        let mut plan = store.plan_draft(p.project_id()).unwrap().unwrap();
        plan.contributions.push(Contribution {
            id: Uuid::new_v4(),
            objective_id: plan.objectives[0].id,
            rig_id: a.rig,
            template: TemplateChoice {
                template_guid: None,
                template_id: None,
                name: "O 300".into(),
                filter_name: "OIII".into(),
                gain: Some(100),
                offset: Some(30),
                bin: Some(1),
                readout_mode: None,
                moon: None,
            },
            exposure_seconds: 300.0,
            panel_ids: vec![],
            enabled: true,
            goal: None,
        });
        store.save_plan_draft(&plan, plan.revision).unwrap();
        p
    };
    let visit = json!([{"import_id": p.import_id(), "source_digest": p.digest()}]);
    let project = p.project_id();
    let (status, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{project}/activation/preview"),
        json!({"collaboration": visit}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{project}/activation/apply"),
        json!({"preview_digest": preview["data"]["preview_digest"], "collaboration": visit}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(open_assignments(&a.db), 6);
    p
}

fn open_assignments(db: &Connection) -> i64 {
    db.query_row(
        "SELECT COUNT(*) FROM psf_guard_collaboration_plan WHERE end_at_ms IS NULL",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

/// A plan from a collaboration with nothing captured: the preview names the
/// collaboration, and deleting removes the rig's project, the plan and its
/// collaboration records. No connection can sign in, so the coordinator is
/// not told, and the outcome says so.
#[tokio::test]
async fn deleting_an_imported_plan_with_nothing_captured_leaves_its_collaboration() {
    let a = activated().await;
    let p = imported(&a).await;
    let path = format!("/projects/{}/deletion", p.project_id());
    let (status, preview) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let preview = &preview["data"];
    assert_eq!(preview["refusal"], Value::Null, "{preview}");
    let collaboration = &preview["collaboration"];
    assert_eq!(collaboration["host"], "collab.example");
    assert_eq!(collaboration["remote_project_id"], "000000000002");
    assert_eq!(
        collaboration["remote_project_name"],
        "M31 halo in narrowband"
    );
    assert_eq!(collaboration["imports"], 1);
    assert_eq!(collaboration["unsent_reports"], 0);
    assert_eq!(collaboration["open_assignments"], 6);
    assert_eq!(collaboration["coordinator_reachable"], false);
    let digest = preview["databases"][0]["project"]["digest"].clone();
    let (status, deleted) = call(
        &a.f.app,
        "POST",
        &path,
        json!({"digests": {guid_of(preview): digest}}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    let deleted = &deleted["data"];
    assert_eq!(deleted["state"], "deleted", "{deleted}");
    assert_eq!(deleted["coordinator"]["declined"], json!([]));
    assert_eq!(
        deleted["coordinator"]["problems"],
        json!(["No rig on this server is signed in as agent 000000000001 at this collaboration."])
    );
    let left: i64 =
        a.db.query_row(
            "SELECT COUNT(*) FROM psf_guard_collaboration_plan",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(left, 0);
    let service = a.f.state.director.clone().unwrap();
    let (import, plan) = (p.import_id(), p.project_id());
    let (import, plan) = service
        .query(move |s| Ok((s.collaboration_import(import)?, s.project(plan)?)))
        .await
        .unwrap();
    assert!(import.is_none());
    assert!(plan.is_none());
}

/// Frames reported to the collaboration keep the plan, and the refusal
/// points to Leave collaboration. Leaving ends the rig's assignments and
/// keeps the plan and its project; the plan can then be deleted.
#[tokio::test]
async fn a_plan_with_reported_frames_stays_until_it_leaves_the_collaboration() {
    use psf_guard_director_interop::collaboration::finalize_contribution;
    let a = activated().await;
    let p = imported(&a).await;
    let service = a.f.state.director.clone().unwrap();
    let finalized = finalize_contribution(&p, &[support::frame(&p, 1)]).unwrap();
    service
        .clone()
        .run(move |s| s.queue_collaboration_report(&finalized, 2000).map(|_| ()))
        .await
        .unwrap();
    let path = format!("/projects/{}/deletion", p.project_id());
    let (_, preview) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(
        preview["data"]["refusal"],
        "1 frame of this plan was reported to the collaboration at collab.example. Open the plan and choose Leave collaboration under Collaboration assignments, then delete the plan."
    );
    let digest = preview["data"]["databases"][0]["project"]["digest"].clone();
    let (status, _) = call(
        &a.f.app,
        "POST",
        &path,
        json!({"digests": {guid_of(&preview): digest}}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let leave = format!("/projects/{}/collaboration/leave", p.project_id());
    let (status, view) = call(&a.f.app, "GET", &leave, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["data"]["refusal"], Value::Null);
    assert_eq!(view["data"]["collaboration"]["unsent_reports"], 1);
    assert_eq!(view["data"]["collaboration"]["reported_frames"], 1);
    let (status, left) = call(&a.f.app, "POST", &leave, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{left}");
    assert_eq!(left["data"]["state"], "left", "{left}");
    assert_eq!(left["data"]["assignments_ended"], 6);
    assert_eq!(open_assignments(&a.db), 0);
    // The rows stay, ended, as the record of what was assigned.
    let rows: i64 =
        a.db.query_row(
            "SELECT COUNT(*) FROM psf_guard_collaboration_plan",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 6);
    let projects: i64 =
        a.db.query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0))
            .unwrap();
    assert_eq!(projects, 1);
    let plan = p.project_id();
    let (kept, imported, reported) = service
        .clone()
        .query(move |s| {
            Ok((
                s.project(plan)?.is_some(),
                s.plan_from_collaboration(plan)?,
                s.reported_image_guids()?,
            ))
        })
        .await
        .unwrap();
    assert!(kept && !imported && reported.is_empty());
    // A plan of its own has nothing to leave.
    let (_, again) = call(&a.f.app, "GET", &leave, Value::Null, None).await;
    assert_eq!(
        again["data"]["refusal"],
        "This plan did not come from a collaboration."
    );
    let (status, _) = call(&a.f.app, "POST", &leave, Value::Null, None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (_, preview) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(preview["data"]["refusal"], Value::Null, "{preview}");
    assert_eq!(preview["data"]["collaboration"], Value::Null);
    let digest = preview["data"]["databases"][0]["project"]["digest"].clone();
    let (status, deleted) = call(
        &a.f.app,
        "POST",
        &path,
        json!({"digests": {guid_of(&preview): digest}}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    assert_eq!(deleted["data"]["state"], "deleted");
    assert_eq!(deleted["data"].get("coordinator"), None);
}
