use psf_guard_director_core::{
    priority::Scope,
    visibility::{Horizon, HorizonPoint, Site},
};
use psf_guard_director_meta::{
    preferences::Settings,
    profile::{Reported, RigProfile, Source},
    site_profile::{Origin, SiteProfile},
    Error, MetaStore, Uuid,
};
use tempfile::TempDir;

fn manual<T>(value: T) -> Reported<T> {
    Reported {
        value,
        source: Source::Manual {},
        reported_at_ms: 1_000,
    }
}

fn place(latitude_degrees: f64) -> Site {
    Site {
        latitude_degrees,
        longitude_degrees: -118.0,
        elevation_meters: 400.0,
    }
}

fn curve(altitude: f64) -> Horizon {
    Horizon::Custom {
        points: vec![
            HorizonPoint {
                azimuth_degrees: 0.0,
                altitude_degrees: altitude,
            },
            HorizonPoint {
                azimuth_degrees: 360.0,
                altitude_degrees: altitude,
            },
        ],
    }
}

#[test]
fn a_site_profile_is_saved_by_compare_and_set_and_survives_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let site = store.create_site(Uuid::new_v4(), "Backyard").unwrap();
    assert_eq!(store.site_profile(site.id).unwrap(), None);
    assert!(matches!(
        store.save_site_profile(&SiteProfile::empty(Uuid::new_v4(), 1), 0),
        Err(Error::NotFound)
    ));

    let mut profile = SiteProfile::empty(site.id, 1_000);
    profile.location = Some(manual(place(34.0)));
    profile.horizon = Some(manual(curve(15.0)));
    let saved = store.save_site_profile(&profile, 0).unwrap();
    assert_eq!(saved.revision, 1);
    assert!(matches!(
        store.save_site_profile(&profile, 0),
        Err(Error::Conflict)
    ));
    // The same body again under the right revision does not bump it.
    let mut again = saved.clone();
    again.updated_at_ms = 2_000;
    assert_eq!(store.save_site_profile(&again, 1).unwrap().revision, 1);

    let mut bad = saved.clone();
    bad.horizon = Some(manual(Horizon::Custom { points: vec![] }));
    assert!(matches!(
        store.save_site_profile(&bad, 1),
        Err(Error::InvalidInput)
    ));
    drop(store);
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(store.site_profile(site.id).unwrap(), Some(saved));
}

#[test]
fn a_rig_takes_its_own_values_first_then_its_planning_site() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "RedCat").unwrap();
    let site = store.create_site(Uuid::new_v4(), "Backyard").unwrap();

    // Nothing anywhere: no location, the flat minimum.
    let resolved = store.rig_site(rig.id, None).unwrap();
    assert_eq!(resolved.site, None);
    assert_eq!(resolved.location, None);
    assert_eq!(resolved.location_from, Origin::None);
    assert_eq!(resolved.horizon, Horizon::FixedMinimum {});
    assert_eq!(resolved.horizon_from, Origin::None);

    let mut profile = SiteProfile::empty(site.id, 1_000);
    profile.location = Some(manual(place(34.0)));
    profile.horizon = Some(manual(curve(15.0)));
    store.save_site_profile(&profile, 0).unwrap();
    // A site profile alone does nothing until the rig names the site.
    assert_eq!(store.rig_site(rig.id, None).unwrap().location, None);

    let mut settings = Settings::empty(Scope::Rig, rig.id);
    settings.site_id = Some(site.id);
    store.save_observing_settings(&settings).unwrap();
    let resolved = store.rig_site(rig.id, None).unwrap();
    assert_eq!(resolved.site.as_ref().map(|s| s.id), Some(site.id));
    assert_eq!(resolved.location, Some(place(34.0)));
    assert_eq!(resolved.location_from, Origin::Site);
    assert_eq!(resolved.horizon, curve(15.0));
    assert_eq!(resolved.horizon_from, Origin::Site);

    // The rig's own horizon wins; with no location of its own the site's stays.
    let mut own = RigProfile::empty(rig.id, 1_000);
    own.horizon = Some(Reported {
        value: curve(25.0),
        source: Source::Plugin {},
        reported_at_ms: 1_000,
    });
    let resolved = store.rig_site(rig.id, Some(&own)).unwrap();
    assert_eq!(resolved.horizon, curve(25.0));
    assert_eq!(resolved.horizon_from, Origin::Rig);
    assert_eq!(resolved.location_from, Origin::Site);
    own.site = Some(manual(place(40.0)));
    let resolved = store.rig_site(rig.id, Some(&own)).unwrap();
    assert_eq!(resolved.location, Some(place(40.0)));
    assert_eq!(resolved.location_from, Origin::Rig);
}
