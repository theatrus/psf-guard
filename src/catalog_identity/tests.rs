use super::*;
use rusqlite::{OpenFlags, TransactionBehavior, MAIN_DB};
use std::{
    sync::{Arc, Barrier},
    time::Duration,
};
use tempfile::TempDir;

fn candidate() -> CatalogIdentity {
    CatalogIdentity {
        id: Uuid::new_v4(),
        origin_instance_id: Uuid::new_v4(),
    }
}

fn commit(
    connection: &mut Connection,
    identity: CatalogIdentity,
) -> Result<CatalogIdentity, Error> {
    let mut tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let result = adopt(&mut tx, identity)?;
    tx.commit()?;
    Ok(result)
}

#[test]
fn ordinary_read_does_not_adopt_and_source_read_only_remains_read_only() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("source.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TABLE project(Id INTEGER PRIMARY KEY, guid TEXT); INSERT INTO project VALUES(1,'source-guid');").unwrap();
    drop(connection);
    let original = std::fs::read(&path).unwrap();
    let mut source = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(read(&source).unwrap(), None);
    let mut tx = source.transaction().unwrap();
    assert!(matches!(adopt(&mut tx, candidate()), Err(Error::Sqlite(_))));
    tx.commit().unwrap();
    drop(source);
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[test]
fn identity_survives_reopen_rename_backup_and_retry_without_touching_ts_history() {
    let dir = TempDir::new().unwrap();
    let original = dir.path().join("original.sqlite");
    let renamed = dir.path().join("renamed.sqlite");
    let backup = dir.path().join("backup.sqlite");
    let mut connection = Connection::open(&original).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA application_id=42; PRAGMA user_version=17;
        CREATE TABLE project(Id INTEGER PRIMARY KEY, guid TEXT, name TEXT);
        INSERT INTO project VALUES(5,'keep-this-guid','Same target');
        CREATE TABLE acquiredimage(Id INTEGER PRIMARY KEY, gradingStatus INTEGER);
        INSERT INTO acquiredimage VALUES(9,2);",
        )
        .unwrap();
    let identity = candidate();
    assert_eq!(commit(&mut connection, identity).unwrap(), identity);
    assert_eq!(commit(&mut connection, identity).unwrap(), identity);
    connection.backup(MAIN_DB, &backup, None).unwrap();
    drop(connection);
    std::fs::rename(&original, &renamed).unwrap();
    for path in [renamed, backup] {
        let mut connection = Connection::open(path).unwrap();
        assert_eq!(read(&connection).unwrap(), Some(identity));
        assert_eq!(commit(&mut connection, identity).unwrap(), identity);
        let project: (i64, String, String) = connection
            .query_row("SELECT * FROM project", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap();
        assert_eq!(project, (5, "keep-this-guid".into(), "Same target".into()));
        assert_eq!(
            connection
                .query_row(
                    "SELECT gradingStatus FROM acquiredimage WHERE Id=9",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        for (pragma, value) in [("PRAGMA application_id", 42), ("PRAGMA user_version", 17)] {
            assert_eq!(
                connection
                    .query_row(pragma, [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                value
            );
        }
    }
}

#[test]
fn caller_rollback_removes_adoption_and_never_mints_an_identity_on_read() {
    let mut connection = Connection::open_in_memory().unwrap();
    let identity = candidate();
    {
        let mut tx = connection.transaction().unwrap();
        assert_eq!(adopt(&mut tx, identity).unwrap(), identity);
        assert_eq!(read(&tx).unwrap(), Some(identity));
    }
    assert_eq!(read(&connection).unwrap(), None);
    assert_eq!(commit(&mut connection, identity).unwrap(), identity);
}

#[test]
fn invalid_proposals_and_conflicting_retries_leave_the_original_unchanged() {
    let mut connection = Connection::open_in_memory().unwrap();
    let identity = candidate();
    for invalid in [
        CatalogIdentity {
            id: Uuid::nil(),
            ..identity
        },
        CatalogIdentity {
            origin_instance_id: Uuid::nil(),
            ..identity
        },
    ] {
        let mut tx = connection.transaction().unwrap();
        assert!(matches!(
            adopt(&mut tx, invalid),
            Err(Error::InvalidIdentity)
        ));
        tx.commit().unwrap();
        assert_eq!(read(&connection).unwrap(), None);
    }
    commit(&mut connection, identity).unwrap();
    for conflict in [
        CatalogIdentity {
            id: Uuid::new_v4(),
            ..identity
        },
        CatalogIdentity {
            origin_instance_id: Uuid::new_v4(),
            ..identity
        },
    ] {
        let mut tx = connection.transaction().unwrap();
        assert!(matches!(adopt(&mut tx, conflict), Err(Error::Conflict)));
        tx.commit().unwrap();
        assert_eq!(read(&connection).unwrap(), Some(identity));
    }
}

#[test]
fn malformed_or_future_records_are_never_repaired_or_reassigned() {
    let identity = candidate();
    for mutation in [
        "DELETE FROM psf_guard_catalog_identity",
        "UPDATE psf_guard_catalog_identity SET schema_version=2",
        "UPDATE psf_guard_catalog_identity SET catalog_id='00000000-0000-0000-0000-000000000000'",
        "UPDATE psf_guard_catalog_identity SET origin_instance_id='not-a-uuid'",
        "UPDATE psf_guard_catalog_identity SET catalog_id=replace(catalog_id,'-','')",
    ] {
        let mut connection = Connection::open_in_memory().unwrap();
        commit(&mut connection, identity).unwrap();
        connection.execute_batch(mutation).unwrap();
        let changes = connection.total_changes();
        assert!(read(&connection).is_err(), "{mutation}");
        let mut tx = connection.transaction().unwrap();
        assert!(adopt(&mut tx, identity).is_err(), "{mutation}");
        tx.commit().unwrap();
        assert_eq!(connection.total_changes(), changes);
    }
    for sql in [
        "CREATE VIEW psf_guard_catalog_identity AS SELECT 1",
        "CREATE TABLE psf_guard_catalog_identity(unrelated TEXT)",
        "CREATE TABLE psf_guard_catalog_identity(singleton,format,schema_version,catalog_id,origin_instance_id);
         INSERT INTO psf_guard_catalog_identity VALUES(1,'psf-guard-catalog',NULL,NULL,NULL)",
    ] {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(sql).unwrap();
        let mut tx = connection.transaction().unwrap();
        assert!(adopt(&mut tx, identity).is_err());
        tx.commit().unwrap();
    }
}

#[test]
fn duplicate_rows_and_temporary_shadow_tables_cannot_supply_identity() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TEMP TABLE psf_guard_catalog_identity(fake TEXT)")
        .unwrap();
    assert_eq!(read(&connection).unwrap(), None);
    let identity = candidate();
    commit(&mut connection, identity).unwrap();
    assert_eq!(read(&connection).unwrap(), Some(identity));
    connection
        .execute_batch(
            "ALTER TABLE main.psf_guard_catalog_identity RENAME TO old_identity;
        CREATE TABLE main.psf_guard_catalog_identity AS SELECT * FROM old_identity;
        INSERT INTO main.psf_guard_catalog_identity SELECT * FROM old_identity;",
        )
        .unwrap();
    assert!(matches!(read(&connection), Err(Error::InvalidRecord)));
    assert!(commit(&mut connection, identity).is_err());
}

#[test]
fn competing_adopters_get_one_durable_identity_not_last_writer_wins() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("catalog.sqlite");
    drop(Connection::open(&path).unwrap());
    let barrier = Arc::new(Barrier::new(2));
    let candidates = [candidate(), candidate()];
    let handles: Vec<_> = candidates
        .into_iter()
        .map(|identity| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut connection = Connection::open(path).unwrap();
                connection.busy_timeout(Duration::from_secs(2)).unwrap();
                barrier.wait();
                commit(&mut connection, identity)
            })
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::Conflict)))
            .count(),
        1
    );
    let connection = Connection::open(path).unwrap();
    assert_eq!(
        read(&connection).unwrap(),
        results.into_iter().find_map(Result::ok)
    );
}

#[test]
fn busy_writer_does_not_adopt_and_retry_keeps_the_confirmed_candidate() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("catalog.sqlite");
    let mut writer = Connection::open(&path).unwrap();
    let lock = writer
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let mut other = Connection::open(&path).unwrap();
    other.busy_timeout(Duration::ZERO).unwrap();
    let identity = candidate();
    assert!(
        matches!(commit(&mut other, identity), Err(Error::Sqlite(error)) if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy))
    );
    lock.rollback().unwrap();
    assert_eq!(read(&other).unwrap(), None);
    assert_eq!(commit(&mut other, identity).unwrap(), identity);
}

#[test]
fn discovery_transaction_keeps_its_snapshot_across_concurrent_adoption() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("catalog.sqlite");
    let mut writer = Connection::open(&path).unwrap();
    writer
        .execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE project(Id INTEGER);")
        .unwrap();
    let mut reader = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let snapshot = reader.transaction().unwrap();
    assert_eq!(read(&snapshot).unwrap(), None);
    let identity = candidate();
    commit(&mut writer, identity).unwrap();
    assert_eq!(read(&snapshot).unwrap(), None);
    snapshot.commit().unwrap();
    assert_eq!(read(&reader).unwrap(), Some(identity));
}
