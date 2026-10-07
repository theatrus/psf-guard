use psf_guard_director_meta::{catalog::ProjectMapping, CatalogIdentity, Error, MetaStore, Uuid};
use rusqlite::Connection;
use tempfile::TempDir;

fn identity(store: &MetaStore) -> CatalogIdentity {
    CatalogIdentity {
        id: Uuid::new_v4(),
        origin_instance_id: store.instance_id(),
    }
}
fn mapping(store: &mut MetaStore, catalog: CatalogIdentity, rig: Uuid) -> ProjectMapping {
    store.register_catalog(catalog).unwrap();
    ProjectMapping {
        catalog_id: catalog.id,
        source_project_guid: Uuid::new_v4(),
        source_profile_id: Uuid::new_v4().to_string(),
        rig_id: rig,
        project_id: store.create_project(Uuid::new_v4(), "Project").unwrap().id,
    }
}

#[test]
fn preview_is_non_mutating_and_apply_retries_preserve_database_rig_identity() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let catalog = identity(&store);
    let preview = store.preview_catalog_rig(catalog, "C925", true).unwrap();
    assert_eq!(preview.rig.id, catalog.id);
    assert!(store.catalog_identity(catalog.id).unwrap().is_none());
    assert!(store.catalog_rig(catalog.id).unwrap().is_none());
    assert!(store.rig(catalog.id).unwrap().is_none());
    assert!(matches!(
        store.bind_catalog_rig_after(catalog, "C925", true, || Err(Error::Conflict)),
        Err(Error::Conflict)
    ));
    assert!(store.catalog_identity(catalog.id).unwrap().is_none());
    assert!(store.rig(catalog.id).unwrap().is_none());
    let applied = store
        .bind_catalog_rig_after(catalog, "C925", true, || Ok(()))
        .unwrap();
    assert_eq!(applied, preview);
    assert_eq!(
        store
            .bind_catalog_rig_after(catalog, "Renamed database", false, || Ok(()))
            .unwrap(),
        applied
    );
    assert!(matches!(
        store.preview_catalog_rig(catalog, "C925", true),
        Err(Error::Conflict)
    ));
    drop(store);
    assert_eq!(
        MetaStore::open(&path)
            .unwrap()
            .catalog_rig(catalog.id)
            .unwrap(),
        Some(applied)
    );
}

#[test]
fn a_single_legacy_rig_is_reused_for_all_profiles_without_changing_links() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let catalog = identity(&store);
    let rig = store
        .create_rig(Uuid::new_v4(), "Existing setup owner")
        .unwrap();
    let first = mapping(&mut store, catalog, rig.id);
    let second = mapping(&mut store, catalog, rig.id);
    store
        .link_catalog_projects(&[first.clone(), second.clone()])
        .unwrap();
    let before = store
        .catalog_project_mappings(catalog.id, None, 256)
        .unwrap();
    assert_eq!(
        store
            .preview_catalog_rig(catalog, "Database", false)
            .unwrap()
            .rig,
        rig
    );
    store
        .bind_catalog_rig_after(catalog, "Database", false, || Ok(()))
        .unwrap();
    assert_eq!(
        store
            .catalog_project_mappings(catalog.id, None, 256)
            .unwrap(),
        before
    );
    let other = store.create_rig(Uuid::new_v4(), "Other").unwrap();
    let wrong = mapping(&mut store, catalog, other.id);
    assert!(matches!(
        store.link_catalog_project(&wrong),
        Err(Error::Conflict)
    ));
    let another_catalog = identity(&store);
    let wrong = mapping(&mut store, another_catalog, rig.id);
    assert!(matches!(
        store.link_catalog_project(&wrong),
        Err(Error::Conflict)
    ));
}

#[test]
fn conflicting_prototype_links_are_reported_without_reassignment() {
    for multiple_rigs in [true, false] {
        let dir = TempDir::new().unwrap();
        let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
        let catalog = identity(&store);
        let rig = store.create_rig(Uuid::new_v4(), "Old rig").unwrap();
        let first = mapping(&mut store, catalog, rig.id);
        store.link_catalog_project(&first).unwrap();
        let other_catalog = if multiple_rigs {
            catalog
        } else {
            identity(&store)
        };
        let other_rig = if multiple_rigs {
            store.create_rig(Uuid::new_v4(), "Second rig").unwrap().id
        } else {
            rig.id
        };
        let second = mapping(&mut store, other_catalog, other_rig);
        store.link_catalog_project(&second).unwrap();
        let before = store
            .catalog_project_mappings(catalog.id, None, 256)
            .unwrap();
        assert!(matches!(
            store.preview_catalog_rig(catalog, "Database", false),
            Err(Error::Conflict)
        ));
        assert!(matches!(
            store.bind_catalog_rig_after(catalog, "Database", false, || panic!(
                "conflicts must not finalize"
            )),
            Err(Error::Conflict)
        ));
        assert!(store.catalog_rig(catalog.id).unwrap().is_none());
        assert_eq!(
            store
                .catalog_project_mappings(catalog.id, None, 256)
                .unwrap(),
            before
        );
    }
}

#[test]
fn names_and_unrelated_uuid_collisions_cannot_claim_rigs() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let first = identity(&store);
    let second = identity(&store);
    let a = store
        .bind_catalog_rig_after(first, "Same name", false, || Ok(()))
        .unwrap();
    let b = store
        .bind_catalog_rig_after(second, "Same name", false, || Ok(()))
        .unwrap();
    assert_ne!(a.rig.id, b.rig.id);
    let collision = identity(&store);
    store.create_rig(collision.id, "Unrelated").unwrap();
    assert!(matches!(
        store.preview_catalog_rig(collision, "Database", false),
        Err(Error::Conflict)
    ));
    assert!(store.catalog_identity(collision.id).unwrap().is_none());
    let foreign = CatalogIdentity {
        origin_instance_id: Uuid::new_v4(),
        ..first
    };
    assert!(matches!(
        store.preview_catalog_rig(foreign, "Same name", false),
        Err(Error::Conflict)
    ));
}

#[test]
fn schema_four_upgrade_creates_no_bindings_and_failure_rolls_back() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let catalog = identity(&store);
    let rig = store.create_rig(Uuid::new_v4(), "Legacy").unwrap();
    let source = mapping(&mut store, catalog, rig.id);
    store.link_catalog_project(&source).unwrap();
    drop(store);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TABLE observing_preferences; DROP TABLE workload_budget; DROP TABLE workload_history; DROP TABLE workload_policy; DROP TABLE equipment_report; DROP TABLE execution_start; DROP TABLE execution_allocation; DROP TABLE program_issue; DROP TABLE rig_contact; DROP TABLE director_client; DROP TABLE director_pairing; DROP TABLE rig_status; DROP TABLE rig_feed; DROP TABLE rig_event; DROP TABLE activation; DROP TABLE plan_draft; DROP TABLE framing_draft; DROP TABLE rig_profile; DROP TABLE catalog_rig; PRAGMA user_version=4; CREATE VIEW catalog_rig AS SELECT 1 AS id",
    )
    .unwrap();
    assert!(MetaStore::open(&path).is_err());
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        4
    );
    conn.execute_batch("DROP VIEW catalog_rig").unwrap();
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(store.catalog_rig(catalog.id).unwrap(), None);
    assert_eq!(
        store
            .catalog_project_mappings(catalog.id, None, 256)
            .unwrap()
            .items,
        vec![source]
    );
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        25
    );
    let backup = dir.path().join("backup.sqlite");
    store.backup(&backup).unwrap();
    assert_eq!(
        MetaStore::open(&backup)
            .unwrap()
            .catalog_rig(catalog.id)
            .unwrap(),
        None
    );
}
