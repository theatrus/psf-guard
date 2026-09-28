use psf_guard_director_meta::{
    inbox::{Receipt, RigStatus, Stored},
    Error, MetaStore, Uuid,
};
use serde_json::json;
use tempfile::TempDir;

fn receipt(rig: Uuid, sequence: u64, state: &str, capture: &str) -> Receipt {
    Receipt {
        rig_id: rig,
        ledger_id: "ledger-1".into(),
        sequence,
        goal_id: "goal-a".into(),
        capture_id: capture.into(),
        state: state.into(),
        payload: json!({"sequence": sequence, "state": state, "capture": capture}),
        received_at_ms: 0,
    }
}

#[test]
fn receipts_are_stored_once_acknowledged_by_contiguous_cursor_and_conflicts_named() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    assert!(matches!(
        store.store_receipts(&[receipt(Uuid::new_v4(), 1, "saved", "c1")], 5),
        Err(Error::NotFound)
    ));
    let page = [
        receipt(rig, 1, "reserved", "c1"),
        receipt(rig, 2, "saved", "c1"),
        receipt(rig, 3, "reserved", "c2"),
    ];
    let (outcomes, cursor) = store.store_receipts(&page, 5).unwrap();
    assert_eq!(outcomes, vec![Stored::Applied; 3]);
    assert_eq!(cursor.highest_contiguous, 3);
    assert_eq!(cursor.highest_seen, 3);
    // A replay is acknowledged again without adding anything.
    let (outcomes, cursor) = store.store_receipts(&page, 6).unwrap();
    assert_eq!(outcomes, vec![Stored::Duplicate; 3]);
    assert_eq!(cursor.highest_contiguous, 3);
    // A gap is stored but not acknowledged past it; a changed replay is a conflict.
    let mut changed = receipt(rig, 2, "failed", "c1");
    changed.payload = json!({"different": true});
    let (outcomes, cursor) = store
        .store_receipts(&[changed, receipt(rig, 5, "saved", "c3")], 7)
        .unwrap();
    assert_eq!(outcomes, vec![Stored::Conflict, Stored::Applied]);
    assert_eq!(cursor.highest_contiguous, 3);
    assert_eq!(cursor.highest_seen, 5);
    let (_, cursor) = store
        .store_receipts(&[receipt(rig, 4, "saved", "c2")], 8)
        .unwrap();
    assert_eq!(cursor.highest_contiguous, 5);
    assert_eq!(
        store
            .feed_cursor(rig, "ledger-1")
            .unwrap()
            .unwrap()
            .last_checkin_ms,
        8
    );
    assert_eq!(
        store.saved_captures_by_goal(rig).unwrap(),
        vec![("goal-a".to_owned(), 3)]
    );
    // Pages must be one ledger, ascending, with known states.
    assert!(matches!(
        store.store_receipts(
            &[receipt(rig, 7, "saved", "x"), receipt(rig, 6, "saved", "y")],
            9
        ),
        Err(Error::InvalidInput)
    ));
    assert!(matches!(
        store.store_receipts(&[receipt(rig, 9, "graded", "x")], 9),
        Err(Error::InvalidInput)
    ));
}

#[test]
fn status_keeps_the_newest_report_and_refuses_late_ones() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    let status = |session: &str, at: u64| RigStatus {
        rig_id: rig,
        session_id: session.into(),
        reported_at_ms: at,
        payload: json!({"state": "exposing", "at": at}),
        received_at_ms: at + 1,
    };
    assert!(store.record_status(&status("s1", 100)).unwrap());
    assert!(!store.record_status(&status("s1", 90)).unwrap());
    assert!(!store.record_status(&status("s1", 100)).unwrap());
    assert!(store.record_status(&status("s1", 110)).unwrap());
    // A new session replaces an older one even at the same instant, but an
    // old session's late report never overwrites a newer session.
    assert!(store.record_status(&status("s2", 110)).unwrap());
    assert!(!store.record_status(&status("s1", 105)).unwrap());
    let current = store.rig_status(rig).unwrap().unwrap();
    assert_eq!(current.session_id, "s2");
    assert_eq!(store.rig_statuses().unwrap().len(), 1);
    assert!(matches!(
        store.record_status(&RigStatus {
            rig_id: Uuid::new_v4(),
            ..status("s3", 1)
        }),
        Err(Error::NotFound)
    ));
}

#[test]
fn another_rig_cannot_append_to_or_acknowledge_an_existing_ledger() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let a = store.create_rig(Uuid::new_v4(), "A").unwrap().id;
    let b = store.create_rig(Uuid::new_v4(), "B").unwrap().id;
    let (_, cursor) = store
        .store_receipts(&[receipt(a, 1, "reserved", "c1")], 5)
        .unwrap();
    assert!(matches!(
        store.store_receipts(
            &[receipt(b, 2, "saved", "c2"), receipt(b, 3, "saved", "c3")],
            6
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.feed_cursor(a, "ledger-1").unwrap(), Some(cursor));
    assert_eq!(store.feed_cursor(b, "ledger-1").unwrap(), None);
    assert!(store.saved_captures_by_goal(b).unwrap().is_empty());
    let conn = rusqlite::Connection::open(&path).unwrap();
    // A missing cursor must not erase ownership recorded by existing events.
    conn.execute("DELETE FROM rig_feed", []).unwrap();
    assert!(matches!(
        store.store_receipts(&[receipt(b, 2, "saved", "c2")], 7),
        Err(Error::Conflict)
    ));
    store
        .store_receipts(&[receipt(a, 2, "saved", "c1")], 8)
        .unwrap();
    // A cursor alone is also sufficient to retain ownership.
    conn.execute("DELETE FROM rig_event", []).unwrap();
    assert!(matches!(
        store.store_receipts(&[receipt(b, 3, "saved", "c2")], 9),
        Err(Error::Conflict)
    ));
    // Defend against legacy mixed-rig data instead of acknowledging it.
    store
        .store_receipts(&[receipt(a, 1, "reserved", "c1")], 10)
        .unwrap();
    conn.execute("UPDATE rig_event SET rig_id=?1", [b.to_string()])
        .unwrap();
    assert!(matches!(
        store.store_receipts(&[receipt(a, 2, "saved", "c1")], 11),
        Err(Error::Conflict)
    ));
}

#[test]
fn contacts_keep_the_latest_call_per_kind_and_never_go_backwards() {
    use psf_guard_director_meta::inbox::ContactKind;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "A").unwrap().id;
    assert!(store.contacts_for_rig(rig).unwrap().is_empty());
    store
        .record_contact(rig, ContactKind::ProgramPull, 100, Some("rev-1"))
        .unwrap();
    store
        .record_contact(rig, ContactKind::Status, 120, Some("s1"))
        .unwrap();
    // A late receipt keeps the newer row; a newer one replaces it.
    store
        .record_contact(rig, ContactKind::ProgramPull, 90, Some("rev-0"))
        .unwrap();
    store
        .record_contact(rig, ContactKind::Status, 130, Some("s2"))
        .unwrap();
    let contacts = store.contacts_for_rig(rig).unwrap();
    assert_eq!(contacts.len(), 2);
    let pull = contacts
        .iter()
        .find(|c| c.kind == ContactKind::ProgramPull)
        .unwrap();
    assert_eq!((pull.at_ms, pull.detail.as_deref()), (100, Some("rev-1")));
    let status = contacts
        .iter()
        .find(|c| c.kind == ContactKind::Status)
        .unwrap();
    assert_eq!((status.at_ms, status.detail.as_deref()), (130, Some("s2")));
    assert!(matches!(
        store.record_contact(Uuid::new_v4(), ContactKind::CheckIn, 1, None),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store.record_contact(rig, ContactKind::CheckIn, 1, Some("bad\u{7}")),
        Err(Error::InvalidInput)
    ));
}
