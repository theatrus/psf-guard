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

fn commission(
    store: &mut MetaStore,
    path: &std::path::Path,
    a: &mut Allocation,
) -> psf_guard_director_meta::workload::Policy {
    use psf_guard_director_meta::profile::{Reported, RigProfile, Source};
    let project = store
        .create_project(Uuid::new_v4(), "Automatic M42")
        .unwrap()
        .id;
    rusqlite::Connection::open(path)
        .unwrap()
        .execute(
            "INSERT INTO activation VALUES(?1,1,'{}')",
            [project.to_string()],
        )
        .unwrap();
    let configuration =
        serde_json::from_value(a.snapshot["program"]["configuration"].clone()).unwrap();
    let mut profile = RigProfile::empty(a.rig_id, 1000);
    profile.configuration = Some(Reported {
        value: configuration,
        source: Source::Plugin {},
        reported_at_ms: 1000,
    });
    let profile = store.save_rig_profile(&profile, 0).unwrap();
    a.snapshot["rig"] = json!({"profile_revision":profile.revision});
    a.snapshot["links"] = json!([{"project_id":project}]);
    let policy = psf_guard_director_meta::workload::Policy {
        rig_id: a.rig_id,
        catalog_id: a.catalog_id,
        client_id: a.client_id,
        profile_id: a.profile_id,
        profile_revision: profile.revision,
        configuration_id: "config-1".into(),
        project_ids: vec![project],
        enabled: true,
        revision: 1,
    };
    store.save_workload_policy(&policy, 0).unwrap()
}

fn receipts(
    a: &Allocation,
    ledger: Uuid,
    terminal: &str,
) -> Vec<psf_guard_director_meta::inbox::Receipt> {
    ["reserved", terminal].iter().enumerate().map(|(i,state)| {
        let evidence = match *state { "reserved" => json!({"state":state}), "saved" => json!({"state":state,"image_id":"capture-1","elapsed_ms":10}), _ => json!({"state":state,"reason":"test"}) };
        psf_guard_director_meta::inbox::Receipt {rig_id:a.rig_id,ledger_id:ledger.to_string(),sequence:i as u64+1,
            goal_id:"short-ha".into(),capture_id:"capture-1".into(),state:(*state).into(),received_at_ms:1100,
            payload:json!({"schema_version":1,"ledger_id":ledger,"sequence":i+1,"contract_version":psf_guard_director_core::CONTRACT_VERSION,
                "engine_version":psf_guard_director_core::ENGINE_VERSION,"assignment_id":a.snapshot["program"]["assignment"]["id"],
                "assignment_revision":a.snapshot["program"]["assignment"]["revision"],"rig_id":a.rig_id,"configuration_id":"config-1",
                "attempt":{"capture_id":"capture-1","goal_id":"short-ha","reserved_at_ms":1001,"evidence":evidence}}) }
    }).collect()
}

#[test]
fn automatic_work_retries_seals_and_preserves_spent_authority() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    let (mut store, mut a) = fixture(&path);
    let p = commission(&mut store, &path, &mut a);
    let first = store.admit_workload(&a, p.revision).unwrap();
    assert_eq!(store.admit_workload(&a, p.revision).unwrap(), first);
    let mut b = a.clone();
    b.allocation_id = Uuid::new_v4();
    b.snapshot["program"]["assignment"]["id"] = json!(format!("allocation-{}", b.allocation_id));
    assert!(store.admit_workload(&b, p.revision).is_err());
    let ledger = Uuid::new_v4();
    store
        .start_allocation(a.rig_id, a.allocation_id, a.client_id, ledger, 1002)
        .unwrap();
    let rows = receipts(&a, ledger, "saved");
    store.store_receipts(&rows[..1], 1100).unwrap();
    assert!(store
        .release_workload(a.rig_id, a.client_id, a.allocation_id, ledger, 1)
        .is_err());
    store.store_receipts(&rows[1..], 1100).unwrap();
    assert!(store
        .release_workload(a.rig_id, a.client_id, a.allocation_id, ledger, 1)
        .is_err());
    let sealed = store
        .release_workload(a.rig_id, a.client_id, a.allocation_id, ledger, 2)
        .unwrap();
    assert_eq!(
        store
            .release_workload(a.rig_id, a.client_id, a.allocation_id, ledger, 2)
            .unwrap(),
        sealed
    );
    let mut extra = rows[1].clone();
    extra.sequence = 3;
    assert!(store.store_receipts(&[extra], 1101).is_err());
    store.store_receipts(&rows, 1101).unwrap();
    let mut program = serde_json::from_value(b.snapshot["program"].clone()).unwrap();
    store.carry_workload_budget(a.rig_id, &mut program).unwrap();
    assert_eq!(program.assignment.goals[0].attempts_remaining, 1);
    assert!(store.admit_workload(&b, p.revision).is_err());
    b.snapshot["program"] = serde_json::to_value(program).unwrap();
    store.admit_workload(&b, p.revision).unwrap();
    assert!(store
        .start_allocation(a.rig_id, a.allocation_id, a.client_id, Uuid::new_v4(), 1102)
        .is_err());
    assert!(store
        .start_allocation(b.rig_id, b.allocation_id, b.client_id, ledger, 1102)
        .is_err());
    drop(store);
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(
        store
            .workload(a.rig_id, a.client_id, a.allocation_id)
            .unwrap(),
        Some(sealed)
    );
}

#[test]
fn old_engine_terminal_receipts_are_accepted_only_for_non_lunar_programs() {
    for lunar in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.sqlite");
        let (mut store, mut a) = fixture(&path);
        if lunar {
            a.snapshot["program"]["recipes"][0]["moon"] =
                serde_json::to_value(psf_guard_director_core::moon::MoonPolicy {
                    enabled: true,
                    ..Default::default()
                })
                .unwrap();
        }
        let policy = commission(&mut store, &path, &mut a);
        store.admit_workload(&a, policy.revision).unwrap();
        let ledger = Uuid::new_v4();
        store
            .start_allocation(a.rig_id, a.allocation_id, a.client_id, ledger, 1002)
            .unwrap();
        let mut rows = receipts(&a, ledger, "saved");
        for row in &mut rows {
            row.payload["engine_version"] = json!("0.2.0");
        }
        store.store_receipts(&rows, 1100).unwrap();
        assert_eq!(
            store
                .release_workload(a.rig_id, a.client_id, a.allocation_id, ledger, 2)
                .is_ok(),
            !lunar
        );
    }
}

#[test]
fn priority_handoff_releases_partial_work_without_refilling_attempts_or_losing_pending() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    let (mut store, mut first) = fixture(&path);
    first.snapshot["program"]["assignment"]["goals"][0]["requested"] = json!(3);
    let policy = commission(&mut store, &path, &mut first);
    store.admit_workload(&first, policy.revision).unwrap();
    let ledger = Uuid::new_v4();
    store
        .start_allocation(
            first.rig_id,
            first.allocation_id,
            first.client_id,
            ledger,
            1002,
        )
        .unwrap();
    let events = receipts(&first, ledger, "saved");
    store.store_receipts(&events, 1100).unwrap();
    store
        .release_workload(
            first.rig_id,
            first.client_id,
            first.allocation_id,
            ledger,
            2,
        )
        .unwrap();
    let mut next = first.clone();
    next.allocation_id = Uuid::new_v4();
    next.preview_revision = "c".repeat(64);
    let mut program: psf_guard_director_core::program::Program =
        serde_json::from_value(next.snapshot["program"].clone()).unwrap();
    program.assignment.id = format!("allocation-{}", next.allocation_id);
    program.assignment.goals[0].priority = 999;
    let pending = store.saved_captures_by_goal(first.rig_id).unwrap();
    assert_eq!(pending, vec![("short-ha".to_string(), 1)]);
    program.assignment.goals[0].pending = pending[0].1;
    store
        .carry_workload_budget(first.rig_id, &mut program)
        .unwrap();
    assert_eq!(program.assignment.goals[0].attempts_remaining, 1);
    assert_eq!(program.assignment.goals[0].accepted, 0);
    assert_eq!(program.assignment.goals[0].requested, 3);
    next.snapshot["program"] = serde_json::to_value(&program).unwrap();
    store.admit_workload(&next, policy.revision).unwrap();
    store
        .release_workload(
            first.rig_id,
            first.client_id,
            first.allocation_id,
            ledger,
            2,
        )
        .unwrap();
    store.store_receipts(&events, 1101).unwrap();
    assert_eq!(store.saved_captures_by_goal(first.rig_id).unwrap(), pending);
    assert!(store
        .start_allocation(
            first.rig_id,
            first.allocation_id,
            first.client_id,
            Uuid::new_v4(),
            1102
        )
        .is_err());
    drop(store);
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(
        store
            .allocation(first.rig_id)
            .unwrap()
            .unwrap()
            .allocation_id,
        next.allocation_id
    );
    assert_eq!(store.saved_captures_by_goal(first.rig_id).unwrap(), pending);
}

#[test]
fn uncertain_receipts_and_changed_commissioning_cannot_authorize_successors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    let (mut store, mut a) = fixture(&path);
    let p = commission(&mut store, &path, &mut a);
    store.admit_workload(&a, p.revision).unwrap();
    let ledger = Uuid::new_v4();
    store
        .start_allocation(a.rig_id, a.allocation_id, a.client_id, ledger, 1002)
        .unwrap();
    store
        .store_receipts(&receipts(&a, ledger, "uncertain"), 1100)
        .unwrap();
    assert!(store
        .release_workload(a.rig_id, a.client_id, a.allocation_id, ledger, 2)
        .is_err());
    let mut p2 = p.clone();
    p2.enabled = false;
    p2.revision = 2;
    store.save_workload_policy(&p2, 1).unwrap();
    assert!(store.save_workload_policy(&p2, 1).is_err());
    // Revocation leaves both the spent grant and receipt history in place.
    store.revoke_client(a.rig_id, a.client_id).unwrap();
    assert!(store.admit_workload(&a, 1).is_err());
    assert!(store
        .workload(a.rig_id, a.client_id, a.allocation_id)
        .unwrap()
        .is_some());
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
fn schema17_migration_preserves_manual_grant_without_automatic_authority() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    let (mut store, a) = fixture(&path);
    store.admit_allocation(&a).unwrap();
    let ledger = Uuid::new_v4();
    store
        .start_allocation(a.rig_id, a.allocation_id, a.client_id, ledger, 1002)
        .unwrap();
    drop(store);
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch(
        "DROP TABLE observing_preferences; DROP TABLE workload_budget; DROP TABLE workload_history; DROP TABLE workload_policy; PRAGMA user_version=17;",
    )
    .unwrap();
    let mut store = MetaStore::open(&path).unwrap();
    assert_eq!(store.allocation(a.rig_id).unwrap(), Some(a.clone()));
    assert!(store.workload_policy(a.rig_id).unwrap().is_none());
    assert!(store
        .release_workload(a.rig_id, a.client_id, a.allocation_id, ledger, 0)
        .is_err());
    assert!(store
        .start_allocation(a.rig_id, a.allocation_id, a.client_id, Uuid::new_v4(), 1003)
        .is_err());
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
        .execute_batch(
            "DROP TABLE observing_preferences; DROP TABLE workload_budget; DROP TABLE workload_history; DROP TABLE workload_policy; DROP TABLE equipment_report; DROP TABLE execution_start; PRAGMA user_version=15;",
        )
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
    conn.execute_batch("DROP TABLE observing_preferences; DROP TABLE workload_budget; DROP TABLE workload_history; DROP TABLE workload_policy; DROP TABLE equipment_report; DROP TABLE execution_start; DROP TABLE execution_allocation; PRAGMA user_version=14; CREATE VIEW execution_allocation AS SELECT 1 AS rig_id;").unwrap();
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
