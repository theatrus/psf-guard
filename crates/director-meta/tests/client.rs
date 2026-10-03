use psf_guard_director_meta::{CatalogIdentity, Error, MetaStore, Uuid};
use rusqlite::Connection;
use tempfile::TempDir;

#[test]
fn pairing_is_atomic_single_use_bound_durable_and_revocable() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let catalog = CatalogIdentity {
        id: Uuid::new_v4(),
        origin_instance_id: store.instance_id(),
    };
    let rig = store
        .bind_catalog_rig_after(catalog, "Rig", true, || Ok(()))
        .unwrap()
        .rig
        .id;
    let pairing = "a".repeat(64);
    let token = "b".repeat(64);
    let profile = Uuid::new_v4();
    assert!(store
        .issue_client_pairing(catalog.id, Uuid::new_v4(), &pairing, 100, 200)
        .is_err());
    store
        .issue_client_pairing(catalog.id, rig, &pairing, 100, 200)
        .unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_client BEFORE INSERT ON director_client BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
    assert!(store
        .redeem_client_pairing(&pairing, &token, profile, "NINA", 101)
        .is_err());
    conn.execute_batch("DROP TRIGGER fail_client").unwrap();
    let client = store
        .redeem_client_pairing(&pairing, &token, profile, "NINA", 102)
        .unwrap();
    assert_eq!(client.profile_id, profile);
    assert_eq!(client.catalog_id, catalog.id);
    assert_eq!(client.rig_id, rig);
    assert!(matches!(
        store.redeem_client_pairing(&pairing, &"c".repeat(64), profile, "NINA", 103),
        Err(Error::NotFound)
    ));
    drop(store);
    let mut store = MetaStore::open(&path).unwrap();
    assert_eq!(
        store.client_for_token(&token).unwrap(),
        Some(client.clone())
    );
    assert_eq!(store.clients(rig).unwrap(), vec![client.clone()]);
    assert!(!store
        .revoke_client(Uuid::new_v4(), client.client_id)
        .unwrap());
    assert!(store.revoke_client(rig, client.client_id).unwrap());
    assert_eq!(store.client_for_token(&token).unwrap(), None);
}

#[test]
fn expired_replaced_codes_and_changed_catalog_bindings_fail_closed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let catalog = CatalogIdentity {
        id: Uuid::new_v4(),
        origin_instance_id: store.instance_id(),
    };
    let rig = store
        .bind_catalog_rig_after(catalog, "Rig", true, || Ok(()))
        .unwrap()
        .rig
        .id;
    let a = "a".repeat(64);
    let b = "b".repeat(64);
    let c = "c".repeat(64);
    let profile = Uuid::new_v4();
    store
        .issue_client_pairing(catalog.id, rig, &a, 100, 200)
        .unwrap();
    assert!(matches!(
        store.redeem_client_pairing(&a, &c, profile, "NINA", 200),
        Err(Error::NotFound)
    ));
    store
        .issue_client_pairing(catalog.id, rig, &b, 150, 250)
        .unwrap();
    assert!(matches!(
        store.redeem_client_pairing(&a, &c, profile, "NINA", 151),
        Err(Error::NotFound)
    ));
    store
        .redeem_client_pairing(&b, &c, profile, "NINA", 152)
        .unwrap();
    Connection::open(&path)
        .unwrap()
        .execute("DELETE FROM catalog_rig", [])
        .unwrap();
    assert_eq!(store.client_for_token(&c).unwrap(), None);
}

#[test]
fn schema_ten_migrates_without_changing_coordinator_or_inbox() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let store = MetaStore::create(&path).unwrap();
    let instance = store.instance_id();
    drop(store);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TABLE observing_preferences; DROP TABLE workload_budget; DROP TABLE workload_history; DROP TABLE workload_policy; DROP TABLE equipment_report; DROP TABLE execution_start; DROP TABLE execution_allocation; DROP TABLE program_issue; DROP TABLE rig_contact; DROP TABLE director_client; DROP TABLE director_pairing; PRAGMA user_version=10;",
    )
    .unwrap();
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(store.instance_id(), instance);
    assert!(store.clients(Uuid::new_v4()).unwrap().is_empty());
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        20
    );
    conn.prepare("SELECT * FROM rig_event").unwrap();
}

#[test]
fn competing_exchanges_cannot_mint_two_clients_from_one_code() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let catalog = CatalogIdentity {
        id: Uuid::new_v4(),
        origin_instance_id: store.instance_id(),
    };
    let rig = store
        .bind_catalog_rig_after(catalog, "Rig", true, || Ok(()))
        .unwrap()
        .rig
        .id;
    store
        .issue_client_pairing(catalog.id, rig, &"a".repeat(64), 100, 200)
        .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = ['b', 'c']
        .into_iter()
        .map(|token| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = MetaStore::open(&path).unwrap();
                barrier.wait();
                store.redeem_client_pairing(
                    &"a".repeat(64),
                    &token.to_string().repeat(64),
                    Uuid::new_v4(),
                    "NINA",
                    101,
                )
            })
        })
        .collect();
    let outcomes: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| matches!(r, Err(Error::NotFound)))
            .count(),
        1
    );
    assert_eq!(store.clients(rig).unwrap().len(), 1);
}
