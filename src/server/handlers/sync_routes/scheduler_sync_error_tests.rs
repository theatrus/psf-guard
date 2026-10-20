use super::*;
use rusqlite::ffi::{Error as FfiError, SQLITE_BUSY, SQLITE_LOCKED};

#[test]
fn sync_database_failures_keep_both_catalog_paths() {
    for code in [SQLITE_BUSY, SQLITE_LOCKED] {
        let error = anyhow::Error::new(rusqlite::Error::SqliteFailure(FfiError::new(code), None))
            .context("applying scheduler rows")
            .context("sync operation");

        match classify_scheduler_sync_failure(
            &error,
            "source-scheduler.sqlite",
            "destination-scheduler.sqlite",
        ) {
            AppError::DatabaseError(message) => {
                assert!(message.contains("source-scheduler.sqlite"));
                assert!(message.contains("destination-scheduler.sqlite"));
                assert!(message.contains("busy or locked"));
            }
            other => panic!("contention was misclassified: {other:?}"),
        }
    }

    match classify_scheduler_sync_failure(
        &anyhow::anyhow!("disk read failed"),
        "source-scheduler.sqlite",
        "destination-scheduler.sqlite",
    ) {
        AppError::BadRequest(message) => {
            assert!(message.contains("source-scheduler.sqlite"));
            assert!(message.contains("destination-scheduler.sqlite"));
            assert!(message.contains("disk read failed"));
        }
        other => panic!("non-contention sync failure was misclassified: {other:?}"),
    }

    let io_error = anyhow::Error::new(rusqlite::Error::SqliteFailure(
        FfiError::new(rusqlite::ffi::SQLITE_IOERR),
        Some("disk read failed".into()),
    ));
    assert!(matches!(
        classify_scheduler_sync_failure(&io_error, "source.sqlite", "destination.sqlite"),
        AppError::DatabaseError(_)
    ));
}

#[test]
fn stale_sync_preview_remains_a_conflict() {
    let error = anyhow::Error::new(StaleSyncPreview).context("applying preview");
    let app_error = classify_scheduler_sync_failure(&error, "source.sqlite", "dest.sqlite");
    assert!(matches!(&app_error, AppError::Conflict(_)));
    assert_eq!(app_error.into_response().status(), StatusCode::CONFLICT);
}

#[test]
fn exhausted_snapshot_wait_is_an_operational_database_failure() {
    let error = anyhow::Error::new(crate::server::sync_preview::SyncSnapshotContention {
        message: "source remained busy".into(),
    });
    match classify_sync_snapshot_failure(error, "source.sqlite") {
        AppError::DatabaseError(message) => {
            assert!(message.contains("source.sqlite"));
            assert!(message.contains("busy or locked"));
        }
        other => panic!("snapshot contention was misclassified: {other:?}"),
    }
}

#[test]
fn snapshot_filesystem_failures_are_operational() {
    let error = anyhow::anyhow!("renaming server-owned snapshot failed: access denied");
    assert!(matches!(
        classify_sync_snapshot_failure(error, "source.sqlite"),
        AppError::DatabaseError(_)
    ));
}

#[test]
fn a_stopped_preview_is_a_conflict() {
    let error = anyhow::Error::new(PreviewStopped).context("previewing a pull");
    let app_error = classify_scheduler_sync_failure(&error, "source.sqlite", "dest.sqlite");
    assert!(matches!(&app_error, AppError::Conflict(message) if message.contains("stopped")));
}

#[test]
fn a_preview_stop_interrupts_the_statement_under_way() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    let stop = Stop::new();
    stop.interrupts(&connection);
    let count =
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000000) \
                 SELECT COUNT(*) FROM n";
    // Not stopped: the statement runs to the end.
    let rows: i64 = connection.query_row(count, [], |row| row.get(0)).unwrap();
    assert_eq!(rows, 1_000_000);
    // Stopped: SQLite ends it early.
    stop.request();
    let error = connection
        .query_row(count, [], |row| row.get::<_, i64>(0))
        .unwrap_err();
    assert_eq!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::OperationInterrupted)
    );
}
