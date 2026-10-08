use psf_guard_director_meta::{
    collaboration_connection::{BackgroundPolicy, ConnectionBinding, ConnectionState},
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
        settings: None,
        background: None,
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
    // Schema 24 bindings had no settings member. Upgrade without changing identity.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", 24).unwrap();
    drop(conn);
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
fn reviewed_settings_survive_reopen_without_changing_agent() {
    use psf_guard_director_interop::workflow::{FilterSetup, Settings};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let rig = Uuid::new_v4();
    store.create_rig(rig, "Rig").unwrap();
    let binding = ConnectionBinding {
        id: Uuid::new_v4(),
        rig_id: rig,
        base_url: "https://example.com/".into(),
        name: "Rig".into(),
        allow_loopback_http: false,
        agent_id: None,
        state: ConnectionState::New,
        settings: None,
        background: None,
    };
    store.create_collaboration_connection(&binding).unwrap();
    let mut configured = binding.clone();
    configured.settings = Some(Settings {
        binning: 1,
        colour: false,
        hours_per_night: 6.0,
        share_status: false,
        filters: [(
            "Ha".into(),
            FilterSetup {
                exposure_seconds: 300.0,
                bandpass_nm: Some(7.0),
            },
        )]
        .into(),
    });
    configured.background = Some(BackgroundPolicy {
        enabled: true,
        catalog_id: Uuid::new_v4(),
        project_ids: vec!["000000000002".into()],
        interval_minutes: 15,
        activate: true,
    });
    store
        .update_collaboration_connection(&binding, &configured)
        .unwrap();
    assert!(matches!(
        store.update_collaboration_connection(&binding, &configured),
        Err(Error::Conflict)
    ));
    assert_eq!(
        store.collaboration_connection_ids().unwrap(),
        vec![binding.id]
    );
    drop(store);
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(
        store.collaboration_connection(binding.id).unwrap(),
        Some(configured)
    );
}

#[test]
fn background_policy_requires_bounded_explicit_consent() {
    let policy = BackgroundPolicy {
        enabled: true,
        catalog_id: Uuid::new_v4(),
        project_ids: vec!["000000000002".into()],
        interval_minutes: 15,
        activate: true,
    };
    policy.validate().unwrap();
    for invalid in [
        BackgroundPolicy {
            catalog_id: Uuid::nil(),
            ..policy.clone()
        },
        BackgroundPolicy {
            project_ids: vec![],
            ..policy.clone()
        },
        BackgroundPolicy {
            project_ids: vec!["000000000002".into(); 2],
            ..policy.clone()
        },
        BackgroundPolicy {
            project_ids: vec!["bad".into()],
            ..policy.clone()
        },
        BackgroundPolicy {
            interval_minutes: 4,
            ..policy.clone()
        },
        BackgroundPolicy {
            interval_minutes: 1441,
            ..policy.clone()
        },
    ] {
        assert!(invalid.validate().is_err());
    }
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
        settings: None,
        background: None,
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
