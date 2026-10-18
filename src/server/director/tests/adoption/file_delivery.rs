use super::{
    activation::{activated, Activated},
    *,
};
use crate::{
    db_registry::DbRegistry,
    server::{database_context::DatabaseContext, sync_preview::SyncPreviewManager},
    ts_sync::Settings,
};

fn connect(a: &Activated) -> Connection {
    let source = a.f._dir.path().join("TargetScheduler.sqlite");
    a.db.backup("main", &source, None).unwrap();
    let context = a.f.state.get_database("rig").unwrap();
    let mut registry = DbRegistry::default();
    registry
        .add(
            context.name.clone(),
            context.database_path.clone(),
            context.image_dirs.clone(),
            Some("rig".into()),
        )
        .unwrap();
    let entry = registry
        .databases
        .iter_mut()
        .find(|e| e.id == "rig")
        .unwrap();
    entry.ts_sync = Some(Settings {
        source_path: source.to_string_lossy().into_owned(),
        writeback: false,
        ..Settings::default()
    });
    let connected = DatabaseContext::from_entry(entry, a.f.state.storage_roots.clone()).unwrap();
    let path = a.f._dir.path().join("registry.json");
    registry.save(&path).unwrap();
    a.f.state.set_registry_path(Some(path));
    a.f.state
        .databases
        .write()
        .unwrap()
        .insert("rig".into(), Arc::new(connected));
    Connection::open(source).unwrap()
}

async fn preview(a: &Activated) -> Value {
    let (status, response) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    response["data"].clone()
}

async fn apply(a: &Activated, preview: &Value) -> (StatusCode, Value) {
    call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest":preview["preview_digest"]}),
        None,
    )
    .await
}

async fn last(a: &Activated) -> Value {
    let (status, response) = call(
        &a.f.app,
        "GET",
        &format!("/projects/{}/activation", a.project),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    response["data"]["activation"].clone()
}

fn number(db: &Connection, query: &str) -> i64 {
    db.query_row(query, [], |r| r.get(0)).unwrap()
}

#[tokio::test]
async fn activation_delivers_through_the_saved_ts_connection_with_automatic_grades_off() {
    let a = activated().await;
    let ts = connect(&a);
    let p = preview(&a).await;
    assert_eq!(p["rigs"][0]["push"]["peer_id"], "ts:rig");
    assert!(p["rigs"][0]["push"]["ts_file"]["path"]
        .as_str()
        .unwrap()
        .ends_with("TargetScheduler.sqlite"));
    assert_eq!(p["rigs"][0]["push"]["summary"]["project_inserted"], 1);
    assert_eq!(number(&ts, "SELECT count(*) FROM project"), 0);
    assert!(
        a.f.state.sync_previews.list("rig").unwrap().is_empty(),
        "previews do not stage transfers"
    );
    let (status, applied) = apply(&a, &p).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(
        applied["data"]["rigs"][0]["push"]["applied"], true,
        "{applied}"
    );
    assert_eq!(number(&ts, "SELECT count(*) FROM project"), 1);
    assert_eq!(number(&ts, "SELECT count(*) FROM target"), 2);
    assert_eq!(number(&ts, "SELECT count(*) FROM exposureplan"), 2);
    let guid: String =
        a.db.query_row("SELECT guid FROM project", [], |r| r.get(0))
            .unwrap();
    assert_eq!(
        ts.query_row("SELECT guid FROM project", [], |r| r.get::<_, String>(0))
            .unwrap(),
        guid
    );
    assert_eq!(
        number(
            &ts,
            "SELECT count(*) FROM sqlite_master WHERE name='psf_guard_director_project'"
        ),
        0
    );
    assert_eq!(last(&a).await["pushes"][0]["applied"], true);
    assert!(a.f.state.sync_previews.list("rig").unwrap().is_empty());

    // A planning delivery never copies the owned catalog's progress counts.
    ts.execute_batch("UPDATE exposureplan SET acquired=7,accepted=6")
        .unwrap();
    let p = preview(&a).await;
    let (status, applied) = apply(&a, &p).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["data"]["rigs"][0]["push"]["applied"], true);
    assert_eq!(number(&ts, "SELECT min(acquired) FROM exposureplan"), 7);
    assert_eq!(number(&ts, "SELECT min(accepted) FROM exposureplan"), 6);
}

#[tokio::test]
async fn ts_planning_changed_after_preview_refuses_activation_without_local_writes() {
    let a = activated().await;
    let ts = connect(&a);
    let p = preview(&a).await;
    ts.execute("UPDATE exposuretemplate SET gain=123", [])
        .unwrap();
    let (status, response) = apply(&a, &p).await;
    assert_eq!(status, StatusCode::CONFLICT, "{response}");
    assert_eq!(number(&a.db, "SELECT count(*) FROM project"), 0);
    assert_eq!(number(&ts, "SELECT gain FROM exposuretemplate"), 123);
    assert!(a.f.state.sync_previews.list("rig").unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_ts_delivery_keeps_a_durable_frozen_preview_for_retry() {
    let a = activated().await;
    let ts = connect(&a);
    ts.execute_batch("CREATE TRIGGER refuse_planning BEFORE INSERT ON project BEGIN SELECT RAISE(ABORT,'TS unavailable for planning'); END;").unwrap();
    let p = preview(&a).await;
    let (status, response) = apply(&a, &p).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["data"]["rigs"][0]["applied"], true);
    assert_eq!(response["data"]["rigs"][0]["push"]["applied"], false);
    let receipt = last(&a).await;
    let id = receipt["pushes"][0]["ts_sync"]["preview_id"]
        .as_str()
        .unwrap();
    assert_eq!(receipt["pushes"][0]["applied"], false);
    let restarted = SyncPreviewManager::new(a.f._dir.path().join("cache"));
    assert!(
        restarted.get(id).unwrap().is_some(),
        "approval survives a server restart"
    );
    a.db.execute("UPDATE project SET name='Unreviewed edit'", [])
        .unwrap();
    ts.execute_batch("DROP TRIGGER refuse_planning").unwrap();
    crate::server::director::retry_ts_planning(&a.f.state, "rig")
        .await
        .unwrap();
    assert_eq!(last(&a).await["pushes"][0]["applied"], true);
    assert_eq!(
        ts.query_row("SELECT name FROM project", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "Heart Nebula"
    );
    assert_eq!(
        receipt["pushes"][0]["ts_sync"]["source_path"],
        p["rigs"][0]["push"]["ts_file"]["path"]
    );
}

#[tokio::test]
async fn retry_does_not_overwrite_ts_changes_since_the_approved_activation() {
    let a = activated().await;
    let ts = connect(&a);
    ts.execute_batch("CREATE TRIGGER refuse_planning BEFORE INSERT ON project BEGIN SELECT RAISE(ABORT,'TS unavailable'); END;").unwrap();
    let p = preview(&a).await;
    let (status, response) = apply(&a, &p).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    ts.execute_batch("DROP TRIGGER refuse_planning; UPDATE exposuretemplate SET gain=321;")
        .unwrap();
    let (status, response) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/push", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["data"]["rigs"][0]["push"]["applied"], false);
    assert!(response["data"]["rigs"][0]["push"]["error"]
        .as_str()
        .unwrap()
        .contains("stale"));
    assert_eq!(number(&ts, "SELECT gain FROM exposuretemplate"), 321);
    assert_eq!(number(&ts, "SELECT count(*) FROM project"), 0);
}

#[tokio::test]
async fn an_offline_ts_file_requires_a_new_preview_before_later_delivery() {
    let a = activated().await;
    let ts = connect(&a);
    let path = a.f._dir.path().join("TargetScheduler.sqlite");
    drop(ts);
    let away = path.with_extension("away");
    std::fs::rename(&path, &away).unwrap();
    let p = preview(&a).await;
    assert!(p["rigs"][0]["push"]["error"].is_string());
    let (status, response) = apply(&a, &p).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["data"]["rigs"][0]["applied"], true);
    assert_eq!(last(&a).await["pushes"][0]["applied"], false);
    std::fs::rename(&away, &path).unwrap();
    let (status, response) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/push", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["data"]["rigs"][0]["push"]["applied"], false);
    let ts = Connection::open(path).unwrap();
    assert_eq!(number(&ts, "SELECT count(*) FROM project"), 0);
    let (status, response) = apply(&a, &preview(&a).await).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(
        response["data"]["rigs"][0]["push"]["applied"], true,
        "{response}"
    );
}

#[tokio::test]
async fn new_capture_progress_does_not_stale_a_planning_preview_or_change_ts_grades() {
    let a = activated().await;
    let ts = connect(&a);
    let (status, response) = apply(&a, &preview(&a).await).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let p = preview(&a).await;
    ts.execute_batch("UPDATE exposureplan SET acquired=7,accepted=6;
        INSERT INTO acquiredimage(projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,guid,rejectreason)
        SELECT projectid,Id,1000,'Ha',2,'{}','profile-a','capture-guid','TS review' FROM target LIMIT 1;").unwrap();
    let (status, response) = apply(&a, &p).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["data"]["rigs"][0]["push"]["applied"], true);
    assert_eq!(number(&ts, "SELECT min(acquired) FROM exposureplan"), 7);
    assert_eq!(number(&ts, "SELECT gradingStatus FROM acquiredimage"), 2);
    assert_eq!(
        ts.query_row("SELECT rejectreason FROM acquiredimage", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "TS review"
    );
}

#[tokio::test]
async fn taking_ts_values_reads_the_connected_file_instead_of_the_owned_catalog() {
    let a = activated().await;
    let ts = connect(&a);
    let (status, response) = apply(&a, &preview(&a).await).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    ts.execute("UPDATE exposureplan SET desired=90", [])
        .unwrap();
    assert_eq!(number(&a.db, "SELECT min(desired) FROM exposureplan"), 72);
    let (status, response) = super::activation::take_values(&a.f, a.project, a.rig).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert!(
        !response["data"]["taken"].as_array().unwrap().is_empty(),
        "{response}"
    );
    let store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
    let plan = store.plan_draft(a.project).unwrap().unwrap();
    assert!(
        matches!(
            plan.objectives[0].goal,
            psf_guard_director_meta::plan::Goal::Frames { value: 90 }
        ),
        "{plan:?}"
    );
    assert_eq!(number(&a.db, "SELECT min(desired) FROM exposureplan"), 72);
}

#[tokio::test]
async fn turning_a_rig_off_delivers_inactive_state_to_the_connected_ts_file() {
    let a = activated().await;
    let ts = connect(&a);
    let (other_rig, _other_db) = super::activation::add_rig(&a.f, "second-rig").await;
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut plan = store.plan_draft(a.project).unwrap().unwrap();
        let mut contribution = plan.contributions[0].clone();
        contribution.id = Uuid::new_v4();
        contribution.rig_id = other_rig;
        plan.contributions.push(contribution);
        store.save_plan_draft(&plan, plan.revision).unwrap();
    }
    let (status, response) = apply(&a, &preview(&a).await).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut plan = store.plan_draft(a.project).unwrap().unwrap();
        for contribution in &mut plan.contributions {
            contribution.enabled = contribution.rig_id != a.rig;
        }
        store.save_plan_draft(&plan, plan.revision).unwrap();
    }
    let (status, response) = apply(&a, &preview(&a).await).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(
        response["data"]["rigs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["catalog_slug"] == "rig")
            .unwrap()["push"]["applied"],
        true,
        "{response}"
    );
    assert_eq!(number(&ts, "SELECT state FROM project"), 2);
}

#[tokio::test]
async fn retry_recovers_a_committed_ts_transfer_whose_receipt_was_interrupted() {
    let a = activated().await;
    let ts = connect(&a);
    ts.execute_batch("CREATE TRIGGER refuse_planning BEFORE INSERT ON project BEGIN SELECT RAISE(ABORT,'TS unavailable'); END;").unwrap();
    let (status, response) = apply(&a, &preview(&a).await).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let receipt = last(&a).await;
    let id = receipt["pushes"][0]["ts_sync"]["preview_id"]
        .as_str()
        .unwrap();
    let record = a.f.state.sync_previews.get(id).unwrap().unwrap();
    let source =
        a.f.state
            .sync_previews
            .source_snapshot_path(&record)
            .unwrap();
    ts.execute_batch("DROP TRIGGER refuse_planning").unwrap();
    let mut request = record.request.clone();
    request.dry_run = false;
    crate::server::handlers::execute_scheduler_sync_paths(
        &a.f.state,
        source,
        a.f._dir.path().join("TargetScheduler.sqlite"),
        "rig".into(),
        "ts:rig".into(),
        request,
        crate::server::handlers::SyncGuardMode::Apply {
            destination_fingerprint: record.destination_fingerprint,
        },
    )
    .await
    .unwrap();
    // TS committed, but the activation still reads as pending after a crash.
    assert_eq!(last(&a).await["pushes"][0]["applied"], false);
    crate::server::director::retry_ts_planning(&a.f.state, "rig")
        .await
        .unwrap();
    assert_eq!(last(&a).await["pushes"][0]["applied"], true);
    assert_eq!(number(&ts, "SELECT count(*) FROM project"), 1);
    assert!(a.f.state.sync_previews.get(id).unwrap().is_none());
}

#[tokio::test]
async fn activation_does_not_export_stale_planning_from_other_projects() {
    let a = activated().await;
    let ts = connect(&a);
    for db in [&a.db, &ts] {
        db.execute_batch("INSERT INTO project(Id,profileId,name,state,priority,guid) VALUES(42,'profile-a','Other project',2,1,'other-project');").unwrap();
    }
    ts.execute("UPDATE project SET priority=99 WHERE Id=42", [])
        .unwrap();
    let p = preview(&a).await;
    assert_eq!(p["rigs"][0]["push"]["summary"]["project_updated"], 0);
    let (status, response) = apply(&a, &p).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(
        response["data"]["rigs"][0]["push"]["applied"], true,
        "{response}"
    );
    assert_eq!(
        number(
            &ts,
            "SELECT priority FROM project WHERE guid='other-project'"
        ),
        99
    );
    assert_eq!(
        number(
            &a.db,
            "SELECT priority FROM project WHERE guid='other-project'"
        ),
        1
    );
}

#[tokio::test]
async fn activation_keeps_ts_settings_that_director_does_not_own() {
    let a = activated().await;
    let ts = connect(&a);
    let (status, response) = apply(&a, &preview(&a).await).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    ts.execute_batch(
        "UPDATE project SET state=2,priority=99,description='Edited in TS';
        UPDATE target SET roi=75;
        UPDATE exposuretemplate SET gain=250,moonavoidanceenabled=1;",
    )
    .unwrap();
    let p = preview(&a).await;
    assert_eq!(p["rigs"][0]["shoots"][0]["gain"], 250);
    let (status, response) = apply(&a, &p).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(
        response["data"]["rigs"][0]["push"]["applied"], true,
        "{response}"
    );
    assert_eq!(number(&ts, "SELECT state FROM project"), 2);
    assert_eq!(number(&ts, "SELECT priority FROM project"), 99);
    assert_eq!(
        ts.query_row("SELECT min(roi) FROM target", [], |r| r.get::<_, f64>(0))
            .unwrap(),
        75.0
    );
    assert_eq!(number(&ts, "SELECT gain FROM exposuretemplate"), 250);
    assert_eq!(
        number(&ts, "SELECT moonavoidanceenabled FROM exposuretemplate"),
        1
    );
    assert_eq!(
        ts.query_row("SELECT description FROM project", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "Edited in TS"
    );
}

#[tokio::test]
async fn an_expired_delivery_does_not_block_other_approved_projects() {
    let mut a = activated().await;
    let ts = connect(&a);
    let first = a.project;
    let second = {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let second = store
            .create_project(Uuid::new_v4(), "Second project")
            .unwrap()
            .id;
        let mut framing = store.framing_draft(first).unwrap().unwrap();
        framing.project_id = second;
        framing.revision = 0;
        store.save_framing_draft(&framing, 0).unwrap();
        let mut plan = store.plan_draft(first).unwrap().unwrap();
        plan.project_id = second;
        plan.revision = 0;
        store.save_plan_draft(&plan, 0).unwrap();
        second
    };
    ts.execute_batch("CREATE TRIGGER refuse_planning BEFORE INSERT ON project BEGIN SELECT RAISE(ABORT,'TS unavailable'); END;").unwrap();
    for project in [first, second] {
        a.project = project;
        let (status, response) = apply(&a, &preview(&a).await).await;
        assert_eq!(status, StatusCode::OK, "{response}");
        assert_eq!(response["data"]["rigs"][0]["push"]["applied"], false);
    }
    let pending =
        a.f.state
            .director
            .as_ref()
            .unwrap()
            .writer
            .lock()
            .unwrap()
            .pending_pushes("ts:rig")
            .unwrap();
    assert_eq!(pending.len(), 2);
    let stale = &pending[0];
    let id = stale.pushes[0]
        .ts_sync
        .as_ref()
        .unwrap()
        .preview_id
        .as_ref()
        .unwrap();
    a.f.state.sync_previews.discard(id, "rig").unwrap();
    ts.execute_batch("DROP TRIGGER refuse_planning").unwrap();
    assert!(
        crate::server::director::retry_ts_planning(&a.f.state, "rig")
            .await
            .is_err()
    );
    a.project = stale.project_id;
    assert_eq!(last(&a).await["pushes"][0]["applied"], false);
    a.project = pending[1].project_id;
    assert_eq!(last(&a).await["pushes"][0]["applied"], true);
    assert_eq!(number(&ts, "SELECT count(*) FROM project"), 1);
}
