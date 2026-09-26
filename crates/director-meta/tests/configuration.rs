use psf_guard_director_core::{
    program::Program,
    visibility::{Horizon, HorizonPoint, Site},
    windows::MeridianExclusion,
};
use psf_guard_director_meta::{
    configuration::{RigSetup, SiteSnapshot},
    Error, MetaStore, Uuid,
};
use rusqlite::Connection;
use tempfile::TempDir;

fn fixture(store: &mut MetaStore) -> (SiteSnapshot, RigSetup) {
    let site = store.create_site(Uuid::new_v4(), "Site").unwrap();
    let rig = store.create_rig(Uuid::new_v4(), "Rig").unwrap();
    let snapshot = SiteSnapshot {
        id: Uuid::new_v4(),
        site_id: site.id,
        location: Site {
            latitude_degrees: 35.12345678912345,
            longitude_degrees: -120.9876543210123,
            elevation_meters: 1000.0,
        },
        horizon: Horizon::Custom {
            points: vec![
                HorizonPoint {
                    azimuth_degrees: 0.0,
                    altitude_degrees: 10.0,
                },
                HorizonPoint {
                    azimuth_degrees: 90.0,
                    altitude_degrees: 40.0,
                },
                HorizonPoint {
                    azimuth_degrees: 360.0,
                    altitude_degrees: 20.0,
                },
            ],
        },
    };
    let program: Program = serde_json::from_str(include_str!(
        "../../director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    let mut configuration = program.configuration;
    configuration.id = format!("nina-{}", "a".repeat(64));
    configuration.rig_id = rig.id.to_string();
    let setup = RigSetup {
        id: Uuid::new_v4(),
        configuration,
        site_snapshot_id: snapshot.id,
        minimum_altitude_degrees: 15.0,
        maximum_altitude_degrees: 85.0,
        meridian_exclusion: MeridianExclusion {
            before_ms: 3_600_000,
            after_ms: 600_000,
        },
    };
    (snapshot, setup)
}

#[test]
fn complete_snapshots_survive_reopen_and_restore_without_rewriting_history() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let (site, setup) = fixture(&mut store);
    store.register_site_snapshot(&site).unwrap();
    store.register_rig_setup(&setup).unwrap();
    store.register_site_snapshot(&site).unwrap();
    store.register_rig_setup(&setup).unwrap();
    let config_id = setup.id;
    let mut changed = site.clone();
    changed.location.longitude_degrees += 1.0;
    assert!(matches!(
        store.register_site_snapshot(&changed),
        Err(Error::Conflict)
    ));
    changed.id = Uuid::new_v4();
    store.register_site_snapshot(&changed).unwrap();
    let mut moved = setup.clone();
    moved.site_snapshot_id = changed.id;
    assert!(matches!(
        store.register_rig_setup(&moved),
        Err(Error::Conflict)
    ));
    moved.id = Uuid::new_v4();
    store.register_rig_setup(&moved).unwrap();
    store.rename_site(site.site_id, 1, "Renamed site").unwrap();
    let backup = dir.path().join("backup.sqlite");
    store.backup(&backup).unwrap();
    drop(store);
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(store.site_snapshot(site.id).unwrap(), Some(site.clone()));
    assert_eq!(store.rig_setup(config_id).unwrap(), Some(setup.clone()));
    let restored = dir.path().join("restored.sqlite");
    MetaStore::restore(&backup, &restored).unwrap();
    let copy = MetaStore::open(&restored).unwrap();
    assert_eq!(copy.site_snapshot(site.id).unwrap(), Some(site));
    assert_eq!(copy.rig_setup(config_id).unwrap(), Some(setup));
    assert_eq!(copy.rig_setup(moved.id).unwrap(), Some(moved));
}

#[test]
fn changed_horizon_capabilities_and_meridian_policy_need_new_ids() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let (site, setup) = fixture(&mut store);
    store.register_site_snapshot(&site).unwrap();
    store.register_rig_setup(&setup).unwrap();
    let mut changed = site.clone();
    changed.horizon = Horizon::FixedMinimum {};
    assert!(matches!(
        store.register_site_snapshot(&changed),
        Err(Error::Conflict)
    ));
    for mutate in [
        |s: &mut RigSetup| s.meridian_exclusion.after_ms += 1,
        |s: &mut RigSetup| s.minimum_altitude_degrees += 1.0,
        |s: &mut RigSetup| s.configuration.camera_id.push_str("-changed"),
        |s: &mut RigSetup| s.configuration.filters[0].id.push_str("-changed"),
    ] {
        let mut changed = setup.clone();
        mutate(&mut changed);
        assert!(matches!(
            store.register_rig_setup(&changed),
            Err(Error::Conflict)
        ));
    }
}

#[test]
fn shared_core_validation_and_parent_scope_precede_storage() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let (site, setup) = fixture(&mut store);
    assert!(matches!(
        store.register_rig_setup(&setup),
        Err(Error::NotFound)
    ));
    let mut invalid = site.clone();
    invalid.site_id = Uuid::new_v4();
    assert!(matches!(
        store.register_site_snapshot(&invalid),
        Err(Error::NotFound)
    ));
    invalid = site.clone();
    invalid.location.latitude_degrees = f64::NAN;
    assert!(matches!(
        store.register_site_snapshot(&invalid),
        Err(Error::InvalidInput)
    ));
    invalid = site.clone();
    invalid.horizon = Horizon::Custom { points: vec![] };
    assert!(matches!(
        store.register_site_snapshot(&invalid),
        Err(Error::InvalidInput)
    ));
    store.register_site_snapshot(&site).unwrap();
    for mutate in [
        |s: &mut RigSetup| s.configuration.rig_id = Uuid::nil().to_string(),
        |s: &mut RigSetup| s.configuration.id.clear(),
        |s: &mut RigSetup| s.id = Uuid::nil(),
        |s: &mut RigSetup| s.configuration.filters.clear(),
        |s: &mut RigSetup| s.maximum_altitude_degrees = s.minimum_altitude_degrees,
        |s: &mut RigSetup| s.minimum_altitude_degrees = f64::INFINITY,
    ] {
        let mut changed = setup.clone();
        mutate(&mut changed);
        assert!(matches!(
            store.register_rig_setup(&changed),
            Err(Error::InvalidInput)
        ));
    }
    let mut wrong_rig = setup.clone();
    wrong_rig.configuration.rig_id = Uuid::new_v4().to_string();
    assert!(matches!(
        store.register_rig_setup(&wrong_rig),
        Err(Error::NotFound)
    ));
    assert!(store.rig_setup(setup.id).unwrap().is_none());
}

#[test]
fn snapshot_pages_are_scoped_bounded_and_not_an_implicit_latest_pointer() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let (mut site, mut setup) = fixture(&mut store);
    let rig = Uuid::parse_str(&setup.configuration.rig_id).unwrap();
    for n in [3, 1, 2] {
        site.id = Uuid::from_u128(n);
        store.register_site_snapshot(&site).unwrap();
        setup.id = Uuid::from_u128(n);
        setup.site_snapshot_id = site.id;
        store.register_rig_setup(&setup).unwrap();
    }
    let page = store.site_snapshot_ids(site.site_id, None, 2).unwrap();
    assert_eq!(page.ids, vec![Uuid::from_u128(1), Uuid::from_u128(2)]);
    assert_eq!(page.next_after, Some(Uuid::from_u128(2)));
    let last = store
        .site_snapshot_ids(site.site_id, page.next_after, 2)
        .unwrap();
    assert_eq!(last.ids, vec![Uuid::from_u128(3)]);
    assert_eq!(last.next_after, None);
    assert_eq!(store.rig_setup_ids(rig, None, 2).unwrap(), page);
    assert!(store
        .rig_setup_ids(Uuid::new_v4(), None, 2)
        .unwrap()
        .ids
        .is_empty());
    assert!(matches!(
        store.site_snapshot_ids(site.site_id, None, 257),
        Err(Error::InvalidInput)
    ));
    assert!(matches!(
        store.rig_setup_ids(rig, Some(Uuid::nil()), 2),
        Err(Error::InvalidInput)
    ));
    assert_eq!(store.sites(None, 5).unwrap().items.len(), 1);
}

#[test]
fn mismatched_and_oversized_stored_payloads_are_corruption_not_configuration() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let (site, setup) = fixture(&mut store);
    store.register_site_snapshot(&site).unwrap();
    store.register_rig_setup(&setup).unwrap();
    let id = setup.id;
    let external = Connection::open(&path).unwrap();
    let mut changed = setup;
    changed.configuration.rig_id = Uuid::new_v4().to_string();
    external
        .execute(
            "UPDATE rig_setup SET payload=?1",
            [serde_json::to_string(&changed).unwrap()],
        )
        .unwrap();
    assert!(matches!(store.rig_setup(id), Err(Error::CorruptDatabase)));
    external
        .execute(
            "UPDATE site_snapshot SET payload=?1",
            ["x".repeat(psf_guard_director_core::MAX_REQUEST_BYTES + 100)],
        )
        .unwrap();
    assert!(matches!(
        store.site_snapshot(site.id),
        Err(Error::CorruptDatabase)
    ));
}

fn downgrade_to_v1(path: &std::path::Path) {
    Connection::open(path).unwrap().execute_batch("DROP TABLE rig_setup; DROP TABLE site_snapshot; DROP TABLE site; PRAGMA user_version=1;").unwrap();
}

#[test]
fn schema_one_migrates_without_changing_identities_and_old_backups_stay_read_only() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let instance = store.instance_id();
    let project = store.create_project(Uuid::new_v4(), "M31").unwrap();
    drop(store);
    downgrade_to_v1(&path);
    let original = std::fs::read(&path).unwrap();
    let restored = dir.path().join("restored.sqlite");
    MetaStore::restore(&path, &restored).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let mut copy = MetaStore::open(&restored).unwrap();
    assert_eq!(copy.instance_id(), instance);
    assert_eq!(copy.project(project.id).unwrap(), Some(project.clone()));
    let (site, setup) = fixture(&mut copy);
    copy.register_site_snapshot(&site).unwrap();
    copy.register_rig_setup(&setup).unwrap();
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(store.project(project.id).unwrap(), Some(project));
    assert_eq!(
        Connection::open(&path)
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        2
    );
}

#[test]
fn migration_failure_rolls_back_schema_version_and_existing_records() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let project = store.create_project(Uuid::new_v4(), "M31").unwrap();
    drop(store);
    downgrade_to_v1(&path);
    let external = Connection::open(&path).unwrap();
    external
        .execute_batch("CREATE VIEW site_snapshot AS SELECT 1 AS id")
        .unwrap();
    assert!(MetaStore::open(&path).is_err());
    assert_eq!(
        external
            .pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        external
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='site'",
                [],
                |r| r.get::<_, i32>(0)
            )
            .unwrap(),
        0
    );
    external.execute_batch("DROP VIEW site_snapshot").unwrap();
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(store.project(project.id).unwrap(), Some(project));
}

#[test]
fn competing_snapshot_contents_cannot_replace_each_other() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let (site, _) = fixture(&mut store);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = [10.0, 20.0]
        .into_iter()
        .map(|elevation| {
            let path = path.clone();
            let barrier = barrier.clone();
            let mut site = site.clone();
            site.location.elevation_meters = elevation;
            std::thread::spawn(move || {
                let mut store = MetaStore::open(&path).unwrap();
                barrier.wait();
                store.register_site_snapshot(&site)
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
}

#[test]
fn concurrent_openers_migrate_once_and_preserve_identity() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let store = MetaStore::create(&path).unwrap();
    let id = store.instance_id();
    drop(store);
    downgrade_to_v1(&path);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                MetaStore::open(&path).unwrap().instance_id()
            })
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().unwrap(), id);
    }
}

#[test]
fn oversized_snapshot_is_rejected_without_thinning_the_horizon() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let (mut site, _) = fixture(&mut store);
    site.horizon = Horizon::Custom {
        points: (0..4098)
            .map(|n| HorizonPoint {
                azimuth_degrees: f64::from(n) * 360.0 / 4097.0,
                altitude_degrees: 35.12345678912345,
            })
            .collect(),
    };
    site.horizon.validate().unwrap();
    assert!(serde_json::to_vec(&site).unwrap().len() > psf_guard_director_core::MAX_REQUEST_BYTES);
    assert!(matches!(
        store.register_site_snapshot(&site),
        Err(Error::InvalidInput)
    ));
    assert!(store.site_snapshot(site.id).unwrap().is_none());
}
