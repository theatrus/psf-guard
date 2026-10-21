//! MCP tools driven over HTTP through the real API router, as an agent
//! drives them.

use super::*;
use axum::{body::Body, http::Request, Router};
use tower::ServiceExt;

fn planning_state(dir: &tempfile::TempDir, management: bool) -> Arc<AppState> {
    let mut state = AppState::from_databases(
        vec![],
        dir.path().join("cache"),
        crate::cli::PregenerationConfig::default(),
    )
    .unwrap();
    state.set_allow_database_management(true);
    state.director =
        crate::server::director::Service::configured(Some(&dir.path().join("meta.sqlite")))
            .unwrap();
    state.set_allow_database_management(management);
    Arc::new(state)
}

fn app(state: &Arc<AppState>) -> Router {
    Router::new().nest("/api", crate::server::api_router(Arc::clone(state)))
}

/// Call one tool; the result's JSON text parsed, or `Err` with the tool
/// error's text.
async fn tool(app: &Router, name: &str, arguments: Value) -> Result<Value, String> {
    tool_as(app, None, name, arguments).await
}

/// Call one tool with an API token, as an agent connected with one does.
async fn tool_as(
    app: &Router,
    token: Option<&str>,
    name: &str,
    arguments: Value,
) -> Result<Value, String> {
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let mut request = Request::post("/api/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = request.body(Body::from(body.to_string())).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let reply: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&bytes)));
    let result = &reply["result"];
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    if result["isError"] == true {
        Err(text.to_string())
    } else {
        Ok(serde_json::from_str(text).unwrap_or(Value::String(text.to_string())))
    }
}

#[tokio::test]
async fn an_agent_creates_and_renames_a_plan_and_keeps_the_template_library() {
    let dir = tempfile::tempdir().unwrap();
    let state = planning_state(&dir, true);
    let app = app(&state);

    let created = tool(&app, "create_plan", json!({ "name": "Bubble" }))
        .await
        .unwrap();
    let plan_id = created["id"].as_str().unwrap().to_string();
    let revision = created["revision"].as_u64().unwrap();
    let renamed = tool(
        &app,
        "rename_plan",
        json!({ "plan_id": plan_id, "name": "Bubble Nebula", "expected_revision": revision }),
    )
    .await
    .unwrap();
    assert_eq!(renamed["name"], "Bubble Nebula");
    // The revision it was read at has passed: the API's own conflict.
    assert!(tool(
        &app,
        "rename_plan",
        json!({ "plan_id": plan_id, "name": "Stale", "expected_revision": revision }),
    )
    .await
    .is_err());
    let plan = tool(&app, "get_plan", json!({ "plan_id": plan_id }))
        .await
        .unwrap();
    assert_eq!(plan["project"]["name"], "Bubble Nebula");

    let template_id = uuid::Uuid::new_v4().to_string();
    let template = json!({
        "id": template_id, "revision": 0, "name": "SII 300", "filter_name": "SII",
        "gain": 100, "offset": 30, "bin": 1, "readout_mode": null,
        "default_exposure_seconds": 300.0, "updated_at_ms": 0,
    });
    let saved = tool(
        &app,
        "save_template",
        json!({ "template_id": template_id, "template": template }),
    )
    .await
    .unwrap();
    assert_eq!(saved["revision"], 1);
    let library = tool(&app, "list_templates", json!({})).await.unwrap();
    assert!(library.to_string().contains("SII 300"), "{library}");
    tool(
        &app,
        "delete_template",
        json!({ "template_id": template_id, "revision": 1 }),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn without_database_management_activation_is_refused_as_in_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let state = planning_state(&dir, false);
    let app = app(&state);
    // Editing a plan needs only write access...
    let created = tool(&app, "create_plan", json!({ "name": "Bubble" }))
        .await
        .unwrap();
    let plan_id = created["id"].as_str().unwrap();
    // ...writing to the rigs needs database management too.
    let refused = tool(&app, "push_activation", json!({ "plan_id": plan_id }))
        .await
        .unwrap_err();
    assert!(refused.contains("database management"), "{refused}");
    tool(&app, "list_plans", json!({})).await.unwrap();
}

/// A server with one Target Scheduler catalog on disk: a project, a target,
/// a template and an exposure plan, and an (empty) image folder.
fn catalog_state(dir: &tempfile::TempDir, management: bool) -> Arc<AppState> {
    let path = dir.path().join("rig.sqlite");
    let images = dir.path().join("images");
    std::fs::create_dir_all(&images).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    crate::ts_schema::apply_schema(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO project (Id,profileId,name,description,state,priority,guid) VALUES (1,'p','Bubble','',1,1,'pg');
         INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES (1,'Bubble Nebula',1,23.3,61.2,0,1,'tg');
         INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,guid) VALUES (1,'p','SII','SII',100,'eg');
         INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,guid) VALUES (1,'p',300,50,10,8,1,1,'lg');",
    )
    .unwrap();
    drop(conn);
    let catalog = crate::server::database_context::DatabaseContext::new(
        "rig".into(),
        "Rig".into(),
        path.display().to_string(),
        vec![images.display().to_string()],
        None,
        None,
        None,
        dir.path().join("cache"),
    )
    .unwrap();
    let state = AppState::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
    {
        let mut databases = state.databases.write().unwrap();
        databases.clear();
        databases.insert(catalog.id.clone(), Arc::new(catalog));
    }
    state.set_allow_database_management(management);
    Arc::new(state)
}

#[tokio::test]
async fn an_agent_edits_the_scheduler_and_previews_reject_removal() {
    let dir = tempfile::tempdir().unwrap();
    let state = catalog_state(&dir, false);
    let app = app(&state);
    let db = state.all_databases()[0].id.clone();

    tool(
        &app,
        "update_exposure_plan",
        json!({ "database": db, "exposure_plan_id": 1, "exposure": 600.0, "desired": 30, "enabled": true }),
    )
    .await
    .unwrap();
    tool(
        &app,
        "update_project",
        json!({ "database": db, "project_id": 1, "changes": { "priority": 2 } }),
    )
    .await
    .unwrap();
    let scheduler = tool(
        &app,
        "get_project_scheduler",
        json!({ "database": db, "project_id": 1 }),
    )
    .await
    .unwrap();
    assert_eq!(scheduler["priority"], 2, "{scheduler}");
    assert!(scheduler.to_string().contains("600"), "{scheduler}");

    // No frames: nothing to stack, said plainly.
    let empty = tool(
        &app,
        "start_stack",
        json!({ "database": db, "project_id": 1, "filter_name": "SII" }),
    )
    .await
    .unwrap_err();
    assert_eq!(empty, "No frames match that project, target and filter");

    // No frames to read, so none that cannot be calibrated; the digest
    // still names the (empty) list.
    let uncalibrated = tool(
        &app,
        "check_uncalibrated_lights",
        json!({ "database": db, "project_id": 1 }),
    )
    .await
    .unwrap();
    assert_eq!(uncalibrated["lights"], json!([]));
    assert_eq!(uncalibrated["digest"].as_str().unwrap().len(), 64);
    assert!(tool(
        &app,
        "reject_uncalibrated_lights",
        json!({ "database": db, "project_id": 1, "digest": "stale" }),
    )
    .await
    .unwrap_err()
    .contains("changed since the check"));

    // Rejects move only within an image folder: an editor's, with
    // database management off, as in the app.
    // The preview is a job: it starts, and get_reject_removal has the
    // preview once it is done.
    let started = tool(
        &app,
        "preview_reject_removal",
        json!({ "database": db, "min_age_days": 7 }),
    )
    .await
    .unwrap();
    assert_eq!(started["action"], "preview", "{started}");
    let mut preview = started;
    for _ in 0..400 {
        if preview["state"] != "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        preview = tool(&app, "get_reject_removal", json!({ "database": db }))
            .await
            .unwrap();
    }
    assert_eq!(preview["state"], "done", "{preview}");
    assert_eq!(preview["plan"]["frames"], json!([]), "{preview}");
}

/// A viewer and an editor, each with an API token: (viewer's, editor's).
fn sign_in_with_tokens(state: &AppState) -> (String, String) {
    use crate::auth_registry::{AccessRole, AuthRegistry, AuthTokenRecord, AuthUserRecord};
    let mut accounts = AuthRegistry::default();
    for (name, role) in [
        ("viewer", AccessRole::ReadOnly),
        ("editor", AccessRole::ReadWrite),
    ] {
        accounts
            .add(
                AuthUserRecord::new(name, role, &format!("{name}-password")).unwrap(),
                false,
            )
            .unwrap();
    }
    let (viewer, record) = AuthTokenRecord::mint("viewer", "agent", true, None).unwrap();
    accounts.tokens.push(record);
    let (editor, record) = AuthTokenRecord::mint("editor", "agent", false, None).unwrap();
    accounts.tokens.push(record);
    state.set_server_auth(
        crate::server::auth::ServerAuth::from_sources(None, &accounts, 3000).unwrap(),
    );
    (viewer, editor)
}

const DARK: &str = "2026-10-09_20-01-02_dark_0001.fits";

/// A catalog with a calibration library: one dark recorded twice (in
/// `library` and `drop`, both on disk), a light with a GUID, and a person's
/// split of a flat family recorded at the flat `flat-1`.
fn calibration_state(dir: &tempfile::TempDir, management: bool) -> Arc<AppState> {
    use crate::calibration::flat_groups::overrides::{GroupChoice, GroupOverrideRow};
    let path = dir.path().join("rig.sqlite");
    let images = dir.path().join("images");
    let conn = rusqlite::Connection::open(&path).unwrap();
    crate::ts_schema::apply_schema(&conn).unwrap();
    crate::calibration::ensure_schema(&conn).unwrap();
    crate::calibration::flat_groups::store::add_tables(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO project (Id,profileId,name,description,state,priority,guid) VALUES (1,'p','Bubble','',1,1,'pg');
         INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES (1,'Bubble Nebula',1,23.3,61.2,0,1,'tg');
         INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, profileId, guid)
             VALUES (7, 1, 1, 1000, 'Ha', 0, '{\"FileName\": \"light-7.fits\"}', 'p', 'guid-7');",
    )
    .unwrap();
    for (id, uuid, folder) in [(1, "kept", "library"), (2, "copy", "drop")] {
        let file = images.join(folder).join(DARK);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"one dark exposure").unwrap();
        conn.execute(
            "INSERT INTO psf_guard_calibration_frame
                (id, frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
                 captured_at, file_size, added_at, updated_at)
             VALUES (?1, ?2, 'rig', 'dark', ?3, ?2, 1000, 17, 0, 0)",
            rusqlite::params![id, uuid, file.to_string_lossy()],
        )
        .unwrap();
    }
    crate::calibration::flat_groups::override_store::set_group_override(
        &conn,
        &GroupOverrideRow {
            anchor_frame: "flat-1".into(),
            session_key: "session-1".into(),
            family_key: "family".into(),
            rig_uuid: "rig".into(),
            choice: Some(GroupChoice::Split),
            before_frame: None,
            before_session_key: None,
            label: "Ha, 2026-09-12".into(),
            chosen_at: 10,
            night: Some("2026-09-12".into()),
            before_night: None,
            rotation: Some(0.0),
            before_rotation: None,
        },
    )
    .unwrap();
    drop(conn);
    let catalog = crate::server::database_context::DatabaseContext::new(
        "rig".into(),
        "Rig".into(),
        path.display().to_string(),
        vec![images.display().to_string()],
        None,
        None,
        None,
        dir.path().join("cache"),
    )
    .unwrap();
    let state = AppState::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
    {
        let mut databases = state.databases.write().unwrap();
        databases.clear();
        databases.insert(catalog.id.clone(), Arc::new(catalog));
    }
    state.set_allow_database_management(management);
    Arc::new(state)
}

/// Poll `get_calibration_duplicates`, as `token`, until the check or
/// repair ends.
async fn settled_duplicates(app: &Router, token: Option<&str>) -> Value {
    let began = std::time::Instant::now();
    loop {
        let status = tool_as(
            app,
            token,
            "get_calibration_duplicates",
            json!({ "database": "rig" }),
        )
        .await
        .unwrap();
        if status["state"] != "running" {
            return status;
        }
        assert!(
            began.elapsed() < std::time::Duration::from_secs(60),
            "the duplicate job never ended: {status}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

fn frame_rows(state: &AppState) -> i64 {
    let path = state.all_databases()[0].database_path.clone();
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM psf_guard_calibration_frame",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[tokio::test]
async fn an_agent_reads_flat_groups_and_its_flat_choices_wait_for_database_management() {
    let dir = tempfile::tempdir().unwrap();
    let state = calibration_state(&dir, false);
    let app = app(&state);
    let db = json!("rig");

    let groups = tool(&app, "get_flat_groups", json!({ "database": db }))
        .await
        .unwrap();
    assert_eq!(groups["available"], true, "{groups}");
    assert!(groups["rigs"].is_array(), "{groups}");
    tool(&app, "get_flat_group_use", json!({ "database": db }))
        .await
        .unwrap();
    let options = tool(
        &app,
        "get_light_flat_options",
        json!({ "database": db, "image_ids": [7] }),
    )
    .await
    .unwrap();
    assert_eq!(options["with_choices"], 0, "{options}");

    // A person's choice of flats needs database management, as in the app.
    let refused = tool(
        &app,
        "set_flat_group_override",
        json!({ "database": db, "choice": "reset", "anchor_frame": "flat-1" }),
    )
    .await
    .unwrap_err();
    assert!(refused.contains("database management"), "{refused}");
    let refused = tool(
        &app,
        "set_light_flat_choice",
        json!({ "database": db, "image_ids": [7], "choice": "reset" }),
    )
    .await
    .unwrap_err();
    assert!(refused.contains("database management"), "{refused}");

    // Use anyway needs write access only, as in the app.
    let used = tool(
        &app,
        "set_calibration_frame_choice",
        json!({ "database": db, "frame_uuids": ["kept"], "choice": "use" }),
    )
    .await
    .unwrap();
    assert_eq!(used["updated"], 1, "{used}");
    let frames = tool(
        &app,
        "list_calibration_frames",
        json!({ "database": db, "kind": "dark" }),
    )
    .await
    .unwrap();
    assert_eq!(frames["total"], 2, "{frames}");
    let frame = tool(
        &app,
        "get_calibration_frame",
        json!({ "database": db, "frame_uuid": "kept" }),
    )
    .await
    .unwrap();
    assert_eq!(frame["frame_uuid"], "kept", "{frame}");
    let sessions = tool(&app, "list_calibration_sessions", json!({ "database": db }))
        .await
        .unwrap();
    assert!(sessions["sessions"].is_array(), "{sessions}");

    state.set_allow_database_management(true);
    let reset = tool(
        &app,
        "set_flat_group_override",
        json!({ "database": db, "choice": "reset", "anchor_frame": "flat-1" }),
    )
    .await
    .unwrap();
    assert_eq!(reset["choice"], Value::Null, "{reset}");
    let lights = tool(
        &app,
        "set_light_flat_choice",
        json!({ "database": db, "image_ids": [7], "choice": "reset" }),
    )
    .await
    .unwrap();
    assert_eq!(lights["lights"], 1, "{lights}");

    // The reset queued the rig's lights; a pause holds it, Resume takes it
    // up, and get_jobs lists what is paused.
    let paused = tool(&app, "pause_job", json!({ "database": db, "job": "flats" }))
        .await
        .unwrap();
    assert_eq!(paused, json!(true), "{paused}");
    let flats = tool(&app, "get_jobs", json!({ "database": db }))
        .await
        .unwrap();
    assert_eq!(flats["paused"], json!(["flats"]), "{flats}");
    assert_eq!(flats["flat_measure"]["state"], "paused", "{flats}");
    let resumed = tool(
        &app,
        "resume_job",
        json!({ "database": db, "job": "flats" }),
    )
    .await
    .unwrap();
    assert_eq!(resumed, json!(true), "{resumed}");
    for job in ["flats", "sites"] {
        tool(&app, "pause_job", json!({ "database": db, "job": job }))
            .await
            .unwrap();
    }

    let jobs = tool(&app, "get_jobs", json!({ "database": db }))
        .await
        .unwrap();
    for job in [
        "import",
        "quality_backfill",
        "quality_scan",
        "wbpp_run",
        "calibration_grading",
        "flat_measure",
        "frame_sites",
        "calibration_duplicates",
    ] {
        assert!(jobs.get(job).is_some(), "{job} missing: {jobs}");
        // The loopback caller with no accounts may read every part.
        assert!(
            jobs[job].get("unavailable").is_none(),
            "{job} unavailable: {jobs}"
        );
    }
    assert_eq!(jobs["flat_measure"]["state"], "paused", "{jobs}");
    assert_eq!(jobs["paused"], json!(["flats", "sites"]), "{jobs}");

    // A kind the account does not know is refused, not filtered to nothing.
    let unknown = tool(
        &app,
        "explain_calibration",
        json!({ "database": db, "image_id": 7, "kinds": ["flat", "flats"] }),
    )
    .await
    .unwrap_err();
    assert!(unknown.contains("\"flats\""), "{unknown}");
}

#[tokio::test]
async fn a_duplicate_repair_takes_the_token_of_a_check_and_database_management() {
    let dir = tempfile::tempdir().unwrap();
    let state = calibration_state(&dir, false);
    let app = app(&state);
    let db = json!("rig");

    tool(
        &app,
        "check_calibration_duplicates",
        json!({ "database": db }),
    )
    .await
    .unwrap();
    let checked = settled_duplicates(&app, None).await;
    assert_eq!(checked["state"], "done", "{checked}");
    let report = &checked["report"];
    assert_eq!(report["groups_total"], 1, "{checked}");
    assert_eq!(report["groups"][0]["copies"], 2, "{checked}");
    let token = report["token"].as_str().unwrap().to_string();

    let no_token = tool(
        &app,
        "repair_calibration_duplicates",
        json!({ "database": db, "token": " " }),
    )
    .await
    .unwrap_err();
    assert!(
        no_token.contains("check_calibration_duplicates"),
        "{no_token}"
    );
    let refused = tool(
        &app,
        "repair_calibration_duplicates",
        json!({ "database": db, "token": token }),
    )
    .await
    .unwrap_err();
    assert!(refused.contains("database management"), "{refused}");
    assert_eq!(frame_rows(&state), 2, "a refused repair changes nothing");

    state.set_allow_database_management(true);
    // A token that is not the plan's applies nothing, and says what changed.
    tool(
        &app,
        "repair_calibration_duplicates",
        json!({ "database": db, "token": "stale" }),
    )
    .await
    .unwrap();
    let stale = settled_duplicates(&app, None).await;
    assert!(stale["changed"].is_object(), "{stale}");
    assert_eq!(frame_rows(&state), 2, "a stale token merges nothing");
    let token = stale["report"]["token"].as_str().unwrap().to_string();

    tool(
        &app,
        "repair_calibration_duplicates",
        json!({ "database": db, "token": token }),
    )
    .await
    .unwrap();
    let repaired = settled_duplicates(&app, None).await;
    assert_eq!(repaired["state"], "done", "{repaired}");
    assert_eq!(frame_rows(&state), 1, "{repaired}");
}

#[tokio::test]
async fn a_read_only_token_reads_and_checks_but_does_not_choose() {
    let dir = tempfile::tempdir().unwrap();
    let state = calibration_state(&dir, true);
    let (viewer, editor) = sign_in_with_tokens(&state);
    let app = app(&state);
    let db = json!("rig");

    tool_as(
        &app,
        Some(&viewer),
        "get_flat_groups",
        json!({ "database": db }),
    )
    .await
    .unwrap();
    // A check writes nothing, so a viewer may run it, as in the app.
    tool_as(
        &app,
        Some(&viewer),
        "check_calibration_duplicates",
        json!({ "database": db }),
    )
    .await
    .unwrap();
    settled_duplicates(&app, Some(&viewer)).await;
    for (name, arguments) in [
        (
            "set_calibration_frame_choice",
            json!({ "database": db, "frame_uuids": ["kept"], "choice": "exclude" }),
        ),
        (
            "set_flat_group_override",
            json!({ "database": db, "choice": "reset", "anchor_frame": "flat-1" }),
        ),
        (
            "set_light_flat_choice",
            json!({ "database": db, "image_ids": [7], "choice": "reset" }),
        ),
        (
            "get_light_flat_options",
            json!({ "database": db, "image_ids": [7] }),
        ),
        ("pause_job", json!({ "database": db, "job": "flats" })),
        ("resume_job", json!({ "database": db, "job": "sites" })),
        ("cancel_calibration_duplicates", json!({ "database": db })),
        (
            "repair_calibration_duplicates",
            json!({ "database": db, "token": "any" }),
        ),
    ] {
        let refused = tool_as(&app, Some(&viewer), name, arguments)
            .await
            .unwrap_err();
        assert!(refused.contains("read-only"), "{name}: {refused}");
    }
    let chosen = tool_as(
        &app,
        Some(&editor),
        "set_calibration_frame_choice",
        json!({ "database": db, "frame_uuids": ["kept"], "choice": "exclude" }),
    )
    .await
    .unwrap();
    assert_eq!(chosen["updated"], 1, "{chosen}");
    state
        .catalog_jobs
        .flat_measure
        .job(&state.all_databases()[0].database_path)
        .pause();
}

/// A server with a database registry, so backups can run.
fn backup_state(dir: &tempfile::TempDir, management: bool) -> Arc<AppState> {
    let catalog = dir.path().join("rig").join("schedulerdb.sqlite");
    std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(&catalog).unwrap();
    crate::ts_schema::apply_schema(&conn).unwrap();
    drop(conn);
    let registry = dir.path().join("config").join("config.json");
    let mut entries = crate::db_registry::DbRegistry::default();
    entries
        .add(
            "Imaging Rig".into(),
            catalog.display().to_string(),
            vec![],
            Some("imaging-rig".into()),
        )
        .unwrap();
    entries.save(&registry).unwrap();
    let state = AppState::from_databases(
        vec![],
        dir.path().join("cache"),
        crate::cli::PregenerationConfig::default(),
    )
    .unwrap();
    state.set_registry_path(Some(registry));
    state.set_allow_database_management(management);
    Arc::new(state)
}

async fn backup_ends(state: &AppState) {
    let began = std::time::Instant::now();
    while state.backups.job().is_some() {
        assert!(
            began.elapsed() < std::time::Duration::from_secs(60),
            "the backup never ended"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn backups_through_mcp_never_carry_secrets_or_hand_out_an_archive() {
    let dir = tempfile::tempdir().unwrap();
    let state = backup_state(&dir, true);
    let app = app(&state);

    // Secrets are refused before anything is asked of the server.
    let refused = tool(
        &app,
        "start_backup",
        json!({ "scope": "meta", "include_secrets": true }),
    )
    .await
    .unwrap_err();
    assert_eq!(refused, super::backups::NO_SECRETS);
    assert!(state.backups.job().is_none() && state.backups.last_run().is_none());
    // The schedule has no folder to back up to.
    let no_folder = tool(
        &app,
        "start_backup",
        json!({ "scope": "meta", "destination": "schedule_folder" }),
    )
    .await
    .unwrap_err();
    assert!(no_folder.contains("no folder"), "{no_folder}");

    // An agent that names no scope gets the small one.
    tool(&app, "start_backup", json!({})).await.unwrap();
    backup_ends(&state).await;
    let last = state.backups.last_run().unwrap();
    assert!(last.error.is_none(), "{:?}", last.error);
    let overview = tool(&app, "get_backups", json!({})).await.unwrap();
    assert_eq!(overview["backups_total"], 1, "{overview}");
    let archive = &overview["backups"][0];
    assert_eq!(archive["secrets"], "redacted", "{overview}");
    assert_eq!(archive["scope"], "meta", "{overview}");
    assert!(archive.get("download_path").is_none(), "{overview}");
    let name = archive["file_name"].as_str().unwrap();
    let download = tool(
        &app,
        "api_get",
        json!({ "path": format!("/backups/files/downloads/{name}") }),
    )
    .await
    .unwrap_err();
    assert!(download.contains("not handed to agents"), "{download}");
    let listing = tool(&app, "api_get", json!({ "path": "/backups" }))
        .await
        .unwrap_err();
    assert!(listing.contains("get_backups"), "{listing}");
    let activity = tool(&app, "get_activity", json!({})).await.unwrap();
    assert_eq!(
        activity["backup"]["last_run"]["outcome"], "succeeded",
        "{activity}"
    );

    // An editor's token backs up, never with secrets; a viewer's reads nothing.
    let (viewer, editor) = sign_in_with_tokens(&state);
    let refused = tool_as(&app, Some(&viewer), "get_backups", json!({}))
        .await
        .unwrap_err();
    assert!(refused.contains("read-only"), "{refused}");
    let activity = tool_as(&app, Some(&viewer), "get_activity", json!({}))
        .await
        .unwrap();
    assert!(activity["backup"]["unavailable"].is_string(), "{activity}");
    for name in ["start_backup", "cancel_backup"] {
        let refused = tool_as(&app, Some(&viewer), name, json!({}))
            .await
            .unwrap_err();
        assert!(refused.contains("read-only"), "{name}: {refused}");
    }
    assert!(state.backups.job().is_none(), "a viewer started nothing");
    let refused = tool_as(
        &app,
        Some(&editor),
        "start_backup",
        json!({ "include_secrets": true }),
    )
    .await
    .unwrap_err();
    assert_eq!(refused, super::backups::NO_SECRETS);
    tool_as(
        &app,
        Some(&editor),
        "start_backup",
        json!({ "scope": "meta" }),
    )
    .await
    .unwrap();
    backup_ends(&state).await;

    // Without database management, backups are refused, as in the app.
    state.set_allow_database_management(false);
    for name in ["start_backup", "get_backups", "cancel_backup"] {
        let refused = tool_as(&app, Some(&editor), name, json!({}))
            .await
            .unwrap_err();
        assert!(refused.contains("database management"), "{name}: {refused}");
    }
}

#[test]
fn a_change_s_dust_is_counted_and_its_deepest_marks_named() {
    let spot = |kind: &str, shape: &str, peak: f64, within: bool| {
        json!({
            "kind": kind, "shape": shape, "darker": true, "x": 10.0, "y": 20.0,
            "radius": 300.0, "peak_pct": peak, "bounds": [0, 0, 600, 600],
            "within_tolerance": within, "offset_px": null,
        })
    };
    let split = json!({
        "before_response": "r0", "after_response": "r1",
        "before_night": "2026-08-30", "after_night": "2026-09-01",
        "dust_pct": 0.9, "broad_pct": 0.1, "vignetting_pct": null,
        "vignetting": [], "reason": "0.90% dust (limit 0.5%)",
        "spots": [
            spot("new", "disk", -0.4, false),
            spot("new", "compact", -2.5, false),
            spot("moved", "donut", -0.3, true),
            spot("gone", "compact", 0.2, false),
            spot("new", "patch", -0.1, false),
        ],
        "choice": {
            "choice": "split", "label": "Ha at 72.6°, 2026-09-01 starts a group",
            "chosen_at": 1, "anchor_frame": "flat-1", "agrees": false,
            "orphaned": null, "conflict": null,
        },
    });
    let group = json!({
        "key": "g1", "first_night": "2026-09-01", "last_night": "2026-09-12",
        "frames": 40, "widest": null,
        "pairs": [{ "earlier_response": "r1" }],
        "sessions": [{ "response_key": "r2", "night": "2026-09-12", "frames": 20,
                       "sessions": ["s2"], "bin": 4 }],
    });
    let family = json!({
        "key": "family:ha", "filter": "Ha", "rotation": 72.6, "angles": [],
        "angle_label": "72.6°", "groups": [group], "splits": [split], "joins": [],
    });
    let view = json!({
        "available": true,
        "rigs": [{
            "rig_uuid": "rig", "name": "C925", "measured": true,
            "dust_limit_pct": 0.5, "pattern_limit_pct": 1.0, "own_limits": [],
            "angle_term": null, "angle_pairs": 0,
            "clock": [{ "from": 0, "offset_seconds": -25200 }],
            "families": [family], "events": [], "unmeasured": [], "orphaned": [],
        }],
    });
    let summary = super::calibration::summarize_flat_groups(&view);
    let rig = &summary["rigs"][0];
    assert_eq!(rig["clock"][0]["offset_seconds"], -25200);
    let family = &rig["families"][0];
    assert_eq!(family["angle_label"], "72.6°");
    assert!(family.get("joins").is_none(), "{family}");
    let group = &family["groups"][0];
    assert!(group.get("pairs").is_none(), "{group}");
    assert_eq!(
        group["sessions"][0],
        json!({ "response_key": "r2", "night": "2026-09-12", "frames": 20 })
    );
    let split = &family["splits"][0];
    assert_eq!(split["choice"]["agrees"], false);
    assert!(split.get("spots").is_none(), "{split}");
    let dust = &split["dust"];
    assert_eq!(dust["counted"], 4);
    assert_eq!(dust["within_focus_travel"], 1);
    assert_eq!(dust["by_kind"], json!({ "gone": 1, "new": 3 }));
    assert_eq!(
        dust["by_shape"],
        json!({ "compact": 2, "disk": 1, "patch": 1 })
    );
    let deepest: Vec<&str> = dust["deepest"]
        .as_array()
        .unwrap()
        .iter()
        .map(|spot| spot["shape"].as_str().unwrap())
        .collect();
    assert_eq!(deepest, ["compact", "disk", "compact"]);
    assert!(dust["deepest"][0].get("bounds").is_none());
    assert!(dust.get("listed_at_most").is_none(), "{dust}");
}

/// The counts come from the route's totals of every spot, not from the
/// deepest 12 it lists.
#[test]
fn a_change_s_dust_counts_every_spot_from_the_route_s_totals() {
    let spots: Vec<Value> = (0..12)
        .map(|i| json!({ "kind": "new", "shape": "compact", "peak_pct": -1.0 - f64::from(i) }))
        .collect();
    let totals = json!({
        "counted": 31, "within_tolerance": 4, "listed": 12,
        "by_kind": { "new": 29, "gone": 2 },
        "by_shape": { "compact": 30, "disk": 1 },
    });
    let dust = super::calibration::summarize_spots(&spots, Some(&totals));
    assert_eq!(dust["counted"], 31);
    assert_eq!(dust["within_focus_travel"], 4);
    assert_eq!(dust["by_kind"], json!({ "new": 29, "gone": 2 }));
    assert_eq!(dust["by_shape"], json!({ "compact": 30, "disk": 1 }));
    assert_eq!(dust["listed"], 12);
    assert_eq!(dust["deepest"][0]["peak_pct"], -12.0);
    assert!(dust.get("listed_at_most").is_none(), "{dust}");
}

#[test]
fn a_duplicate_plan_is_listed_a_page_at_a_time() {
    let group = |name: &str| json!({ "kind": "dark", "file_name": name, "members": [{}, {}, {}] });
    let mut status = json!({
        "state": "done",
        "report": {
            "groups": [group("a"), group("b"), group("c")],
            "masters": (0..25).map(|i| json!({ "master_uuid": i })).collect::<Vec<_>>(),
            "token": "t",
        },
    });
    super::calibration::compact_duplicates(&mut status, 1, 1);
    let report = &status["report"];
    assert_eq!(report["groups_total"], 3);
    assert_eq!(
        report["groups"],
        json!([{ "kind": "dark", "file_name": "b", "copies": 3 }])
    );
    assert_eq!(report["masters_total"], 25);
    assert_eq!(report["masters"].as_array().unwrap().len(), 20);
    assert_eq!(report["token"], "t");
}

/// `api_get` reaches no backup route, the archive download included,
/// under any spelling a router might match (#887 review).
#[tokio::test]
async fn api_get_refuses_backup_archives_under_any_spelling() {
    let dir = tempfile::tempdir().unwrap();
    let state = backup_state(&dir, true);
    let app = app(&state);
    tool(&app, "start_backup", json!({ "scope": "meta" }))
        .await
        .unwrap();
    backup_ends(&state).await;
    let overview = tool(&app, "get_backups", json!({})).await.unwrap();
    let name = overview["backups"][0]["file_name"]
        .as_str()
        .unwrap()
        .to_string();
    for path in [
        format!("/backups/files/downloads/{name}"),
        format!("/backups/files/downloads/{name}/"),
        format!("/backups/%66iles/downloads/{name}"),
        format!("/backups/files%2Fdownloads%2F{name}"),
        format!("/backups//files/downloads/{name}"),
        format!("//backups/files/downloads/{name}"),
        format!("/Backups/files/downloads/{name}"),
        format!("/backups/files/downloads/{name}?x=1"),
        format!("/backups/files/downloads/{name}#frag"),
        format!("/%62ackups/files/downloads/{name}"),
    ] {
        let error = tool(&app, "api_get", json!({ "path": path }))
            .await
            .expect_err(&path);
        assert!(
            error.contains("not handed to agents"),
            "{path}: reached the archive: {error}"
        );
    }
}

/// The flat-groups route lists at most 12 spots a change, deepest first.
/// Without the route's totals, a summary of a full list says it may be
/// cut rather than presenting 12 as the change's count (#887 review).
#[test]
fn a_change_s_dust_without_route_totals_says_its_list_may_be_cut() {
    let spots: Vec<Value> = (0..12)
        .map(|i| {
            json!({
                "kind": "new", "shape": "compact", "darker": true, "x": i, "y": 0,
                "radius": 4.0, "peak_pct": -1.0 - f64::from(i), "bounds": [0, 0, 1, 1],
                "within_tolerance": false, "offset_px": null,
            })
        })
        .collect();
    let dust = super::calibration::summarize_spots(&spots, None);
    assert_eq!(dust["counted"], 12);
    assert!(
        dust.get("listed_at_most").is_some() || dust.get("capped").is_some(),
        "a full list of 12 reads as exactly 12 marks: {dust}"
    );
}
