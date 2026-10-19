//! What stack preparation read from files, written to the catalog after it.
//!
//! Preparation reads a light's header when the catalog keeps none that holds
//! its file, and a calibration file's header when the catalog cannot vouch
//! for it. It runs on the preparation runner, ahead of the builds it queues,
//! and on refreshes and restores; keeping what it found must hold up none of
//! them, nor the shared connection every request waits on. So it hands that
//! here. One writer per catalog, on a connection of its own, takes what was
//! queued while it ran and stops when nothing is left. A write that fails,
//! as on a catalog that cannot be written, loses only a cache: the next
//! preparation reads those files again.

use crate::calibration::FileCheck;
use crate::light_headers::KeptHeader;
use rusqlite::OpenFlags;
use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};

#[derive(Default)]
struct Queue {
    headers: BTreeMap<i32, KeptHeader>,
    file_checks: Vec<FileCheck>,
    writing: bool,
}

static QUEUES: LazyLock<Mutex<HashMap<String, Queue>>> = LazyLock::new(Default::default);

/// Keep these headers and calibration file checks in the catalog at
/// `database_path`, after the caller returns.
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
    let mut queues = QUEUES.lock().unwrap_or_else(|p| p.into_inner());
    let queue = queues.entry(database_path.to_string()).or_default();
    queue.headers.extend(headers);
    queue.file_checks.extend(file_checks);
    if std::mem::replace(&mut queue.writing, true) {
        // The writer serves this catalog already and takes these next.
        return;
    }
    let path = database_path.to_string();
    std::thread::spawn(move || {
        let _claim = WriterClaim(path.clone());
        write_queued(&path);
    });
}

/// Lets the next queue start a writer should this one panic. A writer that
/// ends normally lets go under the queue's lock instead.
struct WriterClaim(String);

impl Drop for WriterClaim {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        if let Some(queue) = QUEUES
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(&self.0)
        {
            queue.writing = false;
        }
    }
}

fn write_queued(path: &str) {
    loop {
        let (headers, file_checks) = {
            let mut queues = QUEUES.lock().unwrap_or_else(|p| p.into_inner());
            let Some(queue) = queues.get_mut(path) else {
                return;
            };
            if queue.headers.is_empty() && queue.file_checks.is_empty() {
                // Let go with the lock held, so nothing can queue
                // between this check and the release.
                queue.writing = false;
                return;
            }
            (
                std::mem::take(&mut queue.headers),
                std::mem::take(&mut queue.file_checks),
            )
        };
        if let Err(error) = write(path, &headers, &file_checks) {
            tracing::debug!(
                "Could not keep {} light header(s) and {} calibration file check(s) in {path}: \
                 {error:#}. They are read again next time.",
                headers.len(),
                file_checks.len()
            );
        }
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
    if !headers.is_empty() {
        crate::light_headers::store(&conn, headers.iter().map(|(id, header)| (*id, header)))?;
    }
    crate::calibration::record_file_checks(&conn, file_checks)?;
    Ok(())
}

/// Wait until nothing is queued or being written for `path`.
#[cfg(test)]
pub(crate) fn wait_until_written(path: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let idle = QUEUES
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(path)
            .is_none_or(|queue| !queue.writing);
        if idle {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the light header writer did not finish");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light_headers::FileStamp;

    #[test]
    fn queued_headers_reach_the_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let catalog = temp.path().join("catalog.sqlite");
        let conn = rusqlite::Connection::open(&catalog).unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
                 gradingStatus, metadata)
             VALUES (7, 1, 1, 0, 'Ha', 0, '{}')",
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
        let path = catalog.to_string_lossy().into_owned();
        queue(&path, vec![(7, header)], Vec::new());
        wait_until_written(&path);
        let kept = crate::light_headers::project_headers(&conn, 1).unwrap();
        assert_eq!(kept[&7].meta.object.as_deref(), Some("M31"));
        assert!(kept[&7].located);
    }
}
