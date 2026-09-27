use super::activation::activated;
use super::*;

fn event(ledger: &str, rig: Uuid, sequence: u64, goal: &str, capture: &str, state: Value) -> Value {
    json!({
        "schema_version": 1, "ledger_id": ledger, "sequence": sequence, "contract_version": 2, "engine_version": "0.6.0",
        "assignment_id": "assignment-x", "assignment_revision": 1, "rig_id": rig, "configuration_id": "config-1",
        "attempt": {"capture_id": capture, "goal_id": goal, "reserved_at_ms": 1_700_000_000_000u64 + sequence, "evidence": state},
    })
}

#[tokio::test]
async fn check_in_stores_receipts_once_acknowledges_cursors_and_status_feeds_the_operator_view() {
    let a = activated().await;
    let instance = a.f.state.director.as_ref().unwrap().instance_id;
    let catalog = a.rig; // the database binding minted the rig id from the catalog id
    let (status, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let (status, _) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest": preview["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let goal: String =
        a.db.query_row(
            "SELECT guid FROM exposureplan ORDER BY Id LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let path = format!("/rigs/{}/checkin", a.rig);
    let body = |events: Vec<Value>, held: Option<&str>| {
        json!({
            "coordinator_instance_id": instance, "catalog_id": catalog, "ledger_id": "ledger-1", "program_revision": held, "events": events,
        })
    };
    let page = vec![
        event(
            "ledger-1",
            a.rig,
            1,
            &goal,
            "cap-1",
            json!({"state": "reserved"}),
        ),
        event(
            "ledger-1",
            a.rig,
            2,
            &goal,
            "cap-1",
            json!({"state": "saved", "image_id": "img-1", "elapsed_ms": 320000}),
        ),
        event(
            "ledger-1",
            a.rig,
            3,
            &goal,
            "cap-2",
            json!({"state": "reserved"}),
        ),
    ];
    // Before any equipment report there is no program yet, and that is not an error here.
    let (status, ack) = call(&a.f.app, "POST", &path, body(page.clone(), None), None).await;
    assert_eq!(status, StatusCode::OK, "{ack}");
    assert_eq!(ack["data"]["acknowledged_through"], 3);
    assert_eq!(ack["data"]["applied"], 3);
    assert_eq!(ack["data"]["program_revision"], Value::Null);
    assert_eq!(ack["data"]["program_changed"], false);
    // A replay is acknowledged again and applies nothing.
    let (_, again) = call(&a.f.app, "POST", &path, body(page.clone(), None), None).await;
    assert_eq!(again["data"]["duplicates"], 3);
    assert_eq!(again["data"]["applied"], 0);
    // A changed replay is named as a conflict and a gap holds the cursor back.
    let mut changed = page[1].clone();
    changed["attempt"]["evidence"] = json!({"state": "failed", "reason": "rewritten"});
    let (_, conflicted) = call(
        &a.f.app,
        "POST",
        &path,
        body(
            vec![
                changed,
                event(
                    "ledger-1",
                    a.rig,
                    5,
                    &goal,
                    "cap-3",
                    json!({"state": "saved", "image_id": "img-3", "elapsed_ms": 1}),
                ),
            ],
            None,
        ),
        None,
    )
    .await;
    assert_eq!(conflicted["data"]["conflicts"], json!([2]));
    assert_eq!(conflicted["data"]["acknowledged_through"], 3);
    assert_eq!(conflicted["data"]["highest_seen"], 5);
    // Events for another rig or ledger, or out of order, are refused whole.
    assert_eq!(
        call(
            &a.f.app,
            "POST",
            &path,
            body(
                vec![event(
                    "ledger-1",
                    Uuid::new_v4(),
                    6,
                    &goal,
                    "x",
                    json!({"state": "reserved"})
                )],
                None
            ),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &a.f.app,
            "POST",
            &path,
            body(
                vec![event(
                    "ledger-2",
                    a.rig,
                    6,
                    &goal,
                    "x",
                    json!({"state": "reserved"})
                )],
                None
            ),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&a.f.app, "POST", &path, body(vec![], None), None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let mut wrong = body(page.clone(), None);
    wrong["catalog_id"] = json!(Uuid::new_v4());
    assert_eq!(
        call(&a.f.app, "POST", &path, wrong, None).await.0,
        StatusCode::FORBIDDEN
    );

    // Once equipment is reported, saved receipts show up as pending credit and
    // check-in says whether the held program is current.
    let program: Value = serde_json::from_str(include_str!(
        "../../../../../crates/director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    let mut configuration = program["configuration"].clone();
    configuration["rig_id"] = json!(a.rig.to_string());
    let (status, _) = call(&a.f.app, "PUT", &format!("/rigs/{}/equipment", a.rig), json!({
        "coordinator_instance_id": instance, "catalog_id": catalog, "configuration": configuration,
        "optics": {"sensor_width_px": 100, "sensor_height_px": 100, "pixel_size_um": 3.76, "focal_length_mm": 250.0, "aperture_mm": null, "rotation": {"mode":"fixed","angle_degrees":0.0}},
        "site": null, "horizon": null, "limits": null, "reported_at_ms": 1_700_000_000_000u64,
    }), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, pulled) = call(
        &a.f.app,
        "GET",
        &format!(
            "/rigs/{}/program?coordinator_instance_id={instance}&catalog_id={catalog}",
            a.rig
        ),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pulled}");
    let revision = pulled["data"]["revision"].as_str().unwrap().to_owned();
    let goals = pulled["data"]["program"]["assignment"]["goals"]
        .as_array()
        .unwrap();
    let mine = goals.iter().find(|g| g["id"] == goal).unwrap();
    assert_eq!(mine["pending"], 2, "{mine}");
    let (_, current) = call(
        &a.f.app,
        "POST",
        &path,
        body(
            vec![event(
                "ledger-1",
                a.rig,
                4,
                &goal,
                "cap-2",
                json!({"state": "failed", "reason": "clouds"}),
            )],
            Some(&revision),
        ),
        None,
    )
    .await;
    assert_eq!(current["data"]["acknowledged_through"], 5);
    assert_eq!(current["data"]["program_changed"], false);
    assert_eq!(current["data"]["program_revision"], revision);
    let (_, stale) = call(
        &a.f.app,
        "POST",
        &path,
        body(
            vec![event(
                "ledger-1",
                a.rig,
                6,
                &goal,
                "cap-4",
                json!({"state": "reserved"}),
            )],
            Some("old"),
        ),
        None,
    )
    .await;
    assert_eq!(stale["data"]["program_changed"], true);

    // Live status: newest wins, late reports are refused, operators see it.
    let status_path = format!("/rigs/{}/status", a.rig);
    let report = |session: &str, at: u64, phase: &str| {
        json!({
            "coordinator_instance_id": instance, "catalog_id": catalog, "session_id": session, "reported_at_ms": at, "program_revision": revision,
            "status": {"phase": phase, "goal_id": goal, "elapsed_ms": 12000, "safety": "safe", "connectivity": "online", "queue_depth": 0},
        })
    };
    let (status, first) = call(
        &a.f.app,
        "POST",
        &status_path,
        report("s1", 100, "exposing"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["data"]["accepted"], true);
    assert_eq!(first["data"]["program_changed"], false);
    let (_, late) = call(
        &a.f.app,
        "POST",
        &status_path,
        report("s1", 90, "waiting"),
        None,
    )
    .await;
    assert_eq!(late["data"]["accepted"], false);
    let mut wrong = report("s1", 200, "exposing");
    wrong["coordinator_instance_id"] = json!(Uuid::new_v4());
    assert_eq!(
        call(&a.f.app, "POST", &status_path, wrong, None).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, listed) = call(&a.f.app, "GET", "/rigs/status", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let rows = listed["data"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["rig"]["id"], a.rig.to_string());
    assert_eq!(rows[0]["status"]["payload"]["phase"], "exposing");
    assert_eq!(rows[0]["status"]["session_id"], "s1");
    assert_eq!(rows[0]["checkins"][0]["ledger_id"], "ledger-1");
    assert_eq!(rows[0]["checkins"][0]["highest_contiguous"], 6);
    let _ = a.objective;
}
