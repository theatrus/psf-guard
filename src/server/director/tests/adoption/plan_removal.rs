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
    // The rig's project, without frames, stays deleted: a hidden local
    // tombstone keeps later syncs from bringing it back.
    let (tombstone, origin, allowed_back): (String, String, Option<i64>) =
        a.db.query_row(
            "SELECT lower(project_guid), origin, allowed_back_at FROM psf_guard_removed_project",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (tombstone, origin.as_str(), allowed_back),
        (guid.to_lowercase(), "local", None)
    );
    assert!(crate::commands::project_removal::deletions(&a.db)
        .unwrap()
        .is_empty());
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
    let p = import_for(a, a.rig, "000000000001");
    activate_imports(a, &[&p]).await;
    assert_eq!(open_assignments(&a.db), 6);
    p
}

/// Import the fixtures' share for `rig` as `agent`, with one OIII recipe
/// for the rig. Agents on one server share the plan.
fn import_for(a: &super::activation::Activated, rig: Uuid, agent: &str) -> PreparedImport {
    import_task(a, rig, agent, "000000000004", 1000)
}

/// Import task `task` of the fixtures' share at `now`, and give the rig an
/// OIII recipe when it has none.
fn import_task(
    a: &super::activation::Activated,
    rig: Uuid,
    agent: &str,
    task: &str,
    now: u64,
) -> PreparedImport {
    use psf_guard_director_interop::{astrocollab::Source, collaboration::prepare_import};
    use psf_guard_director_meta::plan::{Contribution, TemplateChoice};
    let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
    let source = Source::new("https://collab.example", agent, false).unwrap();
    let mut wire: Value = serde_json::from_slice(include_bytes!(
        "../../../../../crates/director-interop/tests/fixtures/starfront-tonight.json"
    ))
    .unwrap();
    wire.as_object_mut().unwrap().remove("task");
    wire["tasks"][0]["agent"] = json!(agent);
    wire["tasks"][0]["id"] = json!(task);
    let p = prepare_import(
        &serde_json::to_vec(&wire).unwrap(),
        &source,
        "2026-10-05",
        task,
    )
    .unwrap();
    let preview = store.preview_collaboration_import(&p, rig).unwrap();
    store
        .apply_collaboration_import(&p, rig, &preview.review_digest, now)
        .unwrap();
    let mut plan = store.plan_draft(p.project_id()).unwrap().unwrap();
    if plan.contributions.iter().any(|c| c.rig_id == rig) {
        return p;
    }
    plan.contributions.push(Contribution {
        id: Uuid::new_v4(),
        objective_id: plan.objectives[0].id,
        rig_id: rig,
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
}

/// Activate the plan of `imports` with each import as its rig's assignment.
async fn activate_imports(a: &super::activation::Activated, imports: &[&PreparedImport]) {
    let visits: Vec<Value> = imports
        .iter()
        .map(|p| json!({"import_id": p.import_id(), "source_digest": p.digest()}))
        .collect();
    let project = imports[0].project_id();
    let (status, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{project}/activation/preview"),
        json!({"collaboration": visits}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{project}/activation/apply"),
        json!({"preview_digest": preview["data"]["preview_digest"], "collaboration": visits}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
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
        json!(["RedCat rig is not signed in to this collaboration."])
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
        "1 frame of this plan is in reports to the collaboration at collab.example. Open the plan and choose Leave collaboration under Collaboration assignments, then delete its projects from the Library."
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

/// An imported plan on two rigs whose second project changed after the
/// preview: the first rig's project is deleted, and the answer is `changed`
/// with a preview that no longer lists the first database, as for any plan.
/// The plan stays in its collaboration.
#[tokio::test]
async fn a_change_part_way_keeps_an_imported_plan_without_the_deleted_project() {
    let a = activated().await;
    let (other_rig, _other_db) = add_rig(&a.f, "rig-b").await;
    let first = import_for(&a, a.rig, "000000000001");
    let second = import_for(&a, other_rig, "000000000009");
    activate_imports(&a, &[&first, &second]).await;
    let path = format!("/projects/{}/deletion", first.project_id());
    let (_, preview) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(preview["data"]["refusal"], Value::Null, "{preview}");
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
    assert_eq!(
        outcome["data"]["preview"]["collaboration"]["imports"], 2,
        "{outcome}"
    );
}

/// Leaving does not wait for a database that is not registered: the
/// assignments elsewhere end, that database's stay open and the result
/// says so. Registering it again and activating the plan ends them.
#[tokio::test]
async fn leaving_with_an_unregistered_database_warns_and_activation_ends_its_assignments() {
    let a = activated().await;
    let p = imported(&a).await;
    let context = a.f.state.databases.write().unwrap().remove("rig").unwrap();
    let leave = format!("/projects/{}/collaboration/leave", p.project_id());
    let warning = "The database for RedCat rig is not registered on this server, so the plan's assignments there stay open. PSF Guard no longer counts frames toward them. To end them, register the database again in Settings if it is missing, then activate the plan.";
    let (status, preview) = call(&a.f.app, "GET", &leave, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["data"]["refusal"], Value::Null);
    assert_eq!(preview["data"]["warnings"], json!([warning]));
    let (status, left) = call(&a.f.app, "POST", &leave, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{left}");
    assert_eq!(left["data"]["state"], "left");
    assert_eq!(left["data"]["assignments_ended"], 0);
    assert_eq!(left["data"]["warnings"], json!([warning]));
    assert_eq!(open_assignments(&a.db), 6);

    // Plan deletion still needs the database.
    let (_, deletion) = call(
        &a.f.app,
        "GET",
        &format!("/projects/{}/deletion", p.project_id()),
        Value::Null,
        None,
    )
    .await;
    assert!(deletion["data"]["refusal"]
        .as_str()
        .unwrap()
        .starts_with("The database for RedCat rig is not registered on this server."));

    a.f.state
        .databases
        .write()
        .unwrap()
        .insert("rig".into(), context);
    let project = p.project_id();
    let (status, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{project}/activation/preview"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{project}/activation/apply"),
        json!({"preview_digest": preview["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(open_assignments(&a.db), 0);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn ended_assignments(db: &Connection) -> i64 {
    db.query_row(
        "SELECT COUNT(*) FROM psf_guard_collaboration_plan WHERE end_at_ms IS NOT NULL",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

/// Leaving commits in the store before it touches the rig databases: when
/// the store is busy, the plan stays in its collaboration with every
/// assignment open, rather than half left.
#[tokio::test]
async fn a_busy_store_leaves_nothing_half_done() {
    let a = activated().await;
    let p = imported(&a).await;
    let leave = format!("/projects/{}/collaboration/leave", p.project_id());
    let service = a.f.state.director.clone().unwrap();
    let writes = service.admission.clone().acquire_owned().await.unwrap();
    let (status, busy) = call(&a.f.app, "POST", &leave, Value::Null, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{busy}");
    drop(writes);
    assert_eq!(open_assignments(&a.db), 6);
    let plan = p.project_id();
    assert!(service
        .clone()
        .query(move |s| s.plan_from_collaboration(plan))
        .await
        .unwrap());
    let (status, left) = call(&a.f.app, "POST", &leave, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{left}");
    assert_eq!(open_assignments(&a.db), 0);
}

/// A plan that left and imported the same task again activates: its ended
/// assignments reopen from the activation, and the frames goal stays.
#[tokio::test]
async fn a_rejoined_plan_activates_its_assignments_again() {
    let a = activated().await;
    let p = imported(&a).await;
    let leave = format!("/projects/{}/collaboration/leave", p.project_id());
    let (status, left) = call(&a.f.app, "POST", &leave, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{left}");
    assert_eq!(open_assignments(&a.db), 0);
    let ended_at: i64 =
        a.db.query_row(
            "SELECT MAX(end_at_ms) FROM psf_guard_collaboration_plan",
            [],
            |row| row.get(0),
        )
        .unwrap();
    // A person rejoins seconds later at the soonest; reopening needs the
    // import a full second after the rows ended.
    let again = import_task(&a, a.rig, "000000000001", "000000000004", now_ms() + 2_000);
    assert_eq!(again.import_id(), p.import_id());
    activate_imports(&a, &[&again]).await;
    assert_eq!(open_assignments(&a.db), 6);
    let started: i64 =
        a.db.query_row(
            "SELECT MIN(start_at_ms) FROM psf_guard_collaboration_plan",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(started >= ended_at, "{started} < {ended_at}");
}

/// A revision a newer assignment replaced keeps its old import time, so its
/// ended rows stay ended and it is not activated again.
#[tokio::test]
async fn a_superseded_assignment_stays_ended() {
    let a = activated().await;
    let first = imported(&a).await;
    let second = import_task(&a, a.rig, "000000000001", "000000000005", now_ms());
    assert_eq!(second.project_id(), first.project_id());
    activate_imports(&a, &[&second]).await;
    let first_open: i64 = a
        .db
        .query_row(
            "SELECT COUNT(*) FROM psf_guard_collaboration_plan WHERE import_id=?1 AND end_at_ms IS NULL",
            [first.import_id().to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(first_open, 0);
    let ended = ended_assignments(&a.db);
    assert!(ended > 0);
    let visits = json!([{"import_id": first.import_id(), "source_digest": first.digest()}]);
    let project = first.project_id();
    let (_, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{project}/activation/preview"),
        json!({"collaboration": visits}),
        None,
    )
    .await;
    assert!(
        preview
            .to_string()
            .contains("This assignment was superseded."),
        "{preview}"
    );
    assert_eq!(ended_assignments(&a.db), ended);
}

/// Activating a plan of its own on the rig ends only its own leftover
/// assignments: a plan still in its collaboration keeps its rows open.
#[tokio::test]
async fn an_ordinary_activation_leaves_an_imported_plans_assignments_open() {
    let a = activated().await;
    imported(&a).await;
    activate(&a).await;
    assert_eq!(open_assignments(&a.db), 6);
}

/// Deleting a plan of its own does not wait for the collaboration gate; a
/// plan from a collaboration does.
#[tokio::test]
async fn only_an_imported_plan_waits_for_collaboration_work() {
    let a = activated().await;
    activate(&a).await;
    let p = imported(&a).await;
    let service = a.f.state.director.clone().unwrap();
    let gate = crate::server::director::collaboration_auth::leave::hold(&service)
        .await
        .unwrap();
    let path = format!("/projects/{}/deletion", p.project_id());
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
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let path = format!("/projects/{}/deletion", a.project);
    let (_, preview) = call(&a.f.app, "GET", &path, Value::Null, None).await;
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
    assert_eq!(deleted["data"]["state"], "deleted", "{deleted}");
    drop(gate);
}
