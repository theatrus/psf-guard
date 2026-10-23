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
