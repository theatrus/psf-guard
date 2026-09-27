use super::activation::activated;
use super::*;

#[tokio::test]
async fn the_plan_list_joins_links_framing_plan_and_activation_per_project() {
    let a = activated().await;
    let (status, before) = call(&a.f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{before}");
    let rows = before["data"].as_array().unwrap();
    // The fixture's "Global project" (from adoption) and the activated "Heart Nebula".
    let heart = rows
        .iter()
        .find(|r| r["project"]["name"] == "Heart Nebula")
        .unwrap();
    assert_eq!(heart["links"], json!([]));
    assert_eq!(heart["framing"]["revision"], 1);
    assert_eq!(heart["framing"]["target_name"], "IC 1805");
    assert_eq!(heart["framing"]["panels"], 2);
    assert_eq!(heart["plan"]["objectives"], 1);
    assert_eq!(heart["plan"]["rigs"], 1);
    assert_eq!(heart["activation"], Value::Null);

    let (_, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    let (status, _) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest": preview["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, after) = call(&a.f.app, "GET", "/plans", Value::Null, None).await;
    let heart = after["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["project"]["name"] == "Heart Nebula")
        .unwrap();
    assert_eq!(heart["activation"]["revision"], 1);
    assert_eq!(heart["activation"]["rigs"], 1);
    // Activation linked the new Target Scheduler project, so the row now
    // points at the database row Rig planning opens.
    assert_eq!(heart["links"].as_array().unwrap().len(), 1);
    assert_eq!(heart["links"][0]["catalog_slug"], "rig");
    assert_eq!(heart["links"][0]["rig"]["id"], a.rig.to_string());
    assert_eq!(heart["links"][0]["source_name"], "Heart Nebula");
    assert!(heart["links"][0]["source_row_id"].as_i64().unwrap() >= 1);
    let _ = a.objective;
}
