use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::{get, post},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const ORIGIN: &str = "11111111-1111-4111-8111-111111111111";
const OTHER_ORIGIN: &str = "22222222-2222-4222-8222-222222222222";
const TARGET: &str = "33333333-3333-4333-8333-333333333333";
const TOKEN: &str = "flat-history-test-token-1234567890";

fn scope() -> Scope {
    Scope {
        protocol_version: 1,
        catalog_id: "test".into(),
        origin_id: ORIGIN.into(),
    }
}

fn capture(id: i64) -> CaptureRecord {
    CaptureRecord {
        source_row_id: id,
        fingerprint: format!("{id:064x}"),
        target_guid: Some(TARGET.into()),
        target_name: Some("M31".into()),
        profile_id: "profile-one".into(),
        light_session_date: Some(100),
        light_session_id: 42,
        flats_taken_date: Some(200 + id),
        flats_type: Some("AFTER".into()),
        filter_name: Some("L".into()),
        gain: Some(100),
        offset: Some(50),
        bin: Some(1),
        readout_mode: Some(0),
        rotation: Some(0.0),
        roi: Some(1.0),
    }
}

fn snapshot_request(records: Vec<CaptureRecord>) -> SnapshotRequest {
    SnapshotRequest {
        scope: scope(),
        source_name: "C925".into(),
        records,
    }
}

fn records(conn: &Connection) -> Vec<HistoryRecord> {
    list_records(conn, &ListQuery::default()).unwrap().records
}

fn invalidate_one(conn: &mut Connection, record_id: &str) {
    assert_eq!(
        invalidate_records(
            conn,
            &InvalidateRequest {
                record_ids: vec![record_id.into()],
                reason: "Bad illumination".into()
            },
            300
        )
        .unwrap(),
        1
    );
}

fn acknowledgement(record: &HistoryRecord, status: AcknowledgeStatus) -> AcknowledgeRequest {
    AcknowledgeRequest {
        scope: scope(),
        results: vec![AcknowledgeResult {
            record_id: record.record_id.clone(),
            source_row_id: record.capture.source_row_id,
            fingerprint: record.capture.fingerprint.clone(),
            status,
            detail: Some("Native coverage row removed".into()),
        }],
    }
}

#[test]
fn reads_of_an_uninitialized_catalog_do_not_create_tables() {
    let conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "query_only", true).unwrap();
    assert_eq!(list_records(&conn, &ListQuery::default()).unwrap().total, 0);
    assert!(pending_decisions(
        &conn,
        &PendingRequest {
            scope: scope(),
            target_guid: None
        }
    )
    .unwrap()
    .is_empty());
    assert!(!schema_exists(&conn).unwrap());
}

#[test]
fn snapshot_replay_and_target_rename_preserve_identity_and_decisions() {
    let mut conn = Connection::open_in_memory().unwrap();
    let mut request = snapshot_request(vec![capture(1)]);
    store_snapshot(&mut conn, &request, 100).unwrap();
    let id = records(&conn)[0].record_id.clone();
    invalidate_one(&mut conn, &id);
    request.source_name = "Renamed telescope".into();
    request.records[0].target_name = Some("Andromeda".into());
    store_snapshot(&mut conn, &request, 500).unwrap();
    let current = records(&conn).remove(0);
    assert_eq!(current.record_id, id);
    assert_eq!(current.state, HistoryState::Pending);
    assert_eq!(current.reason.as_deref(), Some("Bad illumination"));
    assert_eq!(current.invalidated_at, Some(300));
    assert_eq!(current.capture.target_name.as_deref(), Some("Andromeda"));
    assert_eq!(current.source_name, "Renamed telescope");
    assert_eq!(current.last_seen, 500);
    assert_eq!(
        invalidate_records(
            &mut conn,
            &InvalidateRequest {
                record_ids: vec![id],
                reason: "Changed reason".into()
            },
            600
        )
        .unwrap(),
        0
    );
    assert_eq!(
        records(&conn)[0].reason.as_deref(),
        Some("Bad illumination")
    );
}

#[test]
fn reused_ids_never_erase_pending_decisions_or_revive_superseded_generations() {
    let mut conn = Connection::open_in_memory().unwrap();
    let old = snapshot_request(vec![capture(1), capture(2)]);
    store_snapshot(&mut conn, &old, 100).unwrap();
    let first_id = records(&conn)
        .iter()
        .find(|r| r.capture.source_row_id == 1)
        .unwrap()
        .record_id
        .clone();
    invalidate_one(&mut conn, &first_id);
    let mut new = snapshot_request(vec![capture(1), capture(2)]);
    for record in &mut new.records {
        record.fingerprint = "a".repeat(64);
        record.flats_taken_date = Some(900);
    }
    store_snapshot(&mut conn, &new, 400).unwrap();
    store_snapshot(&mut conn, &old, 500).unwrap();
    let all = records(&conn);
    assert_eq!(all.len(), 4);
    assert_eq!(
        all.iter()
            .filter(|r| r.state == HistoryState::Recorded)
            .count(),
        2
    );
    assert_eq!(
        all.iter()
            .filter(|r| r.state == HistoryState::Superseded)
            .count(),
        1
    );
    let pending = pending_decisions(
        &conn,
        &PendingRequest {
            scope: scope(),
            target_guid: None,
        },
    )
    .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].record_id, first_id);
    assert_eq!(pending[0].fingerprint, format!("{:064x}", 1));
}

#[test]
fn capture_metadata_cannot_change_under_an_existing_fingerprint_and_batch_rolls_back() {
    let mut conn = Connection::open_in_memory().unwrap();
    let initial = snapshot_request(vec![capture(1)]);
    store_snapshot(&mut conn, &initial, 100).unwrap();
    let mut changed = capture(1);
    changed.gain = Some(200);
    assert!(matches!(
        store_snapshot(&mut conn, &snapshot_request(vec![capture(2), changed]), 200),
        Err(AppError::Conflict(_))
    ));
    assert_eq!(records(&conn).len(), 1);
    assert_eq!(records(&conn)[0].last_seen, 100);
    assert!(matches!(
        store_snapshot(
            &mut conn,
            &snapshot_request(vec![capture(2), capture(2)]),
            300
        ),
        Err(AppError::BadRequest(_))
    ));
    assert_eq!(records(&conn).len(), 1);
}

#[test]
fn missing_rows_are_not_treated_as_invalidations() {
    let mut conn = Connection::open_in_memory().unwrap();
    store_snapshot(&mut conn, &snapshot_request(vec![capture(1)]), 100).unwrap();
    store_snapshot(&mut conn, &snapshot_request(Vec::new()), 200).unwrap();
    assert_eq!(records(&conn)[0].state, HistoryState::Recorded);
    assert!(pending_decisions(
        &conn,
        &PendingRequest {
            scope: scope(),
            target_guid: None
        }
    )
    .unwrap()
    .is_empty());
}

#[test]
fn pending_is_scoped_to_origin_and_exact_target_but_includes_profile_coverage() {
    let mut conn = Connection::open_in_memory().unwrap();
    let mut profile = capture(2);
    profile.target_guid = None;
    profile.target_name = None;
    store_snapshot(&mut conn, &snapshot_request(vec![capture(1), profile]), 100).unwrap();
    let mut other = snapshot_request(vec![capture(1)]);
    other.scope.origin_id = OTHER_ORIGIN.into();
    store_snapshot(&mut conn, &other, 100).unwrap();
    for record in records(&conn) {
        invalidate_one(&mut conn, &record.record_id);
    }
    let all = pending_decisions(
        &conn,
        &PendingRequest {
            scope: scope(),
            target_guid: None,
        },
    )
    .unwrap();
    assert_eq!(all.len(), 2);
    let selected = pending_decisions(
        &conn,
        &PendingRequest {
            scope: scope(),
            target_guid: Some(TARGET.into()),
        },
    )
    .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].source_row_id, 1);
    let other = pending_decisions(
        &conn,
        &PendingRequest {
            scope: other.scope,
            target_guid: None,
        },
    )
    .unwrap();
    assert_eq!(other.len(), 1);
    assert_ne!(selected[0].record_id, other[0].record_id);
}

#[test]
fn acknowledgement_is_idempotent_and_preserves_first_audit_details() {
    let mut conn = Connection::open_in_memory().unwrap();
    store_snapshot(&mut conn, &snapshot_request(vec![capture(1)]), 100).unwrap();
    let record = records(&conn).remove(0);
    invalidate_one(&mut conn, &record.record_id);
    let mut ack = acknowledgement(&record, AcknowledgeStatus::Removed);
    assert_eq!(acknowledge_records(&mut conn, &ack, 400).unwrap(), 1);
    ack.results[0].detail = Some("Retry".into());
    assert_eq!(acknowledge_records(&mut conn, &ack, 500).unwrap(), 1);
    store_snapshot(&mut conn, &snapshot_request(vec![capture(1)]), 600).unwrap();
    let result = records(&conn).remove(0);
    assert_eq!(result.state, HistoryState::Removed);
    assert_eq!(result.acknowledged_at, Some(400));
    assert_eq!(
        result.detail.as_deref(),
        Some("Native coverage row removed")
    );
    assert_eq!(result.reason.as_deref(), Some("Bad illumination"));
    assert!(pending_decisions(
        &conn,
        &PendingRequest {
            scope: scope(),
            target_guid: None
        }
    )
    .unwrap()
    .is_empty());
    ack.results[0].status = AcknowledgeStatus::Absent;
    assert_eq!(acknowledge_records(&mut conn, &ack, 700).unwrap(), 1);
    assert_eq!(records(&conn)[0].state, HistoryState::Removed);
    assert_eq!(records(&conn)[0].acknowledged_at, Some(400));
    ack.results[0].status = AcknowledgeStatus::Conflict;
    assert!(matches!(
        acknowledge_records(&mut conn, &ack, 700),
        Err(AppError::Conflict(_))
    ));
}

#[test]
fn acknowledgement_refuses_wrong_origin_mismatched_identity_and_unrequested_rows() {
    let mut conn = Connection::open_in_memory().unwrap();
    store_snapshot(&mut conn, &snapshot_request(vec![capture(1)]), 100).unwrap();
    let record = records(&conn).remove(0);
    let mut ack = acknowledgement(&record, AcknowledgeStatus::Conflict);
    assert!(matches!(
        acknowledge_records(&mut conn, &ack, 200),
        Err(AppError::Conflict(_))
    ));
    invalidate_one(&mut conn, &record.record_id);
    ack.scope.origin_id = OTHER_ORIGIN.into();
    assert!(matches!(
        acknowledge_records(&mut conn, &ack, 400),
        Err(AppError::Forbidden(_))
    ));
    ack.scope.origin_id = ORIGIN.into();
    ack.results[0].fingerprint = "f".repeat(64);
    assert!(matches!(
        acknowledge_records(&mut conn, &ack, 400),
        Err(AppError::Conflict(_))
    ));
    ack.results[0].fingerprint = record.capture.fingerprint;
    ack.results[0].source_row_id = 3;
    assert!(matches!(
        acknowledge_records(&mut conn, &ack, 400),
        Err(AppError::Conflict(_))
    ));
    assert_eq!(records(&conn)[0].state, HistoryState::Pending);
}

#[test]
fn invalidation_and_acknowledgement_batches_are_atomic() {
    let mut conn = Connection::open_in_memory().unwrap();
    store_snapshot(
        &mut conn,
        &snapshot_request(vec![capture(1), capture(2)]),
        100,
    )
    .unwrap();
    let all = records(&conn);
    assert!(matches!(
        invalidate_records(
            &mut conn,
            &InvalidateRequest {
                record_ids: vec![all[0].record_id.clone(), Uuid::new_v4().to_string()],
                reason: "Bad flats".into(),
            },
            200
        ),
        Err(AppError::NotFound)
    ));
    assert!(records(&conn)
        .iter()
        .all(|r| r.state == HistoryState::Recorded));
    for record in &all {
        invalidate_one(&mut conn, &record.record_id);
    }
    let mut ack = acknowledgement(&all[0], AcknowledgeStatus::Removed);
    ack.results
        .extend(acknowledgement(&all[1], AcknowledgeStatus::Absent).results);
    ack.results[1].fingerprint = "f".repeat(64);
    assert!(acknowledge_records(&mut conn, &ack, 400).is_err());
    assert!(records(&conn)
        .iter()
        .all(|r| r.state == HistoryState::Pending));
}

#[test]
fn pagination_filters_totals_and_treats_search_wildcards_literally() {
    let mut conn = Connection::open_in_memory().unwrap();
    let mut literal = capture(1);
    literal.target_name = Some("100%_Target".into());
    let mut null_date = capture(3);
    null_date.flats_taken_date = None;
    store_snapshot(
        &mut conn,
        &snapshot_request(vec![literal, capture(2), null_date]),
        100,
    )
    .unwrap();
    let first = list_records(
        &conn,
        &ListQuery {
            limit: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(first.total, 3);
    assert_eq!(first.records[0].capture.source_row_id, 2);
    let last = list_records(
        &conn,
        &ListQuery {
            limit: Some(1),
            offset: Some(2),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(last.records[0].capture.source_row_id, 3);
    let literal = list_records(
        &conn,
        &ListQuery {
            q: Some("%_".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(literal.total, 1);
    invalidate_one(&mut conn, &literal.records[0].record_id);
    let filtered = list_records(
        &conn,
        &ListQuery {
            state: Some(HistoryState::Pending),
            q: Some("c925".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.records[0].capture.source_row_id, 1);
    assert!(list_records(
        &conn,
        &ListQuery {
            limit: Some(0),
            ..Default::default()
        }
    )
    .is_err());
    assert!(list_records(
        &conn,
        &ListQuery {
            q: Some("x".repeat(201)),
            ..Default::default()
        }
    )
    .is_err());
}

#[test]
fn request_validation_bounds_strings_counts_ids_and_finite_values() {
    let mut conn = Connection::open_in_memory().unwrap();
    assert!(store_snapshot(
        &mut conn,
        &snapshot_request((1..=1001).map(capture).collect()),
        100
    )
    .is_err());
    let mut record = capture(1);
    record.rotation = Some(f64::INFINITY);
    assert!(validate_capture(&record).is_err());
    record.rotation = Some(0.0);
    record.fingerprint = "A".repeat(64);
    assert!(validate_capture(&record).is_err());
    record.fingerprint = "a".repeat(64);
    record.source_row_id = 0;
    assert!(validate_capture(&record).is_err());
    assert!(!schema_exists(&conn).unwrap());
    let legacy = json!({"source_row_id":1,"fingerprint":"a".repeat(64),"target_guid":null,"profile_id":"profile"});
    let legacy: CaptureRecord = serde_json::from_value(legacy).unwrap();
    assert_eq!(legacy.light_session_id, 0);
    assert_eq!(legacy.flats_taken_date, None);
    assert!(validate_capture(&legacy).is_ok());
}

fn http_fixture() -> (tempfile::TempDir, Arc<AppState>, Router) {
    let directory = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(Connection::open_in_memory().unwrap());
    let mut state = state;
    state.remote_audit = super::super::remote_audit::RemoteAuditLog::new(directory.path());
    let state = Arc::new(state);
    let mut databases = state.databases.write().unwrap();
    databases.clear();
    for (id, token, sync_enabled) in [
        ("test", TOKEN, true),
        ("other", "other-flat-history-token-1234567890", true),
        ("disabled", "disabled-flat-history-token-1234567890", false),
    ] {
        let path = directory.path().join(format!("{id}.sqlite"));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE flathistory (Id INTEGER PRIMARY KEY); INSERT INTO flathistory VALUES (1);").unwrap();
        let mut ctx = DatabaseContext::new_for_test(conn);
        ctx.id = id.into();
        ctx.database_path = path.to_string_lossy().into_owned();
        let mut config = crate::db_registry::RemoteImageUploadConfig {
            sync_enabled,
            ..Default::default()
        };
        config.set_token(token).unwrap();
        ctx.remote_image_upload = Some(config);
        databases.insert(id.into(), Arc::new(ctx));
    }
    drop(databases);
    let router = Router::new()
        .route(
            "/api/sync/v1/capabilities",
            get(super::super::remote_sync::capabilities),
        )
        .route("/api/sync/v1/flat-history/snapshot", post(snapshot))
        .route("/api/sync/v1/flat-history/pending", post(pending))
        .route("/api/sync/v1/flat-history/acknowledge", post(acknowledge))
        .route("/api/db/{db_id}/flat-history", get(list))
        .route("/api/db/{db_id}/flat-history/invalidate", post(invalidate))
        .with_state(Arc::clone(&state));
    (directory, state, router)
}

async fn call(
    router: &Router,
    method: &str,
    url: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(url);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = if let Some(body) = body {
        request
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    } else {
        request.body(Body::empty()).unwrap()
    };
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn snapshot_json() -> Value {
    json!({"protocol_version":1,"catalog_id":"test","origin_id":ORIGIN,"source_name":"C925","records":[capture(1)]})
}

#[tokio::test]
async fn http_routes_enforce_token_scope_sync_grant_management_and_echo_origin() {
    let (_directory, state, router) = http_fixture();
    let snapshot_url = "/api/sync/v1/flat-history/snapshot";
    assert_eq!(
        call(&router, "POST", snapshot_url, None, Some(snapshot_json()))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &router,
            "POST",
            snapshot_url,
            Some("bad-token"),
            Some(snapshot_json())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &router,
            "POST",
            snapshot_url,
            Some("other-flat-history-token-1234567890"),
            Some(snapshot_json())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &router,
            "POST",
            snapshot_url,
            Some("disabled-flat-history-token-1234567890"),
            Some(snapshot_json())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (_, capabilities) = call(
        &router,
        "GET",
        "/api/sync/v1/capabilities",
        Some(TOKEN),
        None,
    )
    .await;
    assert!(capabilities["data"]["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("flat_history_v1")));
    let (status, result) = call(
        &router,
        "POST",
        snapshot_url,
        Some(TOKEN),
        Some(snapshot_json()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["data"]["origin_id"], ORIGIN);
    assert_eq!(result["data"]["catalog_id"], "test");
    assert_eq!(result["data"]["received"], 1);
    let (status, page) = call(
        &router,
        "GET",
        "/api/db/test/flat-history?state=recorded&limit=1&offset=0&q=M31",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["data"]["total"], 1);
    assert_eq!(page["data"]["records"][0]["target_guid"], TARGET);
    let record_id = page["data"]["records"][0]["record_id"].as_str().unwrap();
    let invalidation = json!({"record_ids":[record_id],"reason":"Flat panel reflection"});
    assert_eq!(
        call(
            &router,
            "POST",
            "/api/db/test/flat-history/invalidate",
            None,
            Some(invalidation.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    state.set_allow_database_management(true);
    assert_eq!(
        call(
            &router,
            "POST",
            "/api/db/test/flat-history/invalidate",
            None,
            Some(invalidation)
        )
        .await
        .0,
        StatusCode::OK
    );
    let pending = json!({"protocol_version":1,"catalog_id":"test","origin_id":ORIGIN});
    let (status, result) = call(
        &router,
        "POST",
        "/api/sync/v1/flat-history/pending",
        Some(TOKEN),
        Some(pending),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["data"]["decisions"][0]["record_id"], record_id);
    let ack = json!({"protocol_version":1,"catalog_id":"test","origin_id":ORIGIN,"results":[{
        "record_id":record_id,"source_row_id":1,"fingerprint":capture(1).fingerprint,"status":"removed","detail":"Removed native coverage"
    }]});
    let (status, result) = call(
        &router,
        "POST",
        "/api/sync/v1/flat-history/acknowledge",
        Some(TOKEN),
        Some(ack),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["data"]["acknowledged"], 1);
    let conn = state.get_database("test").unwrap().db();
    let conn = conn.lock().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM flathistory", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1,
        "server must not delete native history"
    );
    let other = state.get_database("other").unwrap().db();
    assert!(!schema_exists(&other.lock().unwrap()).unwrap());
}
