use psf_guard_director_core::{
    program::Program,
    project::{Contribution, Objective, Project, PROJECT_VERSION},
    visibility::{Horizon, Site},
    windows::MeridianExclusion,
};
use psf_guard_director_meta::{
    configuration::{RigSetup, SiteSnapshot},
    Error, MetaStore, Uuid,
};
use rusqlite::{params, Connection};
use tempfile::TempDir;

fn fixture(store: &mut MetaStore) -> Project {
    let owner = store.create_project(Uuid::new_v4(), "M42").unwrap();
    let program: Program = serde_json::from_str(include_str!(
        "../../director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    let mut project = Project {
        schema_version: PROJECT_VERSION,
        project_id: owner.id.to_string(),
        id: Uuid::new_v4().to_string(),
        objectives: vec![],
        contributions: vec![],
    };
    for purpose in ["faint_detail", "unsaturated_stars"] {
        project.objectives.push(Objective {
            id: purpose.into(),
            bandpass_id: "hydrogen-alpha".into(),
            purpose: purpose.into(),
            target: program.targets[0].clone(),
            priority: 1,
        });
    }
    for n in 0..2 {
        let rig = store.create_rig(Uuid::new_v4(), "Same rig name").unwrap();
        let site = store.create_site(Uuid::new_v4(), "Same site name").unwrap();
        let snapshot = SiteSnapshot {
            id: Uuid::new_v4(),
            site_id: site.id,
            location: Site {
                latitude_degrees: 35.0,
                longitude_degrees: -120.0 + f64::from(n),
                elevation_meters: 1000.0,
            },
            horizon: Horizon::FixedMinimum {},
        };
        store.register_site_snapshot(&snapshot).unwrap();
        let mut configuration = program.configuration.clone();
        configuration.rig_id = rig.id.to_string();
        configuration.id = format!("nina-{n}");
        let setup = RigSetup {
            id: Uuid::new_v4(),
            configuration,
            site_snapshot_id: snapshot.id,
            minimum_altitude_degrees: 20.0,
            maximum_altitude_degrees: 85.0,
            meridian_exclusion: MeridianExclusion {
                before_ms: 1000,
                after_ms: 0,
            },
        };
        store.register_rig_setup(&setup).unwrap();
        for (purpose, exposure) in [("faint_detail", 300_000), ("unsaturated_stars", 1000)] {
            let mut framing = program.targets[0].clone();
            framing.id = format!("panel-{n}");
            framing.icrs_ra_mas += n as u32 * 1000;
            let mut recipe = program.recipes[0].clone();
            recipe.id = purpose.into();
            recipe.exposure_ms = exposure;
            project.contributions.push(Contribution {
                id: format!("{n}-{purpose}"),
                objective_id: purpose.into(),
                rig_id: rig.id.to_string(),
                setup_id: setup.id.to_string(),
                configuration_id: setup.configuration.id.clone(),
                framing,
                recipe,
                required_accepted_frames: 20,
            });
        }
    }
    project
}

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap()
}

#[test]
fn immutable_multi_rig_intent_survives_rename_reopen_and_snapshot_restore() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let project = fixture(&mut store);
    store.register_project_intent(&project).unwrap();
    store.register_project_intent(&project).unwrap();
    store
        .rename_project(uuid(&project.project_id), 1, "Renamed")
        .unwrap();
    let backup = dir.path().join("backup.sqlite");
    store.backup(&backup).unwrap();
    let instance = store.instance_id();
    drop(store);
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(
        store.project_intent(uuid(&project.id)).unwrap(),
        Some(project.clone())
    );
    drop(store);
    let restore = dir.path().join("restore.sqlite");
    MetaStore::restore(&backup, &restore).unwrap();
    let copy = MetaStore::open(&restore).unwrap();
    assert_eq!(copy.instance_id(), instance);
    assert_eq!(
        copy.project_intent(uuid(&project.id)).unwrap(),
        Some(project)
    );
}

#[test]
fn changed_intent_requires_new_id_and_pages_keep_explicit_project_scope() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let project = fixture(&mut store);
    store.register_project_intent(&project).unwrap();
    let mut changed = project.clone();
    changed.contributions[0].required_accepted_frames += 1;
    assert!(matches!(
        store.register_project_intent(&changed),
        Err(Error::Conflict)
    ));
    for n in [3, 1, 2] {
        changed.id = Uuid::from_u128(n).to_string();
        store.register_project_intent(&changed).unwrap();
    }
    let owner = uuid(&project.project_id);
    let first = store.project_intent_ids(owner, None, 2).unwrap();
    assert_eq!(first.ids, vec![Uuid::from_u128(1), Uuid::from_u128(2)]);
    assert_eq!(first.next_after, Some(Uuid::from_u128(2)));
    let last = store
        .project_intent_ids(owner, first.next_after, 2)
        .unwrap();
    assert_eq!(last.ids, vec![Uuid::from_u128(3), uuid(&project.id)]);
    assert_eq!(last.next_after, None);
    assert!(store
        .project_intent_ids(Uuid::new_v4(), None, 2)
        .unwrap()
        .ids
        .is_empty());
    for limit in [0, 257] {
        assert!(store.project_intent_ids(owner, None, limit).is_err());
    }
}

#[test]
fn missing_parents_wrong_rigs_and_unsupported_recipes_never_enter_storage() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let project = fixture(&mut store);
    for fault in 0..8 {
        let mut invalid = project.clone();
        match fault {
            0 => invalid.project_id = Uuid::new_v4().to_string(),
            1 => invalid.contributions[0].setup_id = Uuid::new_v4().to_string(),
            2 => invalid.contributions[0].rig_id = Uuid::new_v4().to_string(),
            3 => invalid.contributions[0].configuration_id = "changed".into(),
            4 => invalid.contributions[0].recipe.exposure_ms = 999_999,
            5 => invalid.contributions[0].recipe.filter_id = "display-name".into(),
            6 => invalid.id = Uuid::nil().to_string(),
            _ => invalid.contributions[0].rig_id = "not-a-uuid".into(),
        }
        assert!(
            store.register_project_intent(&invalid).is_err(),
            "fault {fault}"
        );
        assert!(store.project_intent(uuid(&project.id)).unwrap().is_none());
    }
    store.register_project_intent(&project).unwrap();
}

#[test]
fn reference_insert_failure_rolls_back_the_whole_snapshot() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let project = fixture(&mut store);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_link BEFORE INSERT ON project_intent_setup BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
    assert!(store.register_project_intent(&project).is_err());
    assert!(store.project_intent(uuid(&project.id)).unwrap().is_none());
    assert_eq!(
        conn.query_row("SELECT count(*) FROM project_intent_setup", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    conn.execute_batch("DROP TRIGGER fail_link").unwrap();
    store.register_project_intent(&project).unwrap();
}

#[test]
fn competing_writers_cannot_replace_snapshot_content() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let project = fixture(&mut store);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = [20, 30]
        .into_iter()
        .map(|frames| {
            let path = path.clone();
            let barrier = barrier.clone();
            let mut project = project.clone();
            project.contributions[0].required_accepted_frames = frames;
            std::thread::spawn(move || {
                let mut store = MetaStore::open(&path).unwrap();
                barrier.wait();
                store.register_project_intent(&project)
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
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
fn corrupt_payloads_and_reference_inventory_are_not_returned_as_valid_intent() {
    for fault in 0..5 {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("meta.sqlite");
        let mut store = MetaStore::create(&path).unwrap();
        let project = fixture(&mut store);
        store.register_project_intent(&project).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "foreign_keys", false).unwrap();
        match fault {
            0 => {
                conn.execute(
                    "UPDATE project_intent SET payload=?1",
                    ["x".repeat(psf_guard_director_core::MAX_REQUEST_BYTES + 1)],
                )
                .unwrap();
            }
            1 => {
                conn.execute(
                    "UPDATE project_intent SET project_id=?1",
                    [Uuid::new_v4().to_string()],
                )
                .unwrap();
            }
            2 => {
                conn.execute(
                    "DELETE FROM project_intent_setup WHERE setup_id=?1",
                    [&project.contributions[0].setup_id],
                )
                .unwrap();
            }
            3 => {
                conn.execute(
                    "DELETE FROM rig_setup WHERE id=?1",
                    [&project.contributions[0].setup_id],
                )
                .unwrap();
            }
            _ => {
                let mut changed = project.clone();
                changed.contributions[0].recipe.gain = Some(999);
                conn.execute(
                    "UPDATE project_intent SET payload=?1",
                    [serde_json::to_string(&changed).unwrap()],
                )
                .unwrap();
            }
        }
        assert!(
            matches!(
                store.project_intent(uuid(&project.id)),
                Err(Error::CorruptDatabase)
            ),
            "fault {fault}"
        );
    }
}

fn downgrade_to_v2(path: &std::path::Path) {
    Connection::open(path)
        .unwrap()
        .execute_batch(
            "DROP TABLE project_intent_setup; DROP TABLE project_intent; PRAGMA user_version=2;",
        )
        .unwrap();
}

#[test]
fn schema_two_upgrade_preserves_identity_and_configurations_with_transactional_failure() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let project = fixture(&mut store);
    let instance = store.instance_id();
    drop(store);
    downgrade_to_v2(&path);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE VIEW project_intent_setup AS SELECT 1 AS id")
        .unwrap();
    assert!(MetaStore::open(&path).is_err());
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='project_intent'",
            [],
            |r| r.get::<_, i32>(0)
        )
        .unwrap(),
        0
    );
    conn.execute_batch("DROP VIEW project_intent_setup")
        .unwrap();
    let mut store = MetaStore::open(&path).unwrap();
    assert_eq!(store.instance_id(), instance);
    store.register_project_intent(&project).unwrap();
    assert_eq!(
        store.project_intent(uuid(&project.id)).unwrap(),
        Some(project)
    );
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        3
    );
}

#[test]
fn foreign_key_snapshot_references_protect_backup_publication() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let project = fixture(&mut store);
    store.register_project_intent(&project).unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    assert!(conn
        .execute(
            "DELETE FROM rig_setup WHERE id=?1",
            [&project.contributions[0].setup_id]
        )
        .is_err());
    conn.pragma_update(None, "foreign_keys", false).unwrap();
    conn.execute(
        "DELETE FROM rig_setup WHERE id=?1",
        params![project.contributions[0].setup_id],
    )
    .unwrap();
    let destination = dir.path().join("bad-backup.sqlite");
    assert!(matches!(
        store.backup(&destination),
        Err(Error::CorruptDatabase)
    ));
    assert!(!destination.exists());
}

#[test]
fn old_schema_two_backup_is_read_only_and_restored_configurations_bind_new_intent() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let project = fixture(&mut store);
    let instance = store.instance_id();
    drop(store);
    downgrade_to_v2(&path);
    let original = std::fs::read(&path).unwrap();
    let restored = dir.path().join("restored.sqlite");
    MetaStore::restore(&path, &restored).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let mut copy = MetaStore::open(&restored).unwrap();
    assert_eq!(copy.instance_id(), instance);
    copy.register_project_intent(&project).unwrap();
    assert_eq!(
        copy.project_intent(uuid(&project.id)).unwrap(),
        Some(project)
    );
}
