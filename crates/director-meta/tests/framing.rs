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
        rig_framings: vec![],
        layout_revision: 0,
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
    for survey in ["", " dss2_color", "dss2\ncolor", &"x".repeat(257)] {
        let mut bad = draft(project.id);
        bad.survey_id = survey.into();
        assert!(matches!(
            store.save_framing_draft(&bad, 0),
            Err(Error::InvalidInput)
        ));
    }
    let mut bad = draft(project.id);
    bad.view_fov_degrees = 0.0;
    assert!(matches!(
        store.save_framing_draft(&bad, 0),
        Err(Error::InvalidInput)
    ));
    let mut no_panel = draft(project.id);
    no_panel.panel = None;
    assert_eq!(store.save_framing_draft(&no_panel, 0).unwrap().revision, 1);
    // An offline N.I.N.A. map is `nina:` and its folder name, which is
    // longer than the online ids and has a colon.
    let mut offline = draft(project.id);
    offline.survey_id =
        "nina:FramingAssistantCache_NorthernSkyNarrowbandSurvey_OHS_withStars".into();
    assert_eq!(
        store.save_framing_draft(&offline, 1).unwrap().survey_id,
        offline.survey_id
    );
}

#[test]
fn rigs_framed_on_their_own_are_kept_checked_and_laid_out_over_the_shared_target() {
    use psf_guard_director_meta::framing::{RigFraming, RigLayout};
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let project = store.create_project(Uuid::new_v4(), "M31").unwrap().id;
    let wide = store.create_rig(Uuid::new_v4(), "RedCat").unwrap().id;
    let long = store.create_rig(Uuid::new_v4(), "C925").unwrap().id;
    let mut own = draft(project);
    own.rig_framings = vec![RigFraming {
        rig_id: long,
        center: None,
        position_angle_degrees: Some(90.0),
        mosaic: Mosaic {
            rows: 1,
            columns: 3,
            overlap_percent: 10,
        },
        panel: None,
    }];
    let saved = store.save_framing_draft(&own, 0).unwrap();
    assert_eq!(saved.rig_framings.len(), 1);
    // The shared framing for a rig not listed; the rig's own for the one that is,
    // its field standing in for the size, and nothing when neither is known.
    let field = PanelSize {
        width_degrees: 0.5,
        height_degrees: 0.4,
    };
    assert_eq!(
        saved.layout_for(wide, Some(field)),
        Some(RigLayout {
            center: own.center,
            position_angle_degrees: 35.0,
            panel: own.panel.unwrap(),
            mosaic: own.mosaic,
            own: false
        })
    );
    assert_eq!(
        saved.layout_for(long, Some(field)),
        Some(RigLayout {
            center: own.center,
            position_angle_degrees: 90.0,
            panel: field,
            mosaic: Mosaic {
                rows: 1,
                columns: 3,
                overlap_percent: 10
            },
            own: true
        })
    );
    // A rig's own center, once set, replaces the shared one for that rig alone.
    let mut moved = saved.clone();
    moved.rig_framings[0].center = Some(IcrsPosition {
        ra_degrees: 12.0,
        dec_degrees: 40.0,
    });
    assert_eq!(
        moved.layout_for(long, Some(field)).unwrap().center,
        IcrsPosition {
            ra_degrees: 12.0,
            dec_degrees: 40.0
        }
    );
    assert_eq!(
        moved.layout_for(wide, Some(field)).unwrap().center,
        own.center
    );
    assert_eq!(saved.layout_for(long, None), None);
    let mut sized = saved.clone();
    sized.rig_framings[0].panel = Some(PanelSize {
        width_degrees: 1.0,
        height_degrees: 1.0,
    });
    sized.rig_framings[0].position_angle_degrees = None;
    let layout = sized.layout_for(long, None).unwrap();
    assert_eq!(
        (layout.position_angle_degrees, layout.panel.width_degrees),
        (35.0, 1.0)
    );
    // An unknown rig, a rig listed twice, and a bad grid are refused.
    let mut stranger = saved.clone();
    stranger.rig_framings[0].rig_id = Uuid::new_v4();
    assert!(matches!(
        store.save_framing_draft(&stranger, 1),
        Err(Error::NotFound)
    ));
    let mut twice = saved.clone();
    twice.rig_framings.push(twice.rig_framings[0].clone());
    assert!(matches!(
        store.save_framing_draft(&twice, 1),
        Err(Error::InvalidInput)
    ));
    let mut bad = saved.clone();
    bad.rig_framings[0].mosaic.rows = 0;
    assert!(matches!(
        store.save_framing_draft(&bad, 1),
        Err(Error::InvalidInput)
    ));
    // A draft saved before rigs could be framed on their own still reads.
    let stored = store.framing_draft(project).unwrap().unwrap();
    assert_eq!(stored.rig_framings.len(), 1);
}

#[test]
fn only_a_layout_change_moves_the_layout_revision() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Medusa").unwrap();
    let first = store.save_framing_draft(&draft(project.id), 0).unwrap();
    assert_eq!((first.revision, first.layout_revision), (1, 1));
    // The view's survey, width and compared rigs are the view's own.
    let mut view = first.clone();
    view.survey_id = "nina:FramingAssistantCache".into();
    view.view_fov_degrees = 3.0;
    let viewed = store.save_framing_draft(&view, 1).unwrap();
    assert_eq!((viewed.revision, viewed.layout_revision), (2, 1));
    // Turning the camera changes what activation writes.
    let mut turned = viewed.clone();
    turned.position_angle_degrees = 90.0;
    let turned = store.save_framing_draft(&turned, 2).unwrap();
    assert_eq!((turned.revision, turned.layout_revision), (3, 3));
    // A client never sets it: what it sends is ignored.
    let mut sent = turned.clone();
    sent.layout_revision = 0;
    sent.survey_id = "dss2_color".into();
    let resaved = store.save_framing_draft(&sent, 3).unwrap();
    assert_eq!((resaved.revision, resaved.layout_revision), (4, 3));
    assert_eq!(
        store
            .framing_draft(project.id)
            .unwrap()
            .unwrap()
            .layout_revision,
        3
    );
}
