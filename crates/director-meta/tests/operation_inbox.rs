use psf_guard_director_ledger::preparation::Event;
use psf_guard_director_meta::{
    inbox::{Receipt, RigStatus, Stored},
    Error, MetaStore, Uuid,
};
use serde_json::json;
use tempfile::TempDir;

fn event(rig: Uuid, ledger: Uuid, sequence: u64) -> Event {
    serde_json::from_value(json!({
        "schema_version":1,"contract_version":2,"engine_version":"0.3.0",
        "rig_id":rig,"ledger_id":ledger,"sequence":sequence,
        "assignment_id":"assignment","assignment_revision":1,"configuration_id":"config",
        "preparation_id":"prep","event":{"kind":"completed","observation":{
            "command":{"preparation_id":"prep","ordinal":1,"goal_id":"goal","target_id":"target",
                "recipe_id":"recipe","operation":{"operation":"before_target"}},
            "issued_at_ms":100,"completion":{"preparation_id":"prep","ordinal":1,
                "ended_at_ms":200,"elapsed_ms":90,"outcome":{"outcome":"succeeded"}}
        }}
    }))
    .unwrap()
}

#[test]
fn replay_is_durable_independent_and_does_not_refresh_live_status() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    let ledger = Uuid::new_v4();
    let page = [event(rig, ledger, 1), event(rig, ledger, 2)];
    store
        .record_status(&RigStatus {
            rig_id: rig,
            session_id: "session".into(),
            reported_at_ms: 10,
            received_at_ms: 10,
            payload: json!({"phase":"completed"}),
        })
        .unwrap();
    let (outcomes, cursor) = store.store_operation_receipts(rig, &page, 1000).unwrap();
    assert_eq!(outcomes, vec![Stored::Applied; 2]);
    assert_eq!(cursor.highest_contiguous, 2);
    drop(store);
    let mut store = MetaStore::open(&path).unwrap();
    assert_eq!(
        store.store_operation_receipts(rig, &page, 2000).unwrap().0,
        vec![Stored::Duplicate; 2]
    );
    let recent = store.recent_operations(rig).unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].received_at_ms, 1000);
    assert_eq!(store.rig_status(rig).unwrap().unwrap().received_at_ms, 10);
    assert!(store.saved_captures_by_goal(rig).unwrap().is_empty());
    assert!(store
        .feed_cursor(rig, &ledger.to_string())
        .unwrap()
        .is_none());
    let capture = Receipt {
        rig_id: rig,
        ledger_id: ledger.to_string(),
        sequence: 1,
        goal_id: "goal".into(),
        capture_id: "capture".into(),
        state: "saved".into(),
        payload: json!({}),
        received_at_ms: 0,
    };
    assert_eq!(
        store
            .store_receipts(&[capture], 3000)
            .unwrap()
            .1
            .highest_contiguous,
        1
    );
    assert_eq!(store.recent_operations(rig).unwrap().len(), 2);
}

#[test]
fn rejects_gaps_scope_changes_and_changed_duplicates_without_losing_history() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    let other = store.create_rig(Uuid::new_v4(), "Other").unwrap().id;
    let ledger = Uuid::new_v4();
    assert!(matches!(
        store.store_operation_receipts(rig, &[event(rig, ledger, 2)], 10),
        Err(Error::Conflict)
    ));
    store
        .store_operation_receipts(rig, &[event(rig, ledger, 1)], 10)
        .unwrap();
    assert!(matches!(
        store.store_operation_receipts(other, &[event(other, ledger, 1)], 20),
        Err(Error::Conflict)
    ));
    let foreign = Receipt {
        rig_id: other,
        ledger_id: ledger.to_string(),
        sequence: 1,
        goal_id: "goal".into(),
        capture_id: "capture".into(),
        state: "saved".into(),
        payload: json!({}),
        received_at_ms: 0,
    };
    assert!(matches!(
        store.store_receipts(&[foreign], 20),
        Err(Error::Conflict)
    ));
    let mut changed = event(rig, ledger, 1);
    changed.preparation_id = "other".into();
    assert!(matches!(
        store.store_operation_receipts(rig, &[changed], 20),
        Err(Error::InvalidInput)
    ));
    let mut changed = event(rig, ledger, 1);
    if let psf_guard_director_ledger::preparation::EventKind::Completed { observation } =
        &mut changed.event
    {
        observation.completion.elapsed_ms += 1;
    }
    assert_eq!(
        store
            .store_operation_receipts(rig, &[changed], 20)
            .unwrap()
            .0,
        vec![Stored::Conflict]
    );
    assert_eq!(store.recent_operations(rig).unwrap()[0].received_at_ms, 10);
}

#[test]
fn upgrades_version_21_and_preserves_existing_status() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    store
        .record_status(&RigStatus {
            rig_id: rig,
            session_id: "session".into(),
            reported_at_ms: 1,
            received_at_ms: 1,
            payload: json!({"phase":"completed"}),
        })
        .unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TABLE rig_operation_event; DROP TABLE rig_operation_feed; PRAGMA user_version=21;",
    )
    .unwrap();
    drop(conn);
    let mut store = MetaStore::open(&path).unwrap();
    assert_eq!(
        store.rig_status(rig).unwrap().unwrap().payload["phase"],
        "completed"
    );
    store
        .store_operation_receipts(rig, &[event(rig, Uuid::new_v4(), 1)], 10)
        .unwrap();
}

#[test]
fn corrupt_cursor_and_history_fail_closed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    let ledger = Uuid::new_v4();
    store
        .store_operation_receipts(rig, &[event(rig, ledger, 1)], 10)
        .unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute("UPDATE rig_operation_feed SET highest_contiguous=-1", [])
        .unwrap();
    assert!(matches!(
        store.store_operation_receipts(rig, &[event(rig, ledger, 1)], 20),
        Err(Error::CorruptDatabase)
    ));
    conn.execute("UPDATE rig_operation_event SET received_at_ms=-1", [])
        .unwrap();
    assert!(matches!(
        store.recent_operations(rig),
        Err(Error::CorruptDatabase)
    ));
}
