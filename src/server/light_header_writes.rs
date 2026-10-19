//! What stack preparation read from files, written to the catalog after it.
//!
//! Preparation reads a light's header when the catalog keeps none that holds
//! its file, and a calibration file's header when the catalog cannot vouch
//! for it. Writing that must not hold up the builds it prepares, nor the
//! shared connection, so it only hands what it read here. Threads that run
//! already write it, each on a connection of its own:
//!
//! - the preparation runner, once its line is empty, so a person's next
//!   build never waits on the write;
//! - the calibration health pass, a background job, which takes what
//!   automatic refreshes and restored builds prepared.
//!
//! No thread is started for the writes, and a write waits at most
//! [`BUSY_WAIT`] for a catalog another program is writing: an import, a sync
//! or N.I.N.A. may hold it for long. Then what it would have written waits
//! for the next writer. Any other failure, as on a catalog that cannot be
//! written, loses only a cache: the next preparation reads those files again.

use crate::calibration::FileCheck;
use crate::light_headers::KeptHeader;
use rusqlite::{ErrorCode, OpenFlags};
use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// How long a write waits for a catalog another connection is writing.
const BUSY_WAIT: Duration = Duration::from_millis(250);

/// One catalog's headers and file checks waiting to be written.
#[derive(Default)]
struct Pending {
    headers: BTreeMap<i32, KeptHeader>,
    file_checks: Vec<FileCheck>,
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(Default::default);

/// Hand these headers and calibration file checks over to be kept in the
/// catalog at `database_path` by [`write_pending`].
pub(crate) fn queue(
    database_path: &str,
    headers: Vec<(i32, KeptHeader)>,
    file_checks: Vec<FileCheck>,
) {
    if headers.is_empty() && file_checks.is_empty() {
        return;
    }
    // An in-memory catalog, as tests open, has no file to write to.
    if !std::path::Path::new(database_path).is_file() {
        return;
    }
    let mut pending = PENDING.lock().unwrap_or_else(|p| p.into_inner());
    let entry = pending.entry(database_path.to_string()).or_default();
    entry.headers.extend(headers);
    entry.file_checks.extend(file_checks);
}

/// Write what waits for the catalog at `database_path`, on a connection of
/// its own, on the caller's thread.
pub(crate) fn write_pending(database_path: &str) {
    let Some(Pending {
        headers,
        file_checks,
    }) = PENDING
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(database_path)
    else {
        return;
    };
    if let Err(error) = write(database_path, &headers, &file_checks) {
        if is_busy(&error) {
            // Kept for the next writer; anything queued since is newer.
            let mut pending = PENDING.lock().unwrap_or_else(|p| p.into_inner());
            let entry = pending.entry(database_path.to_string()).or_default();
            for (image_id, header) in headers {
                entry.headers.entry(image_id).or_insert(header);
            }
            entry.file_checks.extend(file_checks);
            return;
        }
        tracing::debug!(
            "Could not keep {} light header(s) and {} calibration file check(s) in \
             {database_path}: {error:#}. They are read again next time.",
            headers.len(),
            file_checks.len()
        );
    }
}

fn write(
    path: &str,
    headers: &BTreeMap<i32, KeptHeader>,
    file_checks: &[FileCheck],
) -> anyhow::Result<()> {
    let conn = crate::server::database_context::open_scheduler_connection_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(BUSY_WAIT)?;
    if !headers.is_empty() {
        crate::light_headers::store(&conn, headers.iter().map(|(id, header)| (*id, header)))?;
    }
    crate::calibration::record_file_checks(&conn, file_checks)?;
    Ok(())
}

/// Whether a write failed only because another connection held the catalog.
fn is_busy(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(failure, _))
                if matches!(failure.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light_headers::FileStamp;

    /// A catalog file with light 7, and a header for it.
    fn catalog_and_header(temp: &tempfile::TempDir) -> (rusqlite::Connection, String, KeptHeader) {
        let catalog = temp.path().join("catalog.sqlite");
        let conn = rusqlite::Connection::open(&catalog).unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
                 gradingStatus, metadata, guid)
             VALUES (7, 1, 1, 0, 'Ha', 0, '{}', 'light-seven')",
            [],
        )
        .unwrap();
        let light = temp.path().join("light.fits");
        std::fs::write(&light, b"bytes").unwrap();
        let header = KeptHeader {
            located: true,
            ..KeptHeader::new(
                &light,
                FileStamp::of(&light).unwrap(),
                crate::commands::import::headers::FrameMeta {
                    path: light.clone(),
                    readable: true,
                    object: Some("M31".into()),
                    ..Default::default()
                },
            )
        };
        (conn, catalog.to_string_lossy().into_owned(), header)
    }

    #[test]
    fn a_busy_catalog_does_not_hold_the_writer_and_keeps_the_batch() {
        let temp = tempfile::tempdir().unwrap();
        let (conn, path, header) = catalog_and_header(&temp);
        // Another program holds the catalog's write lock, as an import does.
        let importer = rusqlite::Connection::open(&path).unwrap();
        importer.execute_batch("BEGIN IMMEDIATE").unwrap();
        queue(&path, vec![(7, header)], Vec::new());
        let began = std::time::Instant::now();
        write_pending(&path);
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "the writer gave up at once: {:?}",
            began.elapsed()
        );
        importer.execute_batch("COMMIT").unwrap();
        assert!(crate::light_headers::project_headers(&conn, 1)
            .unwrap()
            .is_empty());

        // The batch waited for the next writer.
        write_pending(&path);
        let kept = crate::light_headers::project_headers(&conn, 1).unwrap();
        assert_eq!(kept[&7].meta.object.as_deref(), Some("M31"));
    }

    #[test]
    fn queued_headers_reach_the_catalog_when_written() {
        let temp = tempfile::tempdir().unwrap();
        let (conn, path, header) = catalog_and_header(&temp);
        queue(&path, vec![(7, header)], Vec::new());
        assert!(
            crate::light_headers::project_headers(&conn, 1)
                .unwrap()
                .is_empty(),
            "nothing is written until a runner writes it"
        );
        write_pending(&path);
        let kept = crate::light_headers::project_headers(&conn, 1).unwrap();
        assert_eq!(kept[&7].meta.object.as_deref(), Some("M31"));
        assert!(kept[&7].located);
        write_pending(&path);
    }
}
