use psf_guard_director_core::{
    framing::{Layout, Mosaic, PanelSize},
    visibility::IcrsPosition,
};
use psf_guard_director_meta::{
    catalog::ProjectMapping, framing::FramingDraft, CatalogIdentity, Error, MetaStore, Uuid,
};

/// A store file in a folder that goes when the returned guard drops.
fn scratch() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    (dir, path)
}

fn draft(project: Uuid, name: &str) -> FramingDraft {
    FramingDraft {
        project_id: project,
        revision: 0,
        target_name: name.into(),
        center: IcrsPosition {
            ra_degrees: 38.2,
            dec_degrees: 61.45,
        },
        position_angle_degrees: 15.0,
        layout: Layout::grid(Mosaic {
            rows: 1,
            columns: 1,
            overlap_percent: 20,
        }),
        panel_rig_id: None,
        panel: Some(PanelSize {
            width_degrees: 2.0,
            height_degrees: 1.5,
        }),
        shown_rig_ids: vec![],
        survey_id: "dss2_color".into(),
        view_fov_degrees: 4.0,
        updated_at_ms: 1,
        rig_framings: vec![],
        layout_revision: 0,
    }
}

/// A catalog bound to a rig with one linked project.
fn linked(store: &mut MetaStore, instance: Uuid, project: Uuid, guid: Uuid, name: &str) -> Uuid {
    let catalog = Uuid::new_v4();
    let binding = store
        .bind_catalog_rig_after(
            CatalogIdentity {
                id: catalog,
                origin_instance_id: instance,
            },
            name,
            true,
            || Ok(()),
        )
        .unwrap();
    store
        .link_catalog_project(&ProjectMapping {
            catalog_id: catalog,
            source_project_guid: guid,
            source_profile_id: "profile".into(),
            project_id: project,
            rig_id: binding.rig.id,
        })
        .unwrap();
    catalog
}

#[test]
fn attaching_moves_links_takes_missing_drafts_and_retires_the_absorbed_plan() {
    let (_dir, path) = scratch();
    let mut store = MetaStore::create(&path).unwrap();
    let instance = store.instance_id();
    let keep = store.create_project(Uuid::new_v4(), "Heart").unwrap().id;
    let other = store
        .create_project(Uuid::new_v4(), "Heart by C925")
        .unwrap()
        .id;
    let keep_catalog = linked(&mut store, instance, keep, Uuid::new_v4(), "Redcat");
    let other_guid = Uuid::new_v4();
    let other_catalog = linked(&mut store, instance, other, other_guid, "C925");
    // Only the absorbed plan has a framing; the kept plan takes it.
    store
        .save_framing_draft(&draft(other, "IC 1805"), 0)
        .unwrap();

    let done = store.attach_project(keep, other).unwrap();
    assert_eq!(
        (done.moved_links, done.framing_taken, done.plan_taken),
        (1, true, false)
    );
    assert_eq!(done.absorbed.name, "Heart by C925");
    assert!(
        store.project(other).unwrap().is_none(),
        "the absorbed plan is gone"
    );
    assert_eq!(
        store.linked_project(other_catalog, other_guid).unwrap(),
        Some(keep)
    );
    let taken = store.framing_draft(keep).unwrap().unwrap();
    assert_eq!(
        (taken.project_id, taken.revision, taken.target_name.as_str()),
        (keep, 1, "IC 1805")
    );
    assert!(store.framing_draft(other).unwrap().is_none());
    assert_eq!(
        store
            .catalog_project_mappings(keep_catalog, None, 8)
            .unwrap()
            .items
            .len(),
        1
    );
    // Gone means gone: attaching it again, or a plan to itself, is refused.
    assert!(matches!(
        store.attach_project(keep, other),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store.attach_project(keep, keep),
        Err(Error::InvalidInput)
    ));

    // A kept plan with a framing of its own keeps it; the absorbed one's is dropped.
    let third = store
        .create_project(Uuid::new_v4(), "Heart again")
        .unwrap()
        .id;
    linked(&mut store, instance, third, Uuid::new_v4(), "Third");
    store
        .save_framing_draft(&draft(third, "Something else"), 0)
        .unwrap();
    let done = store.attach_project(keep, third).unwrap();
    assert!(!done.framing_taken);
    assert_eq!(
        store.framing_draft(keep).unwrap().unwrap().target_name,
        "IC 1805"
    );
    assert_eq!(store.framing_draft(keep).unwrap().unwrap().revision, 1);

    // Two plans on one database cannot be joined.
    let clash = store.create_project(Uuid::new_v4(), "Clash").unwrap().id;
    let rig = store.catalog_rig(other_catalog).unwrap().unwrap().rig.id;
    store
        .link_catalog_project(&ProjectMapping {
            catalog_id: other_catalog,
            source_project_guid: Uuid::new_v4(),
            source_profile_id: "profile".into(),
            project_id: clash,
            rig_id: rig,
        })
        .unwrap();
    assert!(matches!(
        store.attach_project(keep, clash),
        Err(Error::Conflict)
    ));
    assert!(
        store.project(clash).unwrap().is_some(),
        "a refused attach changes nothing"
    );

    // Detaching hands the project a fresh plan and moves only that link.
    let fresh = store
        .detach_project(
            keep,
            other_catalog,
            other_guid,
            Uuid::new_v4(),
            "Heart by C925",
            false,
        )
        .unwrap();
    assert_eq!(fresh.name, "Heart by C925");
    assert_eq!(
        store.linked_project(other_catalog, other_guid).unwrap(),
        Some(fresh.id)
    );
    assert_eq!(
        store.framing_draft(keep).unwrap().unwrap().target_name,
        "IC 1805",
        "the old plan keeps its drafts"
    );
    assert!(matches!(
        store.detach_project(keep, other_catalog, other_guid, Uuid::new_v4(), "x", false),
        Err(Error::NotFound)
    ));
    let reopened = MetaStore::open(&path).unwrap();
    assert!(reopened.project(fresh.id).unwrap().is_some());
}

#[test]
fn forgetting_a_deleted_project_drops_its_link_and_keeps_the_plan() {
    let (_dir, path) = scratch();
    let mut store = MetaStore::create(&path).unwrap();
    let instance = store.instance_id();
    let plan = store.create_project(Uuid::new_v4(), "Heart").unwrap().id;
    let (gone_guid, kept_guid) = (Uuid::new_v4(), Uuid::new_v4());
    let gone = linked(&mut store, instance, plan, gone_guid, "C925");
    let kept = linked(&mut store, instance, plan, kept_guid, "Redcat");
    store
        .save_framing_draft(&draft(plan, "IC 1805"), 0)
        .unwrap();

    let before = store.source_project_plan(gone, gone_guid).unwrap().unwrap();
    assert_eq!(
        (before.plan.id, before.links, before.imported),
        (plan, 2, false)
    );

    assert_eq!(
        store.forget_source_project(gone, gone_guid).unwrap(),
        Some(plan)
    );
    assert_eq!(store.linked_project(gone, gone_guid).unwrap(), None);
    assert_eq!(store.linked_project(kept, kept_guid).unwrap(), Some(plan));
    assert_eq!(
        store
            .source_project_plan(kept, kept_guid)
            .unwrap()
            .unwrap()
            .links,
        1
    );
    assert_eq!(
        store.framing_draft(plan).unwrap().unwrap().target_name,
        "IC 1805",
        "the plan keeps its framing"
    );
    // Nothing is left under the link: the same project can link again.
    let rig = store.catalog_rig(gone).unwrap().unwrap().rig.id;
    let other = store.create_project(Uuid::new_v4(), "Other").unwrap().id;
    store
        .link_catalog_project(&ProjectMapping {
            catalog_id: gone,
            source_project_guid: gone_guid,
            source_profile_id: "profile".into(),
            project_id: other,
            rig_id: rig,
        })
        .unwrap();
    // A project no plan links is nothing to forget.
    assert_eq!(
        store.forget_source_project(kept, Uuid::new_v4()).unwrap(),
        None
    );
    assert!(store
        .source_project_plan(kept, Uuid::new_v4())
        .unwrap()
        .is_none());
}

#[test]
fn deleting_a_plan_removes_it_and_its_links() {
    let (_dir, path) = scratch();
    let mut store = MetaStore::create(&path).unwrap();
    let instance = store.instance_id();
    let plan = store.create_project(Uuid::new_v4(), "Heart").unwrap().id;
    let guid = Uuid::new_v4();
    let catalog = linked(&mut store, instance, plan, guid, "C925");
    store
        .save_framing_draft(&draft(plan, "IC 1805"), 0)
        .unwrap();
    let kept = store.create_project(Uuid::new_v4(), "Kept").unwrap().id;

    assert_eq!(store.plan_links(plan).unwrap(), [(catalog, guid)]);
    // A link the caller did not handle keeps the plan.
    assert!(matches!(
        store.delete_plan(plan, &[], false),
        Err(Error::Conflict)
    ));
    assert!(store.project(plan).unwrap().is_some());
    store.delete_plan(plan, &[(catalog, guid)], false).unwrap();
    assert!(store.project(plan).unwrap().is_none());
    assert!(store.framing_draft(plan).unwrap().is_none());
    assert!(store.plan_links(plan).unwrap().is_empty());
    assert_eq!(store.linked_project(catalog, guid).unwrap(), None);
    assert!(store.project(kept).unwrap().is_some());
    assert!(matches!(
        store.delete_plan(plan, &[], false),
        Err(Error::NotFound)
    ));
    // A plan without any database goes too.
    store.delete_plan(kept, &[], false).unwrap();
    assert!(store.project(kept).unwrap().is_none());
}
