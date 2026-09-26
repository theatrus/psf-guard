use psf_guard_director_meta::{CatalogIdentity, Error, MetaStore, ProjectLink, Uuid};
use rusqlite::Connection;
use std::sync::{Arc, Barrier};
use tempfile::TempDir;

fn catalog(store: &mut MetaStore) -> CatalogIdentity {
    let catalog = CatalogIdentity {
        id: Uuid::new_v4(),
        origin_instance_id: store.instance_id(),
    };
    store.register_catalog(catalog).unwrap();
    catalog
}

#[test]
fn explicit_creation_persists_identity_without_adopting_or_overwriting_files() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    assert!(MetaStore::open(&path).is_err());
    assert!(!path.exists());
    let mut store = MetaStore::create(&path).unwrap();
    let instance = store.instance_id();
    let project = store.create_project(Uuid::new_v4(), "M31").unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Redcat").unwrap();
    drop(store);
    assert!(MetaStore::create(&path).is_err());
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(store.instance_id(), instance);
    assert_eq!(store.project(project.id).unwrap(), Some(project));
    assert_eq!(store.rig(rig.id).unwrap(), Some(rig));
    let other = MetaStore::create(&dir.path().join("other.sqlite")).unwrap();
    assert_ne!(instance, other.instance_id());
}

#[test]
fn names_are_not_identity_and_retries_do_not_rewrite_content() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let a = store.create_project(Uuid::new_v4(), "M31").unwrap();
    let b = store.create_project(Uuid::new_v4(), "M31").unwrap();
    assert_ne!(a.id, b.id);
    assert_eq!(store.create_project(a.id, "M31").unwrap(), a);
    assert!(matches!(
        store.create_project(a.id, "M42"),
        Err(Error::Conflict)
    ));
    let updated = store.rename_project(a.id, 1, "Andromeda").unwrap();
    assert_eq!(updated.revision, 2);
    assert_eq!(store.rename_project(a.id, 2, "Andromeda").unwrap(), updated);
    assert!(matches!(
        store.rename_project(a.id, 1, "Andromeda"),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store.create_project(a.id, "M31"),
        Err(Error::Conflict)
    ));
    assert_eq!(store.project(b.id).unwrap(), Some(b));
    assert!(store.rig(a.id).unwrap().is_none());
}

#[test]
fn rig_renames_do_not_replace_identity_or_require_a_catalog() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "C925").unwrap();
    assert_eq!(store.create_rig(rig.id, "C925").unwrap(), rig);
    assert!(matches!(
        store.create_rig(rig.id, "Redcat"),
        Err(Error::Conflict)
    ));
    let renamed = store.rename_rig(rig.id, 1, "C925 remote").unwrap();
    assert_eq!(renamed.id, rig.id);
    assert_eq!(renamed.revision, 2);
    assert!(matches!(
        store.rename_rig(rig.id, 1, "old edit"),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store.rename_rig(Uuid::new_v4(), 1, "missing"),
        Err(Error::NotFound)
    ));
}

#[test]
fn links_are_explicit_catalog_scoped_and_cannot_silently_change_projects() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let a = store.create_project(Uuid::new_v4(), "same name").unwrap();
    let b = store.create_project(Uuid::new_v4(), "same name").unwrap();
    let c1 = catalog(&mut store);
    let c2 = catalog(&mut store);
    let source_project_guid = Uuid::new_v4();
    let link = ProjectLink {
        project_id: a.id,
        catalog_id: c1.id,
        source_project_guid,
    };
    assert_eq!(
        store.linked_project(c1.id, source_project_guid).unwrap(),
        None
    );
    store.link_project(link).unwrap();
    store.link_project(link).unwrap();
    assert!(matches!(
        store.link_project(ProjectLink {
            project_id: b.id,
            ..link
        }),
        Err(Error::Conflict)
    ));
    store
        .link_project(ProjectLink {
            project_id: b.id,
            catalog_id: c2.id,
            ..link
        })
        .unwrap();
    let other_source = Uuid::new_v4();
    store
        .link_project(ProjectLink {
            catalog_id: c2.id,
            source_project_guid: other_source,
            ..link
        })
        .unwrap();
    drop(store);
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(
        store.linked_project(c1.id, source_project_guid).unwrap(),
        Some(a.id)
    );
    assert_eq!(
        store.linked_project(c2.id, source_project_guid).unwrap(),
        Some(b.id)
    );
    assert_eq!(
        store.linked_project(c2.id, other_source).unwrap(),
        Some(a.id)
    );
}

#[test]
fn unknown_references_and_catalog_origin_changes_are_rejected() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let c = catalog(&mut store);
    store.register_catalog(c).unwrap();
    assert!(matches!(
        store.register_catalog(CatalogIdentity {
            origin_instance_id: Uuid::new_v4(),
            ..c
        }),
        Err(Error::Conflict)
    ));
    let p = store.create_project(Uuid::new_v4(), "M31").unwrap();
    let link = ProjectLink {
        project_id: p.id,
        catalog_id: c.id,
        source_project_guid: Uuid::new_v4(),
    };
    assert!(matches!(
        store.link_project(ProjectLink {
            project_id: Uuid::new_v4(),
            ..link
        }),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store.link_project(ProjectLink {
            catalog_id: Uuid::new_v4(),
            ..link
        }),
        Err(Error::NotFound)
    ));
    assert_eq!(
        store
            .linked_project(link.catalog_id, link.source_project_guid)
            .unwrap(),
        None
    );
}

#[test]
fn concurrent_editors_cannot_lose_a_revision() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let project = store.create_project(Uuid::new_v4(), "M31").unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|name| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut store = MetaStore::open(&path).unwrap();
                barrier.wait();
                store.rename_project(project.id, 1, name)
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
    assert_eq!(store.project(project.id).unwrap().unwrap().revision, 2);
}

#[test]
fn invalid_identity_and_display_name_do_not_enter_storage() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    assert!(matches!(
        store.create_project(Uuid::nil(), "M31"),
        Err(Error::InvalidInput)
    ));
    for name in ["", " ", " M31", "M31 ", "line\nbreak", &"x".repeat(513)] {
        assert!(matches!(
            store.create_project(Uuid::new_v4(), name),
            Err(Error::InvalidInput)
        ));
    }
    assert!(matches!(
        store.register_catalog(CatalogIdentity {
            id: Uuid::new_v4(),
            origin_instance_id: Uuid::nil()
        }),
        Err(Error::InvalidInput)
    ));
}

#[test]
fn failed_sql_write_rolls_back_rename_and_leaves_revision_usable() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let project = store.create_project(Uuid::new_v4(), "M31").unwrap();
    let external = Connection::open(&path).unwrap();
    external.execute_batch("CREATE TRIGGER reject_edit BEFORE UPDATE ON global_project BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(store.rename_project(project.id, 1, "updated").is_err());
    assert_eq!(store.project(project.id).unwrap(), Some(project.clone()));
    external.execute_batch("DROP TRIGGER reject_edit;").unwrap();
    assert_eq!(
        store
            .rename_project(project.id, 1, "updated")
            .unwrap()
            .revision,
        2
    );
}
