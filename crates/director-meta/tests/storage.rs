use psf_guard_director_meta::{Error, MetaStore, Uuid};
use rusqlite::Connection;
use tempfile::TempDir;

#[test]
fn ts_and_empty_databases_are_not_adopted_or_changed() {
    let dir = TempDir::new().unwrap();
    for sql in [
        "",
        "CREATE TABLE project (id INTEGER PRIMARY KEY, guid TEXT);",
    ] {
        let path = dir.path().join(format!("{}.sqlite", Uuid::new_v4()));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(sql).unwrap();
        drop(conn);
        let original = std::fs::read(&path).unwrap();
        assert!(matches!(
            MetaStore::open(&path),
            Err(Error::ForeignDatabase)
        ));
        assert!(MetaStore::create(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            conn.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "delete"
        );
    }
}

#[test]
fn future_schema_and_corrupt_instance_are_refused() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    drop(MetaStore::create(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", 14).unwrap();
    assert!(matches!(
        MetaStore::open(&path),
        Err(Error::UnsupportedSchema)
    ));
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        14
    );
    conn.pragma_update(None, "user_version", 13).unwrap();
    conn.execute("UPDATE meta SET instance_id=?1", [Uuid::nil().to_string()])
        .unwrap();
    assert!(matches!(
        MetaStore::open(&path),
        Err(Error::CorruptDatabase)
    ));
}

#[test]
fn backup_contains_committed_wal_and_restore_preserves_identity_without_overwrite() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&source).unwrap();
    let project = store.create_project(Uuid::new_v4(), "M31").unwrap();
    let backup = dir.path().join("backup.sqlite");
    store.backup(&backup).unwrap();
    let conn = Connection::open(&backup).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "delete"
    );
    drop(conn);
    store.rename_project(project.id, 1, "newer").unwrap();
    let original = std::fs::read(&backup).unwrap();
    assert!(store.backup(&backup).is_err());
    assert_eq!(std::fs::read(&backup).unwrap(), original);
    let restored = dir.path().join("restored.sqlite");
    MetaStore::restore(&backup, &restored).unwrap();
    let copy = MetaStore::open(&restored).unwrap();
    assert_eq!(copy.instance_id(), store.instance_id());
    assert_eq!(copy.project(project.id).unwrap(), Some(project.clone()));
    assert!(MetaStore::restore(&backup, &source).is_err());
    assert!(MetaStore::restore(&backup, &backup).is_err());
    assert_eq!(store.project(project.id).unwrap().unwrap().name, "newer");
    assert_eq!(std::fs::read(&backup).unwrap(), original);
}

#[test]
fn foreign_backup_and_invalid_paths_leave_no_destination() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("ts.sqlite");
    Connection::open(&source)
        .unwrap()
        .execute_batch("CREATE TABLE acquiredimage(id INTEGER);")
        .unwrap();
    let destination = dir.path().join("restored.sqlite");
    assert!(matches!(
        MetaStore::restore(&source, &destination),
        Err(Error::ForeignDatabase)
    ));
    assert!(!destination.exists());
    assert!(MetaStore::create(std::path::Path::new(":memory:")).is_err());
    assert!(MetaStore::open(std::path::Path::new(":memory:")).is_err());
    assert!(MetaStore::create(&dir.path().join("absent/meta.sqlite")).is_err());
    assert!(!dir.path().join("absent").exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn incomplete_schema_and_orphaned_backup_are_not_published() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let store = MetaStore::create(&path).unwrap();
    let external = Connection::open(&path).unwrap();
    external.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    external
        .execute(
            "INSERT INTO project_catalog VALUES(?1,?2,?3)",
            [
                Uuid::new_v4().to_string(),
                Uuid::new_v4().to_string(),
                Uuid::new_v4().to_string(),
            ],
        )
        .unwrap();
    let destination = dir.path().join("backup.sqlite");
    assert!(matches!(
        store.backup(&destination),
        Err(Error::CorruptDatabase)
    ));
    assert!(!destination.exists());
    external
        .execute_batch("DROP TABLE project_catalog")
        .unwrap();
    assert!(matches!(
        MetaStore::open(&path),
        Err(Error::CorruptDatabase)
    ));
    assert!(matches!(
        MetaStore::restore(&path, &destination),
        Err(Error::CorruptDatabase)
    ));
    assert!(!destination.exists());
}

#[test]
fn uncommitted_edits_are_absent_from_backup_and_later_commits_are_independent() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let project = store.create_project(Uuid::new_v4(), "committed").unwrap();
    let mut writer = Connection::open(&path).unwrap();
    let tx = writer.transaction().unwrap();
    tx.execute(
        "UPDATE global_project SET name='uncommitted',revision=2",
        [],
    )
    .unwrap();
    let backup = dir.path().join("backup.sqlite");
    store.backup(&backup).unwrap();
    tx.commit().unwrap();
    let copy = MetaStore::open(&backup).unwrap();
    assert_eq!(copy.project(project.id).unwrap(), Some(project.clone()));
    assert_eq!(store.project(project.id).unwrap().unwrap().revision, 2);
}

/// A store left at schema 11 by the previous build (pairing and client
/// tables present, no contact table) must open and gain only what it lacks.
/// Re-running the schema 11 step here failed with "table director_pairing
/// already exists" and kept every existing coordinator from starting.
#[test]
fn schema_eleven_upgrade_adds_contacts_without_recreating_client_tables() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let instance = MetaStore::create(&path).unwrap().instance_id();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("DROP TABLE rig_contact; PRAGMA user_version=11;")
        .unwrap();
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(store.instance_id(), instance);
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        13
    );
    conn.prepare("SELECT rig_id,kind,at_ms,detail FROM rig_contact")
        .unwrap();
    conn.prepare("SELECT token_hash FROM director_pairing")
        .unwrap();
    // Opening again is a no-op at the current version.
    drop(store);
    MetaStore::open(&path).unwrap();
}
