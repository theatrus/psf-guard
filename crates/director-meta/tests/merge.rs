use psf_guard_director_core::{
    framing::{Mosaic, PanelSize},
    visibility::IcrsPosition,
};
use psf_guard_director_meta::{
    catalog::ProjectMapping, framing::FramingDraft, CatalogIdentity, Error, MetaStore, Uuid,
};

fn scratch() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("director-meta-merge-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("meta.sqlite")
}

fn draft(project: Uuid, name: &str) -> FramingDraft {
    FramingDraft {
        project_id: project,
        revision: 0,
        target_name: name.into(),
        center: IcrsPosition { ra_degrees: 38.2, dec_degrees: 61.45 },
        position_angle_degrees: 15.0,
        mosaic: Mosaic { rows: 1, columns: 1, overlap_percent: 20 },
        panel_rig_id: None,
        panel: Some(PanelSize { width_degrees: 2.0, height_degrees: 1.5 }),
        shown_rig_ids: vec![],
        survey_id: "dss2_color".into(),
        view_fov_degrees: 4.0,
        updated_at_ms: 1,
        rig_framings: vec![],
    }
}

/// A catalog bound to a rig with one linked project.
fn linked(store: &mut MetaStore, instance: Uuid, project: Uuid, guid: Uuid, name: &str) -> Uuid {
    let catalog = Uuid::new_v4();
    let binding = store
        .bind_catalog_rig_after(CatalogIdentity { id: catalog, origin_instance_id: instance }, name, true, || Ok(()))
        .unwrap();
    store
        .link_catalog_project(&ProjectMapping { catalog_id: catalog, source_project_guid: guid, source_profile_id: "profile".into(), project_id: project, rig_id: binding.rig.id })
        .unwrap();
    catalog
}

#[test]
fn attaching_moves_links_takes_missing_drafts_and_retires_the_absorbed_plan() {
    let path = scratch();
    let mut store = MetaStore::create(&path).unwrap();
    let instance = store.instance_id();
    let keep = store.create_project(Uuid::new_v4(), "Heart").unwrap().id;
    let other = store.create_project(Uuid::new_v4(), "Heart by C925").unwrap().id;
    let keep_catalog = linked(&mut store, instance, keep, Uuid::new_v4(), "Redcat");
    let other_guid = Uuid::new_v4();
    let other_catalog = linked(&mut store, instance, other, other_guid, "C925");
    // Only the absorbed plan has a framing; the kept plan takes it.
    store.save_framing_draft(&draft(other, "IC 1805"), 0).unwrap();

    let done = store.attach_project(keep, other).unwrap();
    assert_eq!((done.moved_links, done.framing_taken, done.plan_taken), (1, true, false));
    assert_eq!(done.absorbed.name, "Heart by C925");
    assert!(store.project(other).unwrap().is_none(), "the absorbed plan is gone");
    assert_eq!(store.linked_project(other_catalog, other_guid).unwrap(), Some(keep));
    let taken = store.framing_draft(keep).unwrap().unwrap();
    assert_eq!((taken.project_id, taken.revision, taken.target_name.as_str()), (keep, 1, "IC 1805"));
    assert!(store.framing_draft(other).unwrap().is_none());
    assert_eq!(store.catalog_project_mappings(keep_catalog, None, 8).unwrap().items.len(), 1);
    // Gone means gone: attaching it again, or a plan to itself, is refused.
    assert!(matches!(store.attach_project(keep, other), Err(Error::NotFound)));
    assert!(matches!(store.attach_project(keep, keep), Err(Error::InvalidInput)));

    // A kept plan with a framing of its own keeps it; the absorbed one's is dropped.
    let third = store.create_project(Uuid::new_v4(), "Heart again").unwrap().id;
    linked(&mut store, instance, third, Uuid::new_v4(), "Third");
    store.save_framing_draft(&draft(third, "Something else"), 0).unwrap();
    let done = store.attach_project(keep, third).unwrap();
    assert!(!done.framing_taken);
    assert_eq!(store.framing_draft(keep).unwrap().unwrap().target_name, "IC 1805");
    assert_eq!(store.framing_draft(keep).unwrap().unwrap().revision, 1);

    // Two plans on one database cannot be joined.
    let clash = store.create_project(Uuid::new_v4(), "Clash").unwrap().id;
    let rig = store.catalog_rig(other_catalog).unwrap().unwrap().rig.id;
    store
        .link_catalog_project(&ProjectMapping { catalog_id: other_catalog, source_project_guid: Uuid::new_v4(), source_profile_id: "profile".into(), project_id: clash, rig_id: rig })
        .unwrap();
    assert!(matches!(store.attach_project(keep, clash), Err(Error::Conflict)));
    assert!(store.project(clash).unwrap().is_some(), "a refused attach changes nothing");

    // Detaching hands the project a fresh plan and moves only that link.
    let fresh = store.detach_project(keep, other_catalog, other_guid, Uuid::new_v4(), "Heart by C925").unwrap();
    assert_eq!(fresh.name, "Heart by C925");
    assert_eq!(store.linked_project(other_catalog, other_guid).unwrap(), Some(fresh.id));
    assert_eq!(store.framing_draft(keep).unwrap().unwrap().target_name, "IC 1805", "the old plan keeps its drafts");
    assert!(matches!(store.detach_project(keep, other_catalog, other_guid, Uuid::new_v4(), "x"), Err(Error::NotFound)));
    let reopened = MetaStore::open(&path).unwrap();
    assert!(reopened.project(fresh.id).unwrap().is_some());
}
