use psf_guard_director_meta::{
    collaboration_connection::{ConnectionBinding, ConnectionState},
    Error, MetaStore, Uuid,
};

#[test]
fn migration_identity_and_compare_exchange_preserve_original_agent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let rig = Uuid::new_v4();
    store.create_rig(rig, "Rig").unwrap();
    let b = ConnectionBinding {
        id: Uuid::new_v4(),
        rig_id: rig,
        base_url: "https://example.com/".into(),
        name: "Rig".into(),
        allow_loopback_http: false,
        agent_id: None,
        state: ConnectionState::New,
    };
    store.create_collaboration_connection(&b).unwrap();
    store.create_collaboration_connection(&b).unwrap();
    let mut registered = b.clone();
    registered.agent_id = Some("000000000001".into());
    registered.state = ConnectionState::Registered;
    store
        .update_collaboration_connection(&b, &registered)
        .unwrap();
    assert!(matches!(
        store.update_collaboration_connection(&b, &registered),
        Err(Error::Conflict)
    ));
    let mut wrong = registered.clone();
    wrong.agent_id = Some("000000000002".into());
    assert!(store
        .update_collaboration_connection(&registered, &wrong)
        .is_err());
    let mut disabled = registered.clone();
    disabled.state = ConnectionState::Disabled;
    store
        .update_collaboration_connection(&registered, &disabled)
        .unwrap();
    drop(store);
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(
        store.collaboration_connections(rig).unwrap(),
        vec![disabled]
    );
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("DROP TABLE collaboration_connection; PRAGMA user_version=23")
        .unwrap();
    drop(conn);
    let store = MetaStore::open(&path).unwrap();
    assert!(store.collaboration_connections(rig).unwrap().is_empty());
}

#[test]
fn indexed_identity_must_match_payload_and_capacity_retries_remain_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let rig = Uuid::new_v4();
    store.create_rig(rig, "Rig").unwrap();
    let first = ConnectionBinding {
        id: Uuid::new_v4(),
        rig_id: rig,
        base_url: "https://example.com/".into(),
        name: "Rig".into(),
        allow_loopback_http: false,
        agent_id: None,
        state: ConnectionState::New,
    };
    store.create_collaboration_connection(&first).unwrap();
    for _ in 1..256 {
        store
            .create_collaboration_connection(&ConnectionBinding {
                id: Uuid::new_v4(),
                ..first.clone()
            })
            .unwrap();
    }
    store.create_collaboration_connection(&first).unwrap();
    assert!(store
        .create_collaboration_connection(&ConnectionBinding {
            id: Uuid::new_v4(),
            ..first.clone()
        })
        .is_err());
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE collaboration_connection SET base_url='https://changed.example/' WHERE id=?1",
        [first.id.to_string()],
    )
    .unwrap();
    drop(conn);
    let store = MetaStore::open(&path).unwrap();
    assert!(matches!(
        store.collaboration_connection(first.id),
        Err(Error::CorruptDatabase)
    ));
}
