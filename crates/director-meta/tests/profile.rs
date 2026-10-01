use psf_guard_director_core::{
    optics::{Optics, Rotation},
    program::Program,
    visibility::Site,
};
use psf_guard_director_meta::{
    profile::{Reported, RigProfile, Source},
    Error, MetaStore, Uuid,
};
use tempfile::TempDir;

fn optics() -> Optics {
    Optics {
        sensor_width_px: 6248,
        sensor_height_px: 4176,
        pixel_size_um: 3.76,
        focal_length_mm: 250.0,
        aperture_mm: Some(51.0),
        rotation: Rotation::Manual { angle_degrees: 0.0 },
    }
}

#[test]
fn a_profile_is_saved_by_compare_and_set_and_survives_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "RedCat").unwrap();
    assert_eq!(store.rig_profile(rig.id).unwrap(), None);
    assert!(matches!(
        store.save_rig_profile(&RigProfile::empty(Uuid::new_v4(), 1), 0),
        Err(Error::NotFound)
    ));

    let mut profile = RigProfile::empty(rig.id, 1_000);
    profile.optics = Some(Reported {
        value: optics(),
        source: Source::FrameHeaders {
            file_name: "M31_L_001.fits".into(),
        },
        reported_at_ms: 1_000,
    });
    let saved = store.save_rig_profile(&profile, 0).unwrap();
    assert_eq!(saved.revision, 1);
    // A stale revision is refused; the same body under the right one is a no-op.
    assert!(matches!(
        store.save_rig_profile(&profile, 0),
        Err(Error::Conflict)
    ));
    let mut retried = saved.clone();
    retried.updated_at_ms = 2_000;
    assert_eq!(store.save_rig_profile(&retried, 1).unwrap(), saved);

    let mut changed = saved.clone();
    changed.site = Some(Reported {
        value: Site {
            latitude_degrees: 34.2,
            longitude_degrees: -118.3,
            elevation_meters: 400.0,
        },
        source: Source::Manual {},
        reported_at_ms: 3_000,
    });
    changed.updated_at_ms = 3_000;
    let second = store.save_rig_profile(&changed, 1).unwrap();
    assert_eq!(second.revision, 2);
    drop(store);
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(store.rig_profile(rig.id).unwrap(), Some(second));
}

#[test]
fn invalid_parts_are_refused_before_anything_is_written() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap();
    let base = RigProfile::empty(rig.id, 0);

    let mut bad = base.clone();
    bad.optics = Some(Reported {
        value: Optics {
            focal_length_mm: 0.0,
            ..optics()
        },
        source: Source::Manual {},
        reported_at_ms: 0,
    });
    assert!(matches!(
        store.save_rig_profile(&bad, 0),
        Err(Error::InvalidInput)
    ));

    let mut bad = base.clone();
    bad.limits.value.minimum_altitude_degrees = 95.0;
    assert!(matches!(
        store.save_rig_profile(&bad, 0),
        Err(Error::InvalidInput)
    ));

    // Only the plugin may claim a configuration, and it must name this rig.
    let program: Program = serde_json::from_str(include_str!(
        "../../director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    let mut configuration = program.configuration;
    configuration.rig_id = rig.id.to_string();
    let mut bad = base.clone();
    bad.configuration = Some(Reported {
        value: configuration.clone(),
        source: Source::Manual {},
        reported_at_ms: 0,
    });
    assert!(matches!(
        store.save_rig_profile(&bad, 0),
        Err(Error::InvalidInput)
    ));
    let mut good = base.clone();
    good.configuration = Some(Reported {
        value: configuration,
        source: Source::Plugin {},
        reported_at_ms: 0,
    });
    assert_eq!(store.save_rig_profile(&good, 0).unwrap().revision, 1);
    assert_eq!(store.rig_profile(rig.id).unwrap().unwrap().revision, 1);
    for names in [
        [("unknown".into(), "Ha".into())].into(),
        [("ha".into(), " ".into())].into(),
        [("ha".into(), "Ha\n".into())].into(),
        [("ha".into(), "Ha".into()), ("extra".into(), "Red".into())].into(),
    ] {
        good.filter_names = names;
        assert!(matches!(
            store.save_rig_profile(&good, 1),
            Err(Error::InvalidInput)
        ));
        assert!(store
            .rig_profile(rig.id)
            .unwrap()
            .unwrap()
            .filter_names
            .is_empty());
    }
    good.filter_names = [("ha".into(), "H-alpha".into())].into();
    assert_eq!(
        store.save_rig_profile(&good, 1).unwrap().filter_names["ha"],
        "H-alpha"
    );
}
