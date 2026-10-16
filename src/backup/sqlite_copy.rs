//! A consistent copy of a SQLite file that someone else may be writing.
//!
//! The copy goes through SQLite's online backup API, a few pages at a time,
//! with a pause between steps. Each step holds a read lock only while it
//! copies its pages, so N.I.N.A. and the server keep writing in between. A
//! write by another connection makes SQLite start the copy again; after a
//! few restarts the steps grow, so a busy catalog still finishes. The source
//! opens read-only and is never written.
//!
//! How far the steps grow depends on the source's journal. In WAL mode a
//! reader never blocks a writer, so the last resort is one step for the
//! rest of the file. In rollback-journal mode a step blocks writers while it
//! runs, so steps stop at [`MAX_LOCKING_STEP_PAGES`], and a file that keeps
//! changing through [`MAX_LOCKING_RESTARTS`] restarts is given up rather
//! than locked for long.

use anyhow::{Context, Result};
use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::time::{Duration, Instant};

/// Pages per step at first: 1 MiB at the common 4 KiB page size.
pub const STEP_PAGES: i32 = 256;
/// Restarts before the steps start to grow.
const RESTARTS_BEFORE_GROWING: u32 = 3;
/// Restarts before one step takes everything left, in WAL mode.
const RESTARTS_BEFORE_ALL_AT_ONCE: u32 = 8;
/// The largest step that blocks writers: 16 MiB at 4 KiB pages, tens of
/// milliseconds from disk.
pub const MAX_LOCKING_STEP_PAGES: i32 = 4096;
/// Restarts a rollback-journal source may cause before the copy gives up.
pub const MAX_LOCKING_RESTARTS: u32 = 40;
/// How long to wait when the source is locked for writing.
const BUSY_NAP: Duration = Duration::from_millis(100);
/// How long a source may stay locked before the copy gives up.
const BUSY_LIMIT: Duration = Duration::from_secs(120);

/// The copy stopped because someone asked it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the backup was stopped")
    }
}

impl std::error::Error for Cancelled {}

/// What the copy sees between steps.
pub struct StepHooks<'a> {
    /// Pause between steps, so writers get the file back.
    pub pause: Duration,
    /// True once the copy should stop.
    pub cancelled: &'a dyn Fn() -> bool,
    /// Called before each step; may block while other work goes first.
    pub before_step: &'a dyn Fn(),
    /// Pages copied so far and the source's page count.
    pub progress: &'a dyn Fn(u64, u64),
}

/// How the copy went.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyStats {
    pub steps: u32,
    /// Times a write elsewhere made SQLite start over.
    pub restarts: u32,
    pub page_count: u64,
}

/// Open `path` read-only, for a copy or a look.
pub fn open_read_only(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening {} read-only", path.display()))?;
    connection.busy_timeout(Duration::from_millis(250))?;
    Ok(connection)
}

/// Copy `source` to a new file at `destination`, which must not exist yet.
pub fn copy_online(source: &Path, destination: &Path, hooks: &StepHooks) -> Result<CopyStats> {
    if destination.exists() {
        anyhow::bail!("{} already exists", destination.display());
    }
    let from = open_read_only(source)?;
    let wal = from
        .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
        .is_ok_and(|mode| mode.eq_ignore_ascii_case("wal"));
    let mut to = Connection::open(destination)
        .with_context(|| format!("creating {}", destination.display()))?;
    let stats = {
        let backup = Backup::new(&from, &mut to)
            .with_context(|| format!("starting the copy of {}", source.display()))?;
        run_steps(&backup, wal, hooks).with_context(|| format!("copying {}", source.display()))?
    };
    drop(from);
    to.close()
        .map_err(|(_, error)| error)
        .with_context(|| format!("closing {}", destination.display()))?;
    Ok(stats)
}

fn run_steps(backup: &Backup<'_, '_>, wal: bool, hooks: &StepHooks) -> Result<CopyStats> {
    let mut stats = CopyStats::default();
    let mut pages = STEP_PAGES;
    let mut last_remaining: Option<i32> = None;
    let mut busy_since: Option<Instant> = None;
    loop {
        if (hooks.cancelled)() {
            return Err(Cancelled.into());
        }
        (hooks.before_step)();
        if (hooks.cancelled)() {
            return Err(Cancelled.into());
        }
        let result = backup.step(pages)?;
        stats.steps += 1;
        let progress = backup.progress();
        let total = progress.pagecount.max(0) as u64;
        let remaining = progress.remaining.max(0);
        stats.page_count = total;
        match result {
            StepResult::Done => {
                (hooks.progress)(total, total);
                return Ok(stats);
            }
            StepResult::More => {
                busy_since = None;
                if last_remaining.is_some_and(|last| remaining > last) {
                    // Someone wrote to the source: SQLite started over.
                    stats.restarts += 1;
                    if wal && stats.restarts >= RESTARTS_BEFORE_ALL_AT_ONCE {
                        pages = -1;
                    } else if stats.restarts >= RESTARTS_BEFORE_GROWING {
                        pages = pages.saturating_mul(4);
                        if !wal {
                            pages = pages.min(MAX_LOCKING_STEP_PAGES);
                        }
                    }
                    if !wal && stats.restarts >= MAX_LOCKING_RESTARTS {
                        anyhow::bail!(
                            "the database kept changing while it was copied; try again when \
                             it is quieter"
                        );
                    }
                }
                last_remaining = Some(remaining);
                (hooks.progress)(total.saturating_sub(remaining as u64), total);
                std::thread::sleep(hooks.pause);
            }
            StepResult::Busy | StepResult::Locked => {
                let since = *busy_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= BUSY_LIMIT {
                    anyhow::bail!("the database stayed locked for {}s", BUSY_LIMIT.as_secs());
                }
                std::thread::sleep(BUSY_NAP);
            }
            #[allow(unreachable_patterns)]
            _ => std::thread::sleep(BUSY_NAP),
        }
    }
}

/// What a copied SQLite file says about itself.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SqliteInfo {
    pub page_size: i64,
    pub page_count: i64,
    /// `PRAGMA user_version`: the Target Scheduler schema version of a
    /// catalog.
    pub user_version: i64,
    /// PSF Guard's own schema version for the tables it adds to a catalog,
    /// when it has added them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psf_guard_schema: Option<i64>,
    /// `PRAGMA quick_check` on the copy: `ok` when it is sound.
    pub quick_check: String,
    /// The `psf_guard_*` tables in the file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub psf_guard_tables: Vec<String>,
    /// Rows in `acquiredimage`, for a catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acquired_images: Option<i64>,
}

/// Read [`SqliteInfo`] from a copy nobody else has open.
pub fn inspect(path: &Path) -> Result<SqliteInfo> {
    let connection = open_read_only(path)?;
    let pragma = |name: &str| -> Result<i64> {
        Ok(connection.query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))?)
    };
    let tables: Vec<String> = {
        let mut statement = connection.prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'psf\\_guard\\_%' ESCAPE '\\' ORDER BY name",
        )?;
        statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    let has_table = |name: &str| -> Result<bool> {
        Ok(connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [name],
            |row| row.get(0),
        )?)
    };
    let psf_guard_schema = if has_table("psf_guard_calibration_schema")? {
        connection
            .query_row(
                "SELECT version FROM psf_guard_calibration_schema WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .ok()
    } else {
        None
    };
    let acquired_images = if has_table("acquiredimage")? {
        connection
            .query_row("SELECT COUNT(*) FROM acquiredimage", [], |row| row.get(0))
            .ok()
    } else {
        None
    };
    let quick_check: String =
        connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    Ok(SqliteInfo {
        page_size: pragma("page_size")?,
        page_count: pragma("page_count")?,
        user_version: pragma("user_version")?,
        psf_guard_schema,
        quick_check,
        psf_guard_tables: tables,
        acquired_images,
    })
}

/// The bytes a SQLite file holds on disk now, its write-ahead log included.
pub fn size_on_disk(path: &Path) -> u64 {
    let main = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    main + std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;

    fn quiet<'a>(cancelled: &'a dyn Fn() -> bool) -> StepHooks<'a> {
        StepHooks {
            pause: Duration::from_millis(0),
            cancelled,
            before_step: &|| {},
            progress: &|_, _| {},
        }
    }

    fn catalog(path: &Path, rows: usize) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA user_version = 21;
                 CREATE TABLE acquiredimage (id INTEGER PRIMARY KEY, payload BLOB);
                 CREATE TABLE psf_guard_calibration_schema (singleton INTEGER PRIMARY KEY, version INTEGER);
                 INSERT INTO psf_guard_calibration_schema VALUES (1, 9);",
            )
            .unwrap();
        let mut insert = connection
            .prepare("INSERT INTO acquiredimage (payload) VALUES (?1)")
            .unwrap();
        for _ in 0..rows {
            insert.execute([vec![7u8; 2048]]).unwrap();
        }
    }

    #[test]
    fn a_copy_reads_back_with_its_versions() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("catalog.sqlite");
        catalog(&source, 300);
        let copy = dir.path().join("copy.sqlite");
        let stats = copy_online(&source, &copy, &quiet(&|| false)).unwrap();
        assert!(stats.steps >= 1);
        let info = inspect(&copy).unwrap();
        assert_eq!(info.user_version, 21);
        assert_eq!(info.psf_guard_schema, Some(9));
        assert_eq!(info.acquired_images, Some(300));
        assert_eq!(info.quick_check, "ok");
        assert_eq!(info.psf_guard_tables, vec!["psf_guard_calibration_schema"]);
    }

    #[test]
    fn a_copy_is_consistent_while_another_connection_writes() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("catalog.sqlite");
        catalog(&source, 2000);
        let stop = Arc::new(AtomicBool::new(false));
        let writes = Arc::new(AtomicU32::new(0));
        let writer = {
            let (source, stop, writes) = (source.clone(), stop.clone(), writes.clone());
            std::thread::spawn(move || {
                let connection = Connection::open(&source).unwrap();
                connection.busy_timeout(Duration::from_secs(5)).unwrap();
                while !stop.load(Ordering::SeqCst) {
                    connection
                        .execute(
                            "INSERT INTO acquiredimage (payload) VALUES (?1)",
                            [vec![1u8; 512]],
                        )
                        .unwrap();
                    writes.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(2));
                }
            })
        };
        // Small steps with a pause, so the writer lands between them.
        let copy = dir.path().join("copy.sqlite");
        let hooks = StepHooks {
            pause: Duration::from_millis(1),
            cancelled: &|| false,
            before_step: &|| {},
            progress: &|_, _| {},
        };
        let stats = copy_online(&source, &copy, &hooks).unwrap();
        stop.store(true, Ordering::SeqCst);
        writer.join().unwrap();
        assert!(writes.load(Ordering::SeqCst) > 0, "the writer never ran");
        let info = inspect(&copy).unwrap();
        assert_eq!(info.quick_check, "ok");
        // A snapshot: every row the copy holds is whole, and it holds at
        // least what was there before the writer began.
        assert!(info.acquired_images.unwrap() >= 2000);
        assert!(stats.steps > 1);
        // The source took every write; the copy never held it up.
        let source_rows: i64 = open_read_only(&source)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM acquiredimage", [], |row| row.get(0))
            .unwrap();
        assert_eq!(source_rows, 2000 + i64::from(writes.load(Ordering::SeqCst)));
    }

    #[test]
    fn a_rollback_journal_catalog_is_copied_in_bounded_steps_while_written() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("catalog.sqlite");
        catalog(&source, 1500);
        Connection::open(&source)
            .unwrap()
            .execute_batch("PRAGMA journal_mode = DELETE;")
            .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (source, stop) = (source.clone(), stop.clone());
            std::thread::spawn(move || {
                let connection = Connection::open(&source).unwrap();
                connection.busy_timeout(Duration::from_secs(5)).unwrap();
                let mut slowest = Duration::ZERO;
                while !stop.load(Ordering::SeqCst) {
                    let began = Instant::now();
                    connection
                        .execute("INSERT INTO acquiredimage (payload) VALUES (x'00')", [])
                        .unwrap();
                    slowest = slowest.max(began.elapsed());
                    std::thread::sleep(Duration::from_millis(15));
                }
                slowest
            })
        };
        let hooks = StepHooks {
            pause: Duration::from_millis(2),
            cancelled: &|| false,
            before_step: &|| {},
            progress: &|_, _| {},
        };
        let copy = dir.path().join("copy.sqlite");
        let result = copy_online(&source, &copy, &hooks);
        stop.store(true, Ordering::SeqCst);
        let slowest = writer.join().unwrap();
        // Either a sound copy, or an honest refusal; never a writer held
        // up for long.
        match result {
            Ok(_) => assert_eq!(inspect(&copy).unwrap().quick_check, "ok"),
            Err(error) => assert!(format!("{error:#}").contains("kept changing"), "{error:#}"),
        }
        assert!(
            slowest < Duration::from_secs(2),
            "a write waited {slowest:?}"
        );
    }

    #[test]
    fn a_stopped_copy_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("catalog.sqlite");
        catalog(&source, 50);
        let error =
            copy_online(&source, &dir.path().join("copy.sqlite"), &quiet(&|| true)).unwrap_err();
        assert!(error.downcast_ref::<Cancelled>().is_some(), "{error:#}");
    }

    #[test]
    fn the_source_is_never_opened_for_writing() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("catalog.sqlite");
        catalog(&source, 10);
        let connection = open_read_only(&source).unwrap();
        assert!(connection.is_readonly(rusqlite::MAIN_DB).unwrap());
    }
}
