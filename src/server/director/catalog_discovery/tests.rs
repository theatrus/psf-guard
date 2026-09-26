use super::*;
use rusqlite::params;
use tempfile::TempDir;

fn fixture() -> (TempDir, std::path::PathBuf, Connection) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("catalog.sqlite");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE project(Id INTEGER PRIMARY KEY, name TEXT, guid TEXT, profileId TEXT)",
    )
    .unwrap();
    (dir, path, conn)
}

#[test]
fn discovers_multiple_profiles_without_merging_project_names_or_reading_images() {
    let (_dir, path, conn) = fixture();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    conn.execute(
        "INSERT INTO project VALUES(1,'M31',?1,'profile-a'),(2,'M31',?2,'profile-b')",
        params![a.to_string(), b.to_string()],
    )
    .unwrap();
    let before = std::fs::read(&path).unwrap();
    let result = read_catalog(&path).unwrap();
    assert_eq!(result.projects.len(), 2);
    assert_eq!(result.profiles.len(), 2);
    assert_eq!(result.projects[0].source_project_guid, Some(a));
    assert_eq!(result.projects[1].source_project_guid, Some(b));
    assert!(result.projects.iter().all(|p| p.issues.is_empty()));
    assert_eq!(result.profiles[0].source_profile_id, "profile-a");
    assert_eq!(result.profiles[0].project_count, 1);
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn reports_legacy_missing_columns_and_invalid_values_without_inventing_identity() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE project(Id INTEGER PRIMARY KEY,name TEXT); INSERT INTO project VALUES(1,'Legacy')").unwrap();
    let result = read_evidence(&conn).unwrap();
    assert!(!result.has_project_guid);
    assert!(!result.has_profile_id);
    assert!(result.profiles.is_empty());
    assert_eq!(
        result.projects[0].issues,
        vec![Issue::MissingProjectGuid, Issue::MissingProfileId]
    );
    let (_dir, path, conn) = fixture();
    conn.execute(
        "INSERT INTO project VALUES(1,?1,?2,?3),(2,NULL,NULL,NULL)",
        params![
            "x".repeat(MAX_TEXT_BYTES + 1),
            Uuid::nil().to_string(),
            vec![0u8, 1u8]
        ],
    )
    .unwrap();
    let result = read_catalog(&path).unwrap();
    assert_eq!(
        result.projects[0].issues,
        vec![
            Issue::InvalidProjectName,
            Issue::InvalidProjectGuid,
            Issue::InvalidProfileId
        ]
    );
    assert!(result.projects[0].name.is_none());
    assert!(result.projects[0].source_project_guid.is_none());
    assert!(result.projects[0].source_profile_id.is_none());
    assert_eq!(
        result.projects[1].issues,
        vec![
            Issue::InvalidProjectName,
            Issue::MissingProjectGuid,
            Issue::MissingProfileId
        ]
    );
}

#[test]
fn marks_every_duplicate_guid_even_across_profiles_and_uuid_spellings() {
    let (_dir, path, conn) = fixture();
    let guid = Uuid::new_v4();
    conn.execute(
        "INSERT INTO project VALUES(1,'A',?1,'one'),(2,'B',?2,'two'),(3,'C','not-a-guid','one')",
        params![guid.to_string(), guid.simple().to_string().to_uppercase()],
    )
    .unwrap();
    let result = read_catalog(&path).unwrap();
    assert_eq!(result.projects[0].issues, vec![Issue::DuplicateProjectGuid]);
    assert_eq!(result.projects[1].issues, vec![Issue::DuplicateProjectGuid]);
    assert_eq!(result.projects[2].issues, vec![Issue::InvalidProjectGuid]);
    assert_eq!(result.profiles[0].project_count, 2);
}

#[test]
fn discovery_is_bounded_and_never_returns_partial_catalogs() {
    let (_dir, path, conn) = fixture();
    conn.execute("WITH RECURSIVE ids(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM ids WHERE x<?1) INSERT INTO project(Id,name) SELECT x,'Project' FROM ids", [MAX_PROJECTS as i64]).unwrap();
    assert_eq!(read_catalog(&path).unwrap().projects.len(), MAX_PROJECTS);
    conn.execute(
        "INSERT INTO project(Id,name) VALUES(?1,'Overflow')",
        [(MAX_PROJECTS + 1) as i64],
    )
    .unwrap();
    assert!(matches!(
        read_catalog(&path),
        Err(DiscoveryError::TooManyProjects)
    ));
}

#[test]
fn fresh_read_sees_committed_wal_but_not_uncommitted_changes() {
    let (_dir, path, conn) = fixture();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch("INSERT INTO project(Id,name) VALUES(1,'Committed'); BEGIN IMMEDIATE; INSERT INTO project(Id,name) VALUES(2,'Uncommitted')").unwrap();
    assert_eq!(read_catalog(&path).unwrap().projects.len(), 1);
    conn.execute_batch("COMMIT").unwrap();
    assert_eq!(read_catalog(&path).unwrap().projects.len(), 2);
}

#[test]
fn unsupported_and_missing_files_are_not_created_or_migrated() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("missing.sqlite");
    assert!(read_catalog(&path).is_err());
    assert!(!path.exists());
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE unrelated(x TEXT)")
        .unwrap();
    assert!(matches!(
        read_catalog(&path),
        Err(DiscoveryError::UnsupportedSchema)
    ));
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn views_and_noninteger_source_rows_are_not_supported_catalogs() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE VIEW project AS SELECT 1 AS Id, 'View' AS name")
        .unwrap();
    assert!(matches!(
        read_evidence(&conn),
        Err(DiscoveryError::UnsupportedSchema)
    ));
    conn.execute_batch("DROP VIEW project; CREATE TABLE project(Id TEXT,name TEXT); INSERT INTO project VALUES('row','Name')").unwrap();
    assert!(matches!(
        read_evidence(&conn),
        Err(DiscoveryError::UnsupportedSchema)
    ));
}

#[test]
fn sqlite_contention_is_retryable_not_invalid_schema() {
    let (_dir, path, conn) = fixture();
    conn.execute_batch("BEGIN EXCLUSIVE").unwrap();
    assert!(matches!(
        read_catalog(&path),
        Err(DiscoveryError::Api(Error::Busy))
    ));
}
