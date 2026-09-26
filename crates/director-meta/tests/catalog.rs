use psf_guard_director_meta::{
    catalog::ProjectMapping, CatalogIdentity, Error, MetaStore, ProjectLink, Uuid,
};
use rusqlite::Connection;
use tempfile::TempDir;

fn fixture(store: &mut MetaStore) -> ProjectMapping {
    let catalog_id = Uuid::new_v4();
    store
        .register_catalog(CatalogIdentity {
            id: catalog_id,
            origin_instance_id: Uuid::new_v4(),
        })
        .unwrap();
    ProjectMapping {
        catalog_id,
        source_project_guid: Uuid::new_v4(),
        source_profile_id: "source-profile".into(),
        project_id: store.create_project(Uuid::new_v4(), "M31").unwrap().id,
        rig_id: store.create_rig(Uuid::new_v4(), "Rig").unwrap().id,
    }
}

fn project_link(mapping: &ProjectMapping) -> ProjectLink {
    ProjectLink {
        project_id: mapping.project_id,
        catalog_id: mapping.catalog_id,
        source_project_guid: mapping.source_project_guid,
    }
}

#[test]
fn confirmed_links_are_idempotent_and_survive_rename_restart_and_backup() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let mapping = fixture(&mut store);
    let catalog = store.catalog_identity(mapping.catalog_id).unwrap().unwrap();
    store.link_catalog_project(&mapping).unwrap();
    store.link_catalog_project(&mapping).unwrap();
    store.rename_rig(mapping.rig_id, 1, "Renamed rig").unwrap();
    store
        .rename_project(mapping.project_id, 1, "Andromeda")
        .unwrap();
    let backup = dir.path().join("backup.sqlite");
    store.backup(&backup).unwrap();
    drop(store);
    for path in [&path, &backup] {
        let store = MetaStore::open(path).unwrap();
        assert_eq!(
            store.catalog_identity(mapping.catalog_id).unwrap(),
            Some(catalog)
        );
        assert_eq!(
            store
                .linked_rig(mapping.catalog_id, &mapping.source_profile_id)
                .unwrap(),
            Some(mapping.rig_id)
        );
        assert_eq!(
            store
                .catalog_project_mappings(mapping.catalog_id, None, 64)
                .unwrap()
                .items,
            vec![mapping.clone()]
        );
    }
}

#[test]
fn catalogs_and_profiles_do_not_imply_one_rig_or_one_project() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let first = fixture(&mut store);
    let mut second = fixture(&mut store);
    // Same source profile and project GUID in distinct catalogs are separate
    // evidence. Confirmed mappings may contribute to the same global project.
    second.source_project_guid = first.source_project_guid;
    second.project_id = first.project_id;
    store.link_catalog_project(&first).unwrap();
    store.link_catalog_project(&second).unwrap();
    let third = ProjectMapping {
        catalog_id: first.catalog_id,
        source_profile_id: "other-profile".into(),
        ..second.clone()
    };
    // A project within one catalog cannot silently move to another profile.
    assert!(matches!(
        store.link_catalog_project(&third),
        Err(Error::Conflict)
    ));
    let third = ProjectMapping {
        source_project_guid: Uuid::new_v4(),
        ..third
    };
    store.link_catalog_project(&third).unwrap();
    assert_eq!(
        store
            .catalog_project_mappings(first.catalog_id, None, 64)
            .unwrap()
            .items
            .len(),
        2
    );
    assert_eq!(
        store
            .catalog_project_mappings(second.catalog_id, None, 64)
            .unwrap()
            .items,
        vec![second]
    );
    let fourth = ProjectMapping {
        source_project_guid: Uuid::new_v4(),
        project_id: store
            .create_project(Uuid::new_v4(), "Another goal")
            .unwrap()
            .id,
        ..first.clone()
    };
    store.link_catalog_project(&fourth).unwrap();
    assert_eq!(
        store
            .linked_rig(first.catalog_id, &first.source_profile_id)
            .unwrap(),
        Some(first.rig_id)
    );
}

#[test]
fn changed_project_rig_or_profile_is_a_conflict_without_partial_links() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let mapping = fixture(&mut store);
    let other = fixture(&mut store);
    store.link_catalog_project(&mapping).unwrap();
    for changed in [
        ProjectMapping {
            project_id: other.project_id,
            ..mapping.clone()
        },
        ProjectMapping {
            rig_id: other.rig_id,
            ..mapping.clone()
        },
        ProjectMapping {
            source_profile_id: "changed-profile".into(),
            ..mapping.clone()
        },
        ProjectMapping {
            source_project_guid: Uuid::new_v4(),
            rig_id: other.rig_id,
            ..mapping.clone()
        },
    ] {
        assert!(matches!(
            store.link_catalog_project(&changed),
            Err(Error::Conflict)
        ));
    }
    assert!(store
        .linked_rig(mapping.catalog_id, "changed-profile")
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .catalog_project_mappings(mapping.catalog_id, None, 64)
            .unwrap()
            .items,
        vec![mapping]
    );
}

#[test]
fn missing_parents_and_invalid_fields_cannot_create_links() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let mapping = fixture(&mut store);
    for changed in [
        ProjectMapping {
            catalog_id: Uuid::new_v4(),
            ..mapping.clone()
        },
        ProjectMapping {
            project_id: Uuid::new_v4(),
            ..mapping.clone()
        },
        ProjectMapping {
            rig_id: Uuid::new_v4(),
            ..mapping.clone()
        },
    ] {
        assert!(matches!(
            store.link_catalog_project(&changed),
            Err(Error::NotFound)
        ));
    }
    for changed in [
        ProjectMapping {
            source_project_guid: Uuid::nil(),
            ..mapping.clone()
        },
        ProjectMapping {
            source_profile_id: " ".into(),
            ..mapping.clone()
        },
        ProjectMapping {
            source_profile_id: "line\nbreak".into(),
            ..mapping.clone()
        },
        ProjectMapping {
            source_profile_id: "x".repeat(513),
            ..mapping.clone()
        },
    ] {
        assert!(matches!(
            store.link_catalog_project(&changed),
            Err(Error::InvalidInput)
        ));
    }
    assert!(store
        .catalog_project_mappings(mapping.catalog_id, None, 64)
        .unwrap()
        .items
        .is_empty());
}

#[test]
fn final_insert_failure_rolls_back_both_new_links() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let mapping = fixture(&mut store);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_link BEFORE INSERT ON project_profile BEGIN SELECT RAISE(ABORT,'injected failure'); END").unwrap();
    assert!(store.link_catalog_project(&mapping).is_err());
    assert!(store
        .linked_project(mapping.catalog_id, mapping.source_project_guid)
        .unwrap()
        .is_none());
    assert!(store
        .linked_rig(mapping.catalog_id, &mapping.source_profile_id)
        .unwrap()
        .is_none());
    conn.execute_batch("DROP TRIGGER fail_link").unwrap();
    store.link_catalog_project(&mapping).unwrap();
}

#[test]
fn batch_conflicts_roll_back_earlier_entries_and_retries_are_idempotent() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let first = fixture(&mut store);
    let second = ProjectMapping {
        source_project_guid: Uuid::new_v4(),
        ..first.clone()
    };
    let bad = ProjectMapping {
        rig_id: store.create_rig(Uuid::new_v4(), "Another rig").unwrap().id,
        ..second.clone()
    };
    assert!(matches!(
        store.link_catalog_projects(&[first.clone(), bad]),
        Err(Error::Conflict)
    ));
    assert!(store
        .linked_project(first.catalog_id, first.source_project_guid)
        .unwrap()
        .is_none());
    assert!(store
        .linked_rig(first.catalog_id, &first.source_profile_id)
        .unwrap()
        .is_none());
    let batch = [first.clone(), second];
    store.link_catalog_projects(&batch).unwrap();
    store.link_catalog_projects(&batch).unwrap();
    assert_eq!(
        store
            .catalog_project_mappings(first.catalog_id, None, 64)
            .unwrap()
            .items
            .len(),
        2
    );
    assert!(matches!(
        store.link_catalog_projects(&[]),
        Err(Error::InvalidInput)
    ));
    assert!(matches!(
        store.link_catalog_projects(&vec![first.clone(); 257]),
        Err(Error::InvalidInput)
    ));
    store.link_catalog_projects(&vec![first; 256]).unwrap();
}

#[test]
fn legacy_project_only_links_require_explicit_profile_confirmation() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let mapping = fixture(&mut store);
    store.link_project(project_link(&mapping)).unwrap();
    assert!(store
        .catalog_project_mappings(mapping.catalog_id, None, 64)
        .unwrap()
        .items
        .is_empty());
    assert!(store
        .linked_rig(mapping.catalog_id, &mapping.source_profile_id)
        .unwrap()
        .is_none());
    store.link_catalog_project(&mapping).unwrap();
    assert_eq!(
        store
            .catalog_project_mappings(mapping.catalog_id, None, 64)
            .unwrap()
            .items,
        vec![mapping]
    );
}

#[test]
fn competing_connections_cannot_assign_a_profile_to_different_rigs() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let first = fixture(&mut store);
    let second = ProjectMapping {
        source_project_guid: Uuid::new_v4(),
        rig_id: store.create_rig(Uuid::new_v4(), "Other").unwrap().id,
        ..first.clone()
    };
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = [first.clone(), second]
        .into_iter()
        .map(|mapping| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = MetaStore::open(&path).unwrap();
                barrier.wait();
                store.link_catalog_project(&mapping)
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::Conflict)))
            .count(),
        1
    );
    assert_eq!(
        store
            .catalog_project_mappings(first.catalog_id, None, 64)
            .unwrap()
            .items
            .len(),
        1
    );
}

#[test]
fn pages_are_bounded_and_source_profile_identity_is_not_normalized() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let mapping = fixture(&mut store);
    for (index, profile) in ["Profile", "profile", " profile ", &"x".repeat(512)]
        .into_iter()
        .enumerate()
    {
        store
            .link_catalog_project(&ProjectMapping {
                source_project_guid: Uuid::from_u128(index as u128 + 1),
                source_profile_id: profile.into(),
                ..mapping.clone()
            })
            .unwrap();
    }
    let first = store
        .catalog_project_mappings(mapping.catalog_id, None, 2)
        .unwrap();
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.next_after, Some(Uuid::from_u128(2)));
    let second = store
        .catalog_project_mappings(mapping.catalog_id, first.next_after, 2)
        .unwrap();
    assert_eq!(second.items.len(), 2);
    assert!(second.next_after.is_none());
    assert_eq!(second.items[0].source_profile_id, " profile ");
    assert!(store
        .linked_rig(mapping.catalog_id, "PROFILE")
        .unwrap()
        .is_none());
    for limit in [0, 257, usize::MAX] {
        assert!(matches!(
            store.catalog_project_mappings(mapping.catalog_id, None, limit),
            Err(Error::InvalidInput)
        ));
    }
    assert!(matches!(
        store.catalog_project_mappings(mapping.catalog_id, Some(Uuid::nil()), 1),
        Err(Error::InvalidInput)
    ));
    assert!(matches!(
        store.catalog_project_mappings(Uuid::new_v4(), None, 1),
        Err(Error::NotFound)
    ));
}

#[test]
fn schema_three_migration_preserves_old_links_and_rolls_back_failures() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let mapping = fixture(&mut store);
    store.link_project(project_link(&mapping)).unwrap();
    let instance = store.instance_id();
    drop(store);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("DROP TABLE project_profile; DROP TABLE catalog_profile; PRAGMA user_version=3; CREATE VIEW project_profile AS SELECT 1 AS id").unwrap();
    assert!(MetaStore::open(&path).is_err());
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='catalog_profile'",
            [],
            |r| r.get::<_, i32>(0)
        )
        .unwrap(),
        0
    );
    conn.execute_batch("DROP VIEW project_profile").unwrap();
    let before = std::fs::read(&path).unwrap();
    let restored = dir.path().join("restored.sqlite");
    MetaStore::restore(&path, &restored).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    for path in [&path, &restored] {
        let mut store = MetaStore::open(path).unwrap();
        assert_eq!(store.instance_id(), instance);
        assert_eq!(
            store
                .linked_project(mapping.catalog_id, mapping.source_project_guid)
                .unwrap(),
            Some(mapping.project_id)
        );
        assert!(store
            .catalog_project_mappings(mapping.catalog_id, None, 64)
            .unwrap()
            .items
            .is_empty());
        store.link_catalog_project(&mapping).unwrap();
    }
}

#[test]
fn orphaned_links_fail_reads_and_cannot_be_published_as_a_backup() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let mapping = fixture(&mut store);
    store.link_catalog_project(&mapping).unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "foreign_keys", false).unwrap();
    conn.execute_batch("DELETE FROM catalog_profile").unwrap();
    assert!(matches!(
        store.link_catalog_project(&mapping),
        Err(Error::CorruptDatabase)
    ));
    assert!(store
        .linked_rig(mapping.catalog_id, &mapping.source_profile_id)
        .unwrap()
        .is_none());
    assert!(matches!(
        store.catalog_project_mappings(mapping.catalog_id, None, 64),
        Err(Error::CorruptDatabase)
    ));
    let backup = dir.path().join("bad.sqlite");
    assert!(matches!(store.backup(&backup), Err(Error::CorruptDatabase)));
    assert!(!backup.exists());
}
