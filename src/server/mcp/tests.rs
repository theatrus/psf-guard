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
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let request = Request::post("/api/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .body(Body::from(body.to_string()))
        .unwrap();
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

/// A server with one Target Scheduler catalog: a project, a target, a
/// template and an exposure plan.
fn catalog_state(management: bool) -> Arc<AppState> {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::ts_schema::apply_schema(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO project (Id,profileId,name,description,state,priority,guid) VALUES (1,'p','Bubble','',1,1,'pg');
         INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES (1,'Bubble Nebula',1,23.3,61.2,0,1,'tg');
         INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,guid) VALUES (1,'p','SII','SII',100,'eg');
         INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,guid) VALUES (1,'p',300,50,10,8,1,1,'lg');",
    )
    .unwrap();
    let state = AppState::new_for_test(conn);
    state.set_allow_database_management(management);
    Arc::new(state)
}

#[tokio::test]
async fn an_agent_edits_the_scheduler_and_meets_the_management_gate() {
    let state = catalog_state(false);
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

    // Removing rejects needs database management, as in the app.
    let refused = tool(
        &app,
        "preview_reject_removal",
        json!({ "database": db, "min_age_days": 7 }),
    )
    .await
    .unwrap_err();
    assert!(refused.to_lowercase().contains("management"), "{refused}");
}
