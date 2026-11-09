//! Which journal a PSF Guard catalog keeps: write-ahead logging on a local
//! disk, the rollback journal on a network share.
//!
//! Only PSF Guard opens its catalogs; N.I.N.A. reaches them through the Sync
//! plugin. With write-ahead logging, readers never wait for a writer and a
//! writer never waits for readers, so a long read (an export, a preview, a
//! background pass) no longer holds up grading or an import, and a run of
//! reads can share one snapshot cheaply.
//!
//! Write-ahead logging keeps its index in shared memory beside the file,
//! which network filesystems do not provide reliably, so a catalog on a
//! network share keeps the rollback journal. A filesystem this module cannot
//! identify counts as a network share: the rollback journal is always safe.

use std::path::Path;

use rusqlite::{Connection, MAIN_DB};

/// The journal a catalog keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Journal {
    /// Write-ahead logging, on a local disk.
    Wal,
    /// The rollback journal, on a network share or read-only storage.
    Rollback,
}

/// Put an open catalog in the journal its storage calls for, and answer the
/// one it keeps. A catalog opened read-only keeps whatever it has. When the
/// change cannot be made now (another connection is mid-transaction), the
/// catalog keeps its journal and the next open tries again.
pub fn choose(conn: &Connection, path: &Path) -> Journal {
    let current = current(conn);
    if conn.is_readonly(MAIN_DB).unwrap_or(true) {
        return current;
    }
    let wanted = if on_network_share(path) {
        Journal::Rollback
    } else {
        Journal::Wal
    };
    if wanted == current {
        return current;
    }
    let pragma = match wanted {
        Journal::Wal => "WAL",
        Journal::Rollback => "DELETE",
    };
    match conn.pragma_update_and_check(None, "journal_mode", pragma, |row| row.get::<_, String>(0))
    {
        Ok(mode) => {
            let kept = journal_of(&mode);
            if kept == wanted {
                tracing::info!(catalog = %path.display(), journal = %mode, "Catalog journal set");
            } else {
                tracing::warn!(catalog = %path.display(), journal = %mode, "Catalog journal could not be changed");
            }
            kept
        }
        Err(error) => {
            tracing::warn!(catalog = %path.display(), %error, "Catalog journal could not be changed");
            current
        }
    }
}

fn current(conn: &Connection) -> Journal {
    conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
        .map(|mode| journal_of(&mode))
        .unwrap_or(Journal::Rollback)
}

fn journal_of(mode: &str) -> Journal {
    if mode.eq_ignore_ascii_case("wal") {
        Journal::Wal
    } else {
        Journal::Rollback
    }
}

/// Whether `path` lies on a network share, or on a filesystem this cannot
/// tell from one. A file that does not exist yet is judged by its folder.
pub fn on_network_share(path: &Path) -> bool {
    #[cfg(test)]
    if test_shares::covers(path) {
        return true;
    }
    let existing = path
        .ancestors()
        .find(|candidate| candidate.exists())
        .unwrap_or(path);
    platform::on_network_share(existing)
}

/// Folders tests treat as network shares, to exercise the rollback
/// journal on a local disk. Each test names its own temporary folder.
#[cfg(test)]
mod test_shares {
    use std::{
        path::{Path, PathBuf},
        sync::Mutex,
    };

    static SHARES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    pub(crate) fn add(root: &Path) {
        SHARES.lock().unwrap().push(root.to_path_buf());
    }

    pub(super) fn covers(path: &Path) -> bool {
        SHARES
            .lock()
            .unwrap()
            .iter()
            .any(|root| path.starts_with(root))
    }
}

/// Treat catalogs under `root` as on a network share, so they keep the
/// rollback journal. For tests of what a share's locking does.
#[cfg(test)]
pub(crate) fn treat_as_network_share(root: &Path) {
    test_shares::add(root);
}

#[cfg(target_os = "linux")]
mod platform {
    use std::{ffi::CString, os::unix::ffi::OsStrExt, path::Path};

    /// `statfs` filesystem types that are network or user-space
    /// filesystems. FUSE covers sshfs and rclone as well as local ones such
    /// as ntfs-3g; it counts as a network share, the safe side.
    const NETWORK: &[i64] = &[
        0x6969,                 // NFS
        0x517b,                 // SMB
        0xff53_4d42_u32 as i64, // CIFS
        0xfe53_4d42_u32 as i64, // SMB2
        0x6573_5546,            // FUSE
        0x0102_1997,            // 9P
        0x5346_414f,            // AFS
        0x00c3_6400,            // Ceph
        0x7375_7245,            // Coda
        0x6b41_4653,            // kAFS
        0x0bd0_0bd0,            // Lustre
    ];

    pub(super) fn on_network_share(path: &Path) -> bool {
        let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
            return true;
        };
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `path` is a NUL-terminated string and `stat` a valid
        // out-pointer for the duration of the call.
        if unsafe { libc::statfs(path.as_ptr(), &mut stat) } != 0 {
            return true;
        }
        #[allow(clippy::unnecessary_cast)]
        let kind = stat.f_type as i64;
        NETWORK.contains(&kind)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::{ffi::CString, os::unix::ffi::OsStrExt, path::Path};

    pub(super) fn on_network_share(path: &Path) -> bool {
        let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
            return true;
        };
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `path` is a NUL-terminated string and `stat` a valid
        // out-pointer for the duration of the call.
        if unsafe { libc::statfs(path.as_ptr(), &mut stat) } != 0 {
            return true;
        }
        stat.f_flags & (libc::MNT_LOCAL as u32) == 0
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::{os::windows::ffi::OsStrExt, path::Path};

    /// `GetDriveTypeW`'s answer for a network drive.
    const DRIVE_REMOTE: u32 = 4;

    pub(super) fn on_network_share(path: &Path) -> bool {
        let Ok(path) = std::fs::canonicalize(path) else {
            return true;
        };
        // Canonical paths are verbatim: `\\?\UNC\server\share\...` for a
        // share, `\\?\C:\...` for a drive.
        let text = path.as_os_str().to_string_lossy();
        let Some(rest) = text.strip_prefix(r"\\?\") else {
            return true;
        };
        if rest
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"UNC\"))
        {
            return true;
        }
        let Some(drive) = rest.get(..3) else {
            return true;
        };
        let root: Vec<u16> = std::ffi::OsStr::new(drive)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `root` is a NUL-terminated wide string such as `C:\`.
        let kind = unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) };
        kind == DRIVE_REMOTE
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod platform {
    pub(super) fn on_network_share(_path: &std::path::Path) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalog_on_a_local_disk_takes_the_write_ahead_log_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        assert!(!on_network_share(&path), "a temporary folder is local");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE t(x)").unwrap();
        assert_eq!(current(&conn), Journal::Rollback);
        assert_eq!(choose(&conn, &path), Journal::Wal);
        drop(conn);
        // The mode is the file's: the next connection finds it.
        let again = Connection::open(&path).unwrap();
        assert_eq!(current(&again), Journal::Wal);
        assert_eq!(choose(&again, &path), Journal::Wal);
    }

    #[test]
    fn a_catalog_opened_read_only_keeps_its_journal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE t(x)")
            .unwrap();
        let conn =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(choose(&conn, &path), Journal::Rollback);
    }

    #[test]
    fn a_catalog_on_a_network_share_keeps_the_rollback_journal() {
        let dir = tempfile::tempdir().unwrap();
        treat_as_network_share(dir.path());
        let path = dir.path().join("catalog.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t(x)")
            .unwrap();
        // Moved onto a share in WAL mode: it goes back to the rollback journal.
        assert_eq!(choose(&conn, &path), Journal::Rollback);
        assert_eq!(current(&conn), Journal::Rollback);
    }

    #[test]
    fn a_path_that_does_not_exist_yet_is_judged_by_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!on_network_share(
            &dir.path().join("new").join("catalog.sqlite")
        ));
    }
}
