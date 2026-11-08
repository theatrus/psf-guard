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

#[test]
fn a_rig_without_a_mode_gets_one_and_it_reads_back() {
    use psf_guard_director_meta::profile::RigMode;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "RedCat").unwrap();
    // A rig without a profile gets one, with the mode set.
    let saved = store
        .default_rig_mode(rig.id, RigMode::Director, 1_000)
        .unwrap();
    assert_eq!((saved.mode, saved.revision), (Some(RigMode::Director), 1));
    // Setting the same mode again changes nothing.
    assert_eq!(
        store
            .default_rig_mode(rig.id, RigMode::Director, 2_000)
            .unwrap()
            .revision,
        1
    );
    let mut edited = saved.clone();
    edited.mode = Some(RigMode::TargetScheduler);
    let edited = store.save_rig_profile(&edited, 1).unwrap();
    assert_eq!(edited.revision, 2);
    drop(store);
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(
        store.rig_profile(rig.id).unwrap().unwrap().mode,
        Some(RigMode::TargetScheduler)
    );
    // A profile written before modes reads with none set.
    let unset = RigProfile::empty(rig.id, 1);
    assert_eq!(unset.mode, None);
    assert!(!serde_json::to_string(&unset).unwrap().contains("mode"));
    assert_eq!("director".parse::<RigMode>().unwrap(), RigMode::Director);
    assert_eq!(RigMode::TargetScheduler.to_string(), "target_scheduler");
}

#[test]
fn pairing_fills_in_a_mode_but_never_replaces_one() {
    use psf_guard_director_meta::profile::RigMode;
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap();
    let mut chosen = RigProfile::empty(rig.id, 100);
    chosen.mode = Some(RigMode::TargetScheduler);
    let saved = store.save_rig_profile(&chosen, 0).unwrap();
    let after = store
        .default_rig_mode(rig.id, RigMode::Director, 1_000)
        .unwrap();
    assert_eq!(after.mode, Some(RigMode::TargetScheduler));
    assert_eq!(after.revision, saved.revision);
}

#[test]
fn a_typed_filter_list_is_checked_and_a_reported_wheel_wins_over_it() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap();
    let typed = |names: &[&str], source: Source| {
        let mut profile = RigProfile::empty(rig.id, 0);
        profile.filters = Some(Reported {
            value: names.iter().map(|name| name.to_string()).collect(),
            source,
            reported_at_ms: 0,
        });
        profile
    };

    for bad in [
        typed(&[], Source::Manual {}),
        typed(&["Ha", "ha"], Source::Manual {}),
        typed(&[" Ha"], Source::Manual {}),
        typed(&[""], Source::Manual {}),
        typed(&[&"x".repeat(65)], Source::Manual {}),
        typed(&["Ha"], Source::Plugin {}),
    ] {
        assert!(matches!(
            store.save_rig_profile(&bad, 0),
            Err(Error::InvalidInput)
        ));
    }
    let saved = store
        .save_rig_profile(&typed(&["L", "Ha", "OIII"], Source::Manual {}), 0)
        .unwrap();
    assert_eq!(
        store.rig_profile(rig.id).unwrap().unwrap().filter_list(),
        Some(vec!["L".to_string(), "Ha".into(), "OIII".into()])
    );
    assert_eq!(RigProfile::empty(rig.id, 0).filter_list(), None);

    // A Director plugin's wheel, by its native labels, replaces the typed list.
    let mut reported = saved.clone();
    reported.configuration = Some(Reported {
        value: serde_json::from_value(serde_json::json!({
            "rig_id": rig.id.to_string(), "id": "c", "camera_id": "cam",
            "filter_wheel_id": "fw",
            "filters": [{"id": "f1", "position": 0}, {"id": "f2", "position": 1}],
            "binning_modes": [{"x": 1, "y": 1}], "readout_modes": [0],
            "gain": {"support": "unsupported"}, "offset": {"support": "unsupported"},
            "exposure_min_ms": 1, "exposure_max_ms": 3_600_000,
            "enable_slew_center": true, "dither_every": 0
        }))
        .unwrap(),
        source: Source::Plugin {},
        reported_at_ms: 1,
    });
    reported.filter_names = [("f1".to_string(), "Lum".to_string())].into();
    assert_eq!(
        reported.filter_list(),
        Some(vec!["Lum".to_string(), "f2".into()])
    );
}
