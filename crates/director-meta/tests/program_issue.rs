use psf_guard_director_meta::{CatalogIdentity, MetaStore, Uuid};

#[test]
fn preview_issuance_survives_restart_and_never_extends_an_identity() {
    let dir = tempfile::tempdir().unwrap();
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
    let first = store
        .issue_program_preview(rig, catalog.id, &"a".repeat(64), 100, 1000)
        .unwrap();
    drop(store);
    let mut store = MetaStore::open(&path).unwrap();
    assert_eq!(
        first,
        store
            .issue_program_preview(rig, catalog.id, &"a".repeat(64), 101, 1000)
            .unwrap()
    );
    let expired = store
        .issue_program_preview(rig, catalog.id, &"a".repeat(64), 1100, 1000)
        .unwrap();
    assert_ne!(first.id, expired.id);
    assert_eq!(expired.issued_at_ms, 1100);
    let changed = store
        .issue_program_preview(rig, catalog.id, &"b".repeat(64), 1101, 1000)
        .unwrap();
    assert_ne!(changed.id, expired.id);
    let reverted = store
        .issue_program_preview(rig, catalog.id, &"a".repeat(64), 1102, 1000)
        .unwrap();
    assert_ne!(reverted.id, expired.id);
    let rollback = store
        .issue_program_preview(rig, catalog.id, &"a".repeat(64), 1099, 1000)
        .unwrap();
    assert_ne!(rollback.id, reverted.id);
    assert!(store
        .issue_program_preview(rig, Uuid::new_v4(), &"a".repeat(64), 1103, 1000)
        .is_err());
    assert!(store
        .issue_program_preview(rig, catalog.id, &"a".repeat(64), u64::MAX, 1000)
        .is_err());
}
