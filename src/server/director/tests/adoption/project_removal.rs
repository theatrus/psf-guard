use super::activation::activated;
use super::*;

/// Call the catalog API, as the Library does.
async fn catalog_call(
    state: &Arc<AppState>,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let app = Router::new().nest("/api", crate::server::api_router(Arc::clone(state)));
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(format!("/api/db/rig{path}"))
                .header("content-type", "application/json")
                .body(Body::from(if body.is_null() {
                    String::new()
                } else {
                    body.to_string()
                }))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Deleting an activated project from its rig's catalog: the rows go, the
/// plan forgets the rig, and the next plan listing neither keeps the link
/// nor adopts the project again.
#[tokio::test]
async fn deleting_an_activated_project_takes_it_out_of_its_plan() {
    let a = activated().await;
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
    let id: i64 =
        a.db.query_row("SELECT Id FROM project", [], |row| row.get(0))
            .unwrap();
    let path = format!("/projects/{id}/removal");
    // One frame, so the deletion is listed and keeps the project out.
    a.db.execute(
        "INSERT INTO acquiredimage (projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid)
         SELECT projectid, Id, 0, 'Ha', 0, '{\"FileName\":\"heart_001.fits\"}', 'heart-frame' FROM target LIMIT 1",
        [],
    )
    .unwrap();

    let (status, review) = catalog_call(&a.f.state, "GET", &path, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{review}");
    let review = &review["data"];
    assert_eq!(review["name"], "Heart Nebula");
    assert_eq!(review["targets"], 2);
    assert_eq!(review["frames"]["total"], 1);
    assert_eq!(
        review["linked_plan"],
        json!({"name": "Heart Nebula", "databases": 1, "from_collaboration": false})
    );
    assert_eq!(review["allowed"], true);

    // A preview older than the project changes nothing.
    let (status, stale) = catalog_call(&a.f.state, "POST", &path, json!({"digest": "old"})).await;
    assert_eq!(status, StatusCode::OK, "{stale}");
    assert_eq!(stale["data"]["state"], "changed");
    assert_eq!(stale["data"]["preview"]["digest"], review["digest"]);

    let (status, removed) = catalog_call(
        &a.f.state,
        "POST",
        &path,
        json!({"digest": review["digest"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(
        removed["data"],
        json!({"state": "removed", "name": "Heart Nebula", "targets": 2, "exposure_plans": 2, "frames": 1,
               "plan": "Heart Nebula", "plan_error": null})
    );
    for table in [
        "project",
        "target",
        "exposureplan",
        "psf_guard_director_project",
        "psf_guard_director_target",
        "psf_guard_director_plan",
    ] {
        let left: i64 =
            a.db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(left, 0, "{table} kept rows");
    }

    // The plan keeps its framing but no longer links the rig or plans work there.
    let (_, plans) = call(&a.f.app, "GET", "/plans", Value::Null, None).await;
    let rows = plans["data"]["rows"].as_array().unwrap();
    let heart = rows
        .iter()
        .find(|row| row["project"]["id"] == a.project.to_string())
        .unwrap();
    assert_eq!(heart["links"], json!([]));
    assert_eq!(
        heart["activation"]["rigs"], 0,
        "the activation record forgot the rig"
    );
    assert!(
        rows.iter().all(|row| row["links"]
            .as_array()
            .unwrap()
            .iter()
            .all(|link| link["catalog_slug"] != "rig")),
        "no plan links the rig's catalog: {plans}"
    );
    let (_, plan) = call(
        &a.f.app,
        "GET",
        &format!("/projects/{}/plan", a.project),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(plan["data"]["plan"]["contributions"], json!([]), "{plan}");
    assert_eq!(heart["framing"]["target_name"], "IC 1805");

    // The deletion is listed until the project is allowed back.
    let (_, listed) = catalog_call(&a.f.state, "GET", "/deleted-projects", Value::Null).await;
    let listed = listed["data"].as_array().unwrap().clone();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["name"], "Heart Nebula");
    let (status, allowed) = catalog_call(
        &a.f.state,
        "POST",
        &format!("/deleted-projects/{}/allow", listed[0]["id"]),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{allowed}");
    let (_, listed) = catalog_call(&a.f.state, "GET", "/deleted-projects", Value::Null).await;
    assert_eq!(listed["data"], json!([]));
}

#[tokio::test]
async fn deleting_needs_database_management() {
    let a = activated().await;
    a.db.execute(
        "INSERT INTO project (Id, profileId, name, guid) VALUES (7, 'profile-a', 'Spare', 'spare')",
        [],
    )
    .unwrap();
    a.f.state.set_allow_database_management(false);
    let (status, review) =
        catalog_call(&a.f.state, "GET", "/projects/7/removal", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{review}");
    assert_eq!(review["data"]["allowed"], false);
    let (status, refused) = catalog_call(
        &a.f.state,
        "POST",
        "/projects/7/removal",
        json!({"digest": review["data"]["digest"]}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let left: i64 =
        a.db.query_row("SELECT COUNT(*) FROM project WHERE Id = 7", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(left, 1);
    let (status, _) = catalog_call(&a.f.state, "GET", "/projects/99/removal", Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) =
        catalog_call(&a.f.state, "POST", "/deleted-projects/1/allow", Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// A pull from a catalog that deleted the activated project deletes the
/// rig catalog's copy, which has no frames, and the plan forgets the rig as
/// it does after Delete project.
#[tokio::test]
async fn a_sync_that_deletes_an_activated_project_takes_it_out_of_its_plan() {
    let a = activated().await;
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
    let guid: String =
        a.db.query_row("SELECT guid FROM project", [], |row| row.get(0))
            .unwrap();
    let linked = |plans: &Value| {
        plans["data"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["project"]["id"] == a.project.to_string())
            .unwrap()["links"]
            .clone()
    };
    let (_, plans) = call(&a.f.app, "GET", "/plans", Value::Null, None).await;
    assert_ne!(linked(&plans), json!([]), "{plans}");

    // Another catalog that deleted the project.
    let source_path = a.f._dir.path().join("deleted-there.sqlite");
    let source = crate::ts_schema::create_fresh_db(&source_path).unwrap();
    crate::commands::project_removal::ensure_schema(&source).unwrap();
    source
        .execute(
            "INSERT INTO psf_guard_removed_project (project_id, project_guid, name, removed_at, frames)
             VALUES (1, ?1, 'Heart Nebula', 1, 0)",
            [&guid],
        )
        .unwrap();
    drop(source);
    let rig_path: String =
        a.db.query_row("PRAGMA database_list", [], |row| row.get(2))
            .unwrap();
    let (result, _) = crate::server::handlers::execute_scheduler_sync_paths(
        &a.f.state,
        source_path,
        rig_path.into(),
        "deleted-there".into(),
        "rig".into(),
        crate::server::api::SchedulerSyncRequest {
            peer_db_id: "deleted-there".into(),
            kind: crate::server::api::SchedulerSyncKind::Pull,
            dry_run: false,
            with_image_data: Some(false),
            project: None,
            target: None,
            status: None,
            reviewed_only: true,
        },
        crate::server::handlers::SyncGuardMode::None,
    )
    .await
    .unwrap();
    assert_eq!(result.project.deleted, 1, "{:?}", result.changes);
    let left: i64 =
        a.db.query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0))
            .unwrap();
    assert_eq!(left, 0);
    let (_, plans) = call(&a.f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(linked(&plans), json!([]), "{plans}");
}

/// A planning push into a Target Scheduler file (a `ts:` connection) deletes
/// a project the source catalog deleted and writes no PSF Guard table
/// there.
#[tokio::test]
async fn a_planning_push_into_a_ts_file_deletes_without_a_tombstone() {
    let a = activated().await;
    let source_path = a.f._dir.path().join("planning-source.sqlite");
    let source = crate::ts_schema::create_fresh_db(&source_path).unwrap();
    crate::commands::project_removal::ensure_schema(&source).unwrap();
    source
        .execute(
            "INSERT INTO psf_guard_removed_project (project_id, project_guid, name, removed_at, frames)
             VALUES (1, 'gone-guid', 'Gone', 1, 0)",
            [],
        )
        .unwrap();
    drop(source);
    let ts_path = a.f._dir.path().join("TargetScheduler-copy.sqlite");
    let ts = crate::ts_schema::create_fresh_db(&ts_path).unwrap();
    ts.execute(
        "INSERT INTO project (profileId, name, guid) VALUES ('profile-a', 'Gone', 'gone-guid')",
        [],
    )
    .unwrap();
    let (result, _) = crate::server::handlers::execute_scheduler_sync_paths(
        &a.f.state,
        source_path,
        ts_path,
        "rig".into(),
        "ts:rig".into(),
        crate::server::api::SchedulerSyncRequest {
            peer_db_id: "ts:rig".into(),
            kind: crate::server::api::SchedulerSyncKind::PushPlanning,
            dry_run: false,
            with_image_data: None,
            project: None,
            target: None,
            status: None,
            reviewed_only: true,
        },
        crate::server::handlers::SyncGuardMode::None,
    )
    .await
    .unwrap();
    assert_eq!(result.project.deleted, 1, "{:?}", result.changes);
    let (projects, tombstone_tables): (i64, i64) = ts
        .query_row(
            "SELECT (SELECT COUNT(*) FROM project),
                    (SELECT COUNT(*) FROM sqlite_master WHERE name = 'psf_guard_removed_project')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((projects, tombstone_tables), (0, 0));
}
