use psf_guard_director_core::{
    framing::{Mosaic, PanelSize},
    visibility::IcrsPosition,
};
use psf_guard_director_meta::{framing::FramingDraft, Error, MetaStore, Uuid};
use tempfile::TempDir;

fn draft(project: Uuid) -> FramingDraft {
    FramingDraft {
        project_id: project,
        revision: 0,
        target_name: "M31".into(),
        center: IcrsPosition {
            ra_degrees: 10.6847,
            dec_degrees: 41.269,
        },
        position_angle_degrees: 35.0,
        mosaic: Mosaic {
            rows: 2,
            columns: 1,
            overlap_percent: 15,
        },
        panel_rig_id: None,
        panel: Some(PanelSize {
            width_degrees: 2.0,
            height_degrees: 1.5,
        }),
        shown_rig_ids: vec![],
        survey_id: "dss2_color".into(),
        view_fov_degrees: 6.0,
        updated_at_ms: 1_000,
    }
}

#[test]
fn a_draft_is_saved_by_compare_and_set_and_survives_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Andromeda").unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "RedCat").unwrap();
    assert_eq!(store.framing_draft(project.id).unwrap(), None);
    assert!(matches!(
        store.save_framing_draft(&draft(Uuid::new_v4()), 0),
        Err(Error::NotFound)
    ));
    let mut unknown_rig = draft(project.id);
    unknown_rig.panel_rig_id = Some(Uuid::new_v4());
    assert!(matches!(
        store.save_framing_draft(&unknown_rig, 0),
        Err(Error::NotFound)
    ));

    let saved = store.save_framing_draft(&draft(project.id), 0).unwrap();
    assert_eq!(saved.revision, 1);
    assert!(matches!(
        store.save_framing_draft(&draft(project.id), 0),
        Err(Error::Conflict)
    ));
    let mut same = saved.clone();
    same.updated_at_ms = 2_000;
    assert_eq!(store.save_framing_draft(&same, 1).unwrap(), saved);

    let mut turned = saved.clone();
    turned.position_angle_degrees = 40.0;
    turned.panel_rig_id = Some(rig.id);
    turned.shown_rig_ids = vec![rig.id];
    turned.updated_at_ms = 3_000;
    let second = store.save_framing_draft(&turned, 1).unwrap();
    assert_eq!(second.revision, 2);
    drop(store);
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(store.framing_draft(project.id).unwrap(), Some(second));
}

#[test]
fn bad_geometry_and_survey_ids_are_refused() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Andromeda").unwrap();
    let mut bad = draft(project.id);
    bad.mosaic.rows = 0;
    assert!(matches!(
        store.save_framing_draft(&bad, 0),
        Err(Error::InvalidInput)
    ));
    let mut bad = draft(project.id);
    bad.center.dec_degrees = 95.0;
    assert!(matches!(
        store.save_framing_draft(&bad, 0),
        Err(Error::InvalidInput)
    ));
    let mut bad = draft(project.id);
    bad.survey_id = "../dss".into();
    assert!(matches!(
        store.save_framing_draft(&bad, 0),
        Err(Error::InvalidInput)
    ));
    let mut bad = draft(project.id);
    bad.view_fov_degrees = 0.0;
    assert!(matches!(
        store.save_framing_draft(&bad, 0),
        Err(Error::InvalidInput)
    ));
    let mut no_panel = draft(project.id);
    no_panel.panel = None;
    assert_eq!(store.save_framing_draft(&no_panel, 0).unwrap().revision, 1);
}
