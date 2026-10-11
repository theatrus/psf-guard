use psf_guard_director_core::{
    framing::{Mosaic, PanelSize},
    visibility::IcrsPosition,
};
use psf_guard_director_meta::{
    catalog::ProjectMapping, draft_import::DraftImport, framing::FramingDraft, plan::PlanDraft,
    CatalogIdentity, MetaStore, Uuid,
};

/// A store file in a folder that goes when the returned guard drops.
fn scratch() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    (dir, path)
}

fn framing(project: Uuid, at: u64) -> FramingDraft {
    FramingDraft {
        project_id: project,
        revision: 0,
        target_name: "Heart".into(),
        center: IcrsPosition {
            ra_degrees: 38.2,
            dec_degrees: 61.45,
        },
        position_angle_degrees: 0.0,
        mosaic: Mosaic {
            rows: 1,
            columns: 1,
            overlap_percent: 20,
        },
        panel_rig_id: None,
        panel: Some(PanelSize {
            width_degrees: 2.0,
            height_degrees: 1.5,
        }),
        shown_rig_ids: vec![],
        survey_id: "dss2_color".into(),
        view_fov_degrees: 4.0,
        updated_at_ms: at,
        rig_framings: vec![],
        layout_revision: 0,
    }
}

/// A plan linked to one source project in a new catalog, with both drafts
/// saved at the given times.
fn plan(store: &mut MetaStore, name: &str, framing_at: u64, plan_at: u64) -> (Uuid, Uuid, Uuid) {
    let project = store.create_project(Uuid::new_v4(), name).unwrap().id;
    let catalog = Uuid::new_v4();
    let guid = Uuid::new_v4();
    let instance = store.instance_id();
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
    store
        .save_framing_draft(&framing(project, framing_at), 0)
        .unwrap();
    let mut draft = PlanDraft::empty(project, plan_at);
    draft.updated_at_ms = plan_at;
    store.save_plan_draft(&draft, 0).unwrap();
    (project, catalog, guid)
}

#[test]
fn an_upgrade_marks_the_plans_an_import_left_untouched() {
    let (_dir, path) = scratch();
    let mut store = MetaStore::create(&path).unwrap();
    // One listing imports both drafts in the same millisecond.
    let (imported, catalog, guid) = plan(&mut store, "Imported", 7, 7);
    // Made here: the framing and the plan were saved apart.
    let (made, _, _) = plan(&mut store, "Made here", 7, 9);
    // Edited since the import: a draft is past revision 1.
    let (edited, _, _) = plan(&mut store, "Edited", 7, 7);
    let mut again = framing(edited, 8);
    again.target_name = "Heart and Soul".into();
    assert_eq!(store.save_framing_draft(&again, 1).unwrap().revision, 2);
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("DROP TABLE draft_import; PRAGMA user_version=25")
        .unwrap();
    drop(conn);

    let store = MetaStore::open(&path).unwrap();
    assert_eq!(
        store.draft_import(imported).unwrap(),
        Some(DraftImport {
            project_id: imported,
            catalog_id: catalog,
            source_project_guid: guid,
            framing_revision: 1,
            plan_revision: 1,
        })
    );
    assert_eq!(store.draft_import(made).unwrap(), None);
    assert_eq!(store.draft_import(edited).unwrap(), None);
}

#[test]
fn an_import_is_recorded_moved_on_and_forgotten_and_goes_with_its_plan() {
    let (_dir, path) = scratch();
    let mut store = MetaStore::create(&path).unwrap();
    let (project, catalog, guid) = plan(&mut store, "Heart", 3, 3);
    let mut import = DraftImport {
        project_id: project,
        catalog_id: catalog,
        source_project_guid: guid,
        framing_revision: 1,
        plan_revision: 0,
    };
    store.record_draft_import(&import).unwrap();
    assert_eq!(store.draft_import(project).unwrap(), Some(import));
    import.plan_revision = 2;
    store.record_draft_import(&import).unwrap();
    assert_eq!(store.draft_import(project).unwrap(), Some(import));
    store.forget_draft_import(project).unwrap();
    assert_eq!(store.draft_import(project).unwrap(), None);

    // A plan absorbed by another is retired with its record.
    store.record_draft_import(&import).unwrap();
    let (keep, _, _) = plan(&mut store, "Keep", 4, 4);
    store.attach_project(keep, project).unwrap();
    assert!(store.project(project).unwrap().is_none());
    assert_eq!(store.draft_import(project).unwrap(), None);
}
