//! Filling in Target Scheduler GUIDs that its own migration left empty.
//!
//! Target Scheduler's schema 22 added a `guid` column to six tables and
//! meant to give every existing row one: `SchedulerDatabaseContext` fills
//! them in `RepairAndUpdate` when `oldVersion < 22 && newVersion == 22`.
//! That runs once, after every pending migration script, with `newVersion`
//! set to the final version. A database that went from 21 or earlier
//! straight to 23 in one start therefore skipped the fill, and its older
//! rows kept an empty `guid` for good. New rows get one from the entity
//! constructors, so only the rows from before the upgrade are affected.
//!
//! PSF Guard identifies projects by that GUID (plans, sync, the Library's
//! multi-rig families), so such rows are invisible to planning. This module
//! does what the migration meant to: one new random GUID per empty row, in
//! the same lower-case hyphenated form .NET's `Guid.ToString()` writes, and
//! nothing else. Rows that already have one are never touched.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags, TransactionBehavior};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The tables Target Scheduler's migration 22 gave a `guid` column.
pub const GUID_TABLES: [&str; 6] = [
    "project",
    "target",
    "exposureplan",
    "exposuretemplate",
    "acquiredimage",
    "profilepreference",
];

/// One table's rows without a GUID, out of all its rows.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GuidGap {
    pub table: &'static str,
    pub missing: u64,
    pub total: u64,
}

fn has_guid_column(conn: &Connection, table: &str) -> bool {
    conn.prepare(&format!("PRAGMA table_info({table})"))
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
            Ok(rows.flatten().any(|name| name.eq_ignore_ascii_case("guid")))
        })
        .unwrap_or(false)
}

/// How many rows in each GUID table lack one. A table without the column
/// (a database from before schema 22) is left out: there is nothing to fill.
pub fn missing_guids(conn: &Connection) -> Result<Vec<GuidGap>> {
    let mut gaps = Vec::new();
    for table in GUID_TABLES {
        if !has_guid_column(conn, table) {
            continue;
        }
        let (missing, total): (i64, i64) = conn
            .query_row(
                &format!(
                    "SELECT COALESCE(SUM(guid IS NULL OR TRIM(guid) = ''), 0), COUNT(*) FROM {table}"
                ),
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .with_context(|| format!("count missing GUIDs in {table}"))?;
        gaps.push(GuidGap {
            table,
            missing: missing.max(0) as u64,
            total: total.max(0) as u64,
        });
    }
    Ok(gaps)
}

/// Total rows missing a GUID across every table.
pub fn total_missing(gaps: &[GuidGap]) -> u64 {
    gaps.iter().map(|gap| gap.missing).sum()
}

/// Give every row without a GUID a new one, in one transaction, and return
/// how many each table got. Rows that already have a GUID are not touched.
/// The transaction takes its write lock up front, so a writer already at
/// work (N.I.N.A., a refresh) is waited for rather than failed against.
pub fn fill_missing_guids(conn: &mut Connection) -> Result<Vec<GuidGap>> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("start the GUID fill")?;
    let mut filled = Vec::new();
    for table in GUID_TABLES {
        if !has_guid_column(&tx, table) {
            continue;
        }
        let ids: Vec<i64> = {
            let mut stmt = tx.prepare(&format!(
                "SELECT Id FROM {table} WHERE guid IS NULL OR TRIM(guid) = '' ORDER BY Id"
            ))?;
            let rows = stmt.query_map([], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let total: i64 = tx.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })?;
        {
            let mut update = tx.prepare(&format!(
                "UPDATE {table} SET guid = ?1 WHERE Id = ?2 AND (guid IS NULL OR TRIM(guid) = '')"
            ))?;
            for id in &ids {
                update.execute(params![uuid::Uuid::new_v4().to_string(), id])?;
            }
        }
        filled.push(GuidGap {
            table,
            missing: ids.len() as u64,
            total: total.max(0) as u64,
        });
    }
    tx.commit().context("save the GUID fill")?;
    Ok(filled)
}

const BUSY_TIMEOUT: Duration = Duration::from_secs(60);

/// Open an existing database without ever creating one: a wrong path is an
/// error, not a new empty file.
pub fn open_existing(path: &Path, writable: bool) -> Result<Connection> {
    let flags = if writable {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
    };
    let conn = Connection::open_with_flags(path, flags)
        .with_context(|| format!("open {}", path.display()))?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}

/// Whether this process can write the database file.
pub fn is_writable(path: &Path) -> bool {
    open_existing(path, true)
        .and_then(|conn| Ok(!conn.is_readonly(rusqlite::MAIN_DB)?))
        .unwrap_or(false)
}

/// Copy the database beside itself through SQLite's online backup, so a
/// database in use is copied consistently. The copy is written under a
/// temporary name and renamed once complete, and never replaces a file
/// already there: `<file>.before-guid-fill-<unix seconds>`, with the
/// nanoseconds added if that name is taken.
pub fn backup_beside(database_path: &Path) -> Result<PathBuf> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let name = database_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "database".into());
    let beside = |suffix: String| database_path.with_file_name(format!("{name}{suffix}"));
    let partial = beside(format!(
        ".before-guid-fill-{}-{}.partial",
        std::process::id(),
        now.as_nanos()
    ));
    let copy = || -> Result<()> {
        let source = open_existing(database_path, false)?;
        let mut target =
            Connection::open(&partial).with_context(|| format!("create {}", partial.display()))?;
        rusqlite::backup::Backup::new(&source, &mut target)?.run_to_completion(
            1024,
            Duration::from_millis(5),
            None,
        )?;
        Ok(())
    };
    if let Err(error) = copy() {
        let _ = std::fs::remove_file(&partial);
        return Err(error.context(format!("back up {} first", database_path.display())));
    }
    let mut finished = beside(format!(".before-guid-fill-{}", now.as_secs()));
    if finished.exists() {
        finished = beside(format!(
            ".before-guid-fill-{}-{}",
            now.as_secs(),
            now.subsec_nanos()
        ));
    }
    if finished.exists() {
        let _ = std::fs::remove_file(&partial);
        anyhow::bail!("{} already exists; not overwriting it", finished.display());
    }
    std::fs::rename(&partial, &finished)
        .with_context(|| format!("name the copy {}", finished.display()))?;
    Ok(finished)
}

/// What a fill did: the rows it gave a GUID, per table, and the copy taken
/// first; no copy when there was nothing to fill.
#[derive(Debug, Serialize)]
pub struct FillOutcome {
    pub filled: Vec<GuidGap>,
    pub backup: Option<PathBuf>,
}

/// The whole repair on a database file: refuse a file this process cannot
/// write before copying anything, copy it, then fill. Runs on its own
/// connections, so a server's shared connection stays free meanwhile.
pub fn fill_database(path: &Path) -> Result<FillOutcome> {
    let mut conn = open_existing(path, true)?;
    if conn.is_readonly(rusqlite::MAIN_DB)? {
        anyhow::bail!(
            "{} is read only here; nothing was copied or changed",
            path.display()
        );
    }
    if total_missing(&missing_guids(&conn)?) == 0 {
        return Ok(FillOutcome {
            filled: Vec::new(),
            backup: None,
        });
    }
    let backup = backup_beside(path)?;
    let filled = fill_missing_guids(&mut conn)?;
    Ok(FillOutcome {
        filled,
        backup: Some(backup),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE project (Id INTEGER PRIMARY KEY, name TEXT, guid TEXT);
             CREATE TABLE target (Id INTEGER PRIMARY KEY, name TEXT, guid TEXT);
             CREATE TABLE exposureplan (Id INTEGER PRIMARY KEY, guid TEXT);
             CREATE TABLE exposuretemplate (Id INTEGER PRIMARY KEY, guid TEXT);
             CREATE TABLE acquiredimage (Id INTEGER PRIMARY KEY, guid TEXT);
             CREATE TABLE profilepreference (Id INTEGER PRIMARY KEY, guid TEXT);
             INSERT INTO project VALUES (1, 'Wizard', NULL), (2, 'Kept', 'aaaaaaaa-0000-4000-8000-000000000001'), (3, 'Blank', '  ');
             INSERT INTO target VALUES (1, 'NGC 7380', NULL);
             INSERT INTO acquiredimage VALUES (1, NULL), (2, NULL);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn counts_the_rows_the_migration_skipped() {
        let conn = catalog();
        let gaps = missing_guids(&conn).unwrap();
        let project = gaps.iter().find(|gap| gap.table == "project").unwrap();
        assert_eq!((project.missing, project.total), (2, 3));
        assert_eq!(total_missing(&gaps), 5);
    }

    #[test]
    fn fills_only_empty_rows_with_distinct_lowercase_guids() {
        let mut conn = catalog();
        let filled = fill_missing_guids(&mut conn).unwrap();
        assert_eq!(total_missing(&filled), 5);
        assert_eq!(total_missing(&missing_guids(&conn).unwrap()), 0);
        let kept: String = conn
            .query_row("SELECT guid FROM project WHERE Id = 2", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(kept, "aaaaaaaa-0000-4000-8000-000000000001");
        let guids: Vec<String> = conn
            .prepare("SELECT guid FROM project UNION ALL SELECT guid FROM target UNION ALL SELECT guid FROM acquiredimage")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .flatten()
            .collect();
        let unique: std::collections::BTreeSet<_> = guids.iter().collect();
        assert_eq!(unique.len(), guids.len());
        assert!(guids
            .iter()
            .all(|guid| uuid::Uuid::parse_str(guid).is_ok() && guid == &guid.to_lowercase()));
        // A second run finds nothing to do.
        assert_eq!(total_missing(&fill_missing_guids(&mut conn).unwrap()), 0);
    }

    #[test]
    fn leaves_a_database_from_before_schema_22_alone() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE project (Id INTEGER PRIMARY KEY, name TEXT);")
            .unwrap();
        assert!(missing_guids(&conn).unwrap().is_empty());
        assert!(fill_missing_guids(&mut conn).unwrap().is_empty());
    }

    #[test]
    fn backs_up_beside_the_file_without_overwriting_and_fills_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schedulerdb.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE project (Id INTEGER PRIMARY KEY, guid TEXT); INSERT INTO project VALUES (1, NULL);")
                .unwrap();
        }
        let first = backup_beside(&path).unwrap();
        let second = backup_beside(&path).unwrap();
        assert_ne!(
            first, second,
            "a second copy in the same second keeps the first"
        );
        for copy in [&first, &second] {
            assert!(copy
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("schedulerdb.sqlite.before-guid-fill-"));
            let rows: i64 = Connection::open(copy)
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM project WHERE guid IS NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(rows, 1, "the copy is the database as it was");
        }
        assert!(std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".partial")));
        let outcome = fill_database(&path).unwrap();
        assert_eq!(total_missing(&outcome.filled), 1);
        assert!(outcome.backup.is_some());
        // Nothing left: no copy, nothing filled.
        let again = fill_database(&path).unwrap();
        assert!(again.backup.is_none() && again.filled.is_empty());
    }

    #[test]
    fn never_creates_a_database_from_a_wrong_path() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.sqlite");
        assert!(open_existing(&missing, false).is_err());
        assert!(fill_database(&missing).is_err());
        assert!(!missing.exists());
        assert!(!is_writable(&missing));
    }
}
