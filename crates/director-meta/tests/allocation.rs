use psf_guard_director_meta::{allocation::Allocation, CatalogIdentity, Error, MetaStore, Uuid};
use serde_json::{json, Value};

fn fixture(path: &std::path::Path) -> (MetaStore, Allocation) {
    let mut store = MetaStore::create(path).unwrap();
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
    let client = store
        .redeem_client_pairing(
            &"a".repeat(64),
            &"b".repeat(64),
            Uuid::new_v4(),
            "NINA",
            101,
        )
        .unwrap();
    let id = Uuid::new_v4();
    let mut program: Value = serde_json::from_str(include_str!(
        "../../director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    program["assignment"]["id"] = json!(format!("allocation-{id}"));
    program["assignment"]["rig_id"] = json!(rig);
    program["configuration"]["rig_id"] = json!(rig);
    let allocation = Allocation {
        schema_version: 1,
        allocation_id: id,
        coordinator_instance_id: store.instance_id(),
        catalog_id: catalog.id,
        rig_id: rig,
        client_id: client.client_id,
        profile_id: client.profile_id,
        preview_revision: "a".repeat(64),
        admitted_at_ms: 1001,
        snapshot: json!({"coordinator_instance_id":store.instance_id(),"catalog_id":catalog.id,"rig_id":rig,"program":program}),
    };
    (store, allocation)
}

#[test]
fn start_is_one_shot_even_after_restart_or_with_the_same_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    let (mut store, a) = fixture(&path);
    store.admit_allocation(&a).unwrap();
    let ledger = Uuid::new_v4();
    assert!(store
        .start_allocation(a.rig_id, a.allocation_id, Uuid::new_v4(), ledger, 1001)
        .is_err());
    store
        .start_allocation(a.rig_id, a.allocation_id, a.client_id, ledger, 1001)
        .unwrap();
    drop(store);
    let mut store = MetaStore::open(&path).unwrap();
    for id in [ledger, Uuid::new_v4()] {
        assert!(matches!(
            store.start_allocation(a.rig_id, a.allocation_id, a.client_id, id, 1002),
            Err(Error::Conflict)
        ));
    }
}

#[test]
fn migration_from_fifteen_preserves_grant_and_enables_only_one_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    let (mut store, a) = fixture(&path);
    store.admit_allocation(&a).unwrap();
    drop(store);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TABLE execution_start; PRAGMA user_version=15;")
        .unwrap();
    let mut store = MetaStore::open(&path).unwrap();
    assert_eq!(store.allocation(a.rig_id).unwrap(), Some(a.clone()));
    store
        .start_allocation(a.rig_id, a.allocation_id, a.client_id, Uuid::new_v4(), 1001)
        .unwrap();
    assert!(matches!(
        store.start_allocation(a.rig_id, a.allocation_id, a.client_id, Uuid::new_v4(), 1002),
        Err(Error::Conflict)
    ));
}

#[test]
fn simultaneous_launches_consume_only_one_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    let (mut store, a) = fixture(&path);
    store.admit_allocation(&a).unwrap();
    drop(store);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let a = a.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = MetaStore::open(&path).unwrap();
                barrier.wait();
                store.start_allocation(a.rig_id, a.allocation_id, a.client_id, Uuid::new_v4(), 1001)
            })
        })
        .collect();
    let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| matches!(r, Err(Error::Conflict)))
            .count(),
        1
    );
}

#[test]
fn allocation_is_immutable_durable_and_revocation_does_not_erase_outstanding_work() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    let (mut store, a) = fixture(&path);
    assert_eq!(store.admit_allocation(&a).unwrap(), a);
    drop(store);
    let mut store = MetaStore::open(&path).unwrap();
    assert_eq!(store.admit_allocation(&a).unwrap(), a);
    let mut changed = a.clone();
    changed.snapshot["program"]["assignment"]["goals"][0]["attempts_remaining"] = json!(50);
    assert!(matches!(
        store.admit_allocation(&changed),
        Err(Error::Conflict)
    ));
    changed = a.clone();
    changed.allocation_id = Uuid::new_v4();
    changed.snapshot["program"]["assignment"]["id"] =
        json!(format!("allocation-{}", changed.allocation_id));
    assert!(matches!(
        store.admit_allocation(&changed),
        Err(Error::Conflict)
    ));
    store.revoke_client(a.rig_id, a.client_id).unwrap();
    assert!(matches!(store.admit_allocation(&a), Err(Error::Conflict)));
    assert_eq!(store.allocation(a.rig_id).unwrap(), Some(a.clone()));
    let backup = dir.path().join("backup.sqlite");
    store.backup(&backup).unwrap();
    assert_eq!(
        MetaStore::open(&backup)
            .unwrap()
            .allocation(a.rig_id)
            .unwrap(),
        Some(a)
    );
}

#[test]
fn competing_admission_and_legacy_work_cannot_create_another_budget() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    let (mut first, a) = fixture(&path);
    let mut second = MetaStore::open(&path).unwrap();
    first.admit_allocation(&a).unwrap();
    let mut changed = a.clone();
    changed.client_id = Uuid::new_v4();
    assert!(second.admit_allocation(&changed).is_err());
    let other = dir.path().join("other.sqlite");
    let (mut store, a) = fixture(&other);
    rusqlite::Connection::open(&other)
        .unwrap()
        .execute(
            "INSERT INTO rig_feed VALUES('legacy',?1,0,0,0)",
            [a.rig_id.to_string()],
        )
        .unwrap();
    assert!(matches!(store.admit_allocation(&a), Err(Error::Conflict)));
    assert_eq!(store.allocation(a.rig_id).unwrap(), None);
}

#[test]
fn migration_from_fourteen_preserves_identity_and_rolls_back_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    let (store, _) = fixture(&path);
    let id = store.instance_id();
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("DROP TABLE execution_start; DROP TABLE execution_allocation; PRAGMA user_version=14; CREATE VIEW execution_allocation AS SELECT 1 AS rig_id;").unwrap();
    assert!(MetaStore::open(&path).is_err());
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        14
    );
    conn.execute_batch("DROP VIEW execution_allocation;")
        .unwrap();
    assert_eq!(MetaStore::open(&path).unwrap().instance_id(), id);
}

#[test]
fn concurrent_first_allocations_commit_only_one_budget() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    let (store, a) = fixture(&path);
    drop(store);
    let mut b = a.clone();
    b.allocation_id = Uuid::new_v4();
    b.snapshot["program"]["assignment"]["id"] = json!(format!("allocation-{}", b.allocation_id));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = [a.clone(), b]
        .into_iter()
        .map(|value| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = MetaStore::open(&path).unwrap();
                barrier.wait();
                store.admit_allocation(&value)
            })
        })
        .collect();
    let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| matches!(r, Err(Error::Conflict)))
            .count(),
        1
    );
    assert_eq!(
        MetaStore::open(&path)
            .unwrap()
            .allocation(a.rig_id)
            .unwrap(),
        outcomes.into_iter().find_map(Result::ok)
    );
}

#[test]
fn an_allocation_uuid_cannot_be_reused_by_a_different_rig() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, a) = fixture(&dir.path().join("meta.sqlite"));
    store.admit_allocation(&a).unwrap();
    let catalog = CatalogIdentity {
        id: Uuid::new_v4(),
        origin_instance_id: store.instance_id(),
    };
    let rig = store
        .bind_catalog_rig_after(catalog, "Second", true, || Ok(()))
        .unwrap()
        .rig
        .id;
    store
        .issue_client_pairing(catalog.id, rig, &"c".repeat(64), 100, 200)
        .unwrap();
    let client = store
        .redeem_client_pairing(
            &"c".repeat(64),
            &"d".repeat(64),
            Uuid::new_v4(),
            "Second",
            101,
        )
        .unwrap();
    let mut other = a.clone();
    other.catalog_id = catalog.id;
    other.rig_id = rig;
    other.client_id = client.client_id;
    other.profile_id = client.profile_id;
    other.snapshot["catalog_id"] = json!(catalog.id);
    other.snapshot["rig_id"] = json!(rig);
    other.snapshot["program"]["assignment"]["rig_id"] = json!(rig);
    other.snapshot["program"]["configuration"]["rig_id"] = json!(rig);
    assert!(matches!(
        store.admit_allocation(&other),
        Err(Error::Conflict)
    ));
    assert_eq!(store.allocation(rig).unwrap(), None);
    assert_eq!(store.allocation(a.rig_id).unwrap(), Some(a));
}
