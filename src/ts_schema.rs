//! Create (or upgrade) a Target Scheduler database from the vendored schema.
//!
//! The SQL under `src/ts_schema/` is copied byte-for-byte from the upstream
//! plugin (see `src/ts_schema/README.md` for provenance). This module
//! replicates the bootstrap algorithm of upstream's
//! `SchedulerDatabaseContext.CreateOrMigrateDatabaseInitializer`:
//!
//! 1. If the database has no tables, run `initial_schema.sql`.
//! 2. Read `PRAGMA user_version`; apply every `migrate/N.sql` with
//!    `N > user_version` in ascending order, each in its own transaction
//!    (each script ends by setting `PRAGMA user_version = N`).
//!
//! Upstream also performs post-migration repairs in app code — notably the
//! v22 repair that backfills a fresh GUID on every existing row. A database
//! created here starts empty, so there is nothing to backfill; instead every
//! row PSF Guard inserts must carry a [`new_guid()`] value, exactly as the
//! plugin does for rows it creates after v22.

use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::path::Path;

/// The schema version the vendored migration set lands on.
pub const TS_SCHEMA_VERSION: i64 = 23;

const INITIAL_SCHEMA: &str = include_str!("ts_schema/initial_schema.sql");

/// Migration scripts in replay order; `MIGRATIONS[i]` is `migrate/{i+1}.sql`.
const MIGRATIONS: [&str; TS_SCHEMA_VERSION as usize] = [
    include_str!("ts_schema/migrate/1.sql"),
    include_str!("ts_schema/migrate/2.sql"),
    include_str!("ts_schema/migrate/3.sql"),
    include_str!("ts_schema/migrate/4.sql"),
    include_str!("ts_schema/migrate/5.sql"),
    include_str!("ts_schema/migrate/6.sql"),
    include_str!("ts_schema/migrate/7.sql"),
    include_str!("ts_schema/migrate/8.sql"),
    include_str!("ts_schema/migrate/9.sql"),
    include_str!("ts_schema/migrate/10.sql"),
    include_str!("ts_schema/migrate/11.sql"),
    include_str!("ts_schema/migrate/12.sql"),
    include_str!("ts_schema/migrate/13.sql"),
    include_str!("ts_schema/migrate/14.sql"),
    include_str!("ts_schema/migrate/15.sql"),
    include_str!("ts_schema/migrate/16.sql"),
    include_str!("ts_schema/migrate/17.sql"),
    include_str!("ts_schema/migrate/18.sql"),
    include_str!("ts_schema/migrate/19.sql"),
    include_str!("ts_schema/migrate/20.sql"),
    include_str!("ts_schema/migrate/21.sql"),
    include_str!("ts_schema/migrate/22.sql"),
    include_str!("ts_schema/migrate/23.sql"),
];

/// Fresh GUID in the format the plugin writes (`Guid.NewGuid().ToString()`,
/// i.e. lowercase hyphenated).
pub fn new_guid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Create a brand-new Target Scheduler database at `path` and return the open
/// connection. Refuses to touch an existing non-empty file.
pub fn create_fresh_db(path: &Path) -> Result<Connection> {
    if path.exists() {
        let len = std::fs::metadata(path)
            .with_context(|| format!("stat {}", path.display()))?
            .len();
        if len > 0 {
            bail!(
                "{} already exists — refusing to overwrite. \
                 Use `psf-guard import` to add images to an existing database.",
                path.display()
            );
        }
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating parent directory {}", parent.display()))?;
    }

    let conn =
        Connection::open(path).with_context(|| format!("creating database {}", path.display()))?;
    apply_schema(&conn)?;
    Ok(conn)
}

/// Bring a connection to the vendored schema version: run the initial schema
/// if the database is table-less, then replay any pending migrations.
///
/// Safe on an already-current database (no-op). Errors if the database
/// reports a `user_version` NEWER than the vendored set — that database was
/// written by a newer plugin than this snapshot understands.
///
/// Refuses to migrate an EXISTING pre-v22 database: upstream pairs several
/// migrations with app-code data repairs this module does not implement
/// (v17 remaps `gradingStatus` values and converts override exposure
/// orders; v22 backfills a GUID on every row). Replaying only the SQL on a
/// populated database would leave those rows semantically wrong — such
/// databases must be upgraded by opening them in N.I.N.A. + Target
/// Scheduler. v22 → v23 is pure SQL and is replayed here.
pub fn apply_schema(conn: &Connection) -> Result<()> {
    let table_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
        [],
        |row| row.get(0),
    )?;
    let was_empty = table_count == 0;

    if was_empty {
        conn.execute_batch(INITIAL_SCHEMA)
            .context("applying initial Target Scheduler schema")?;
    }

    let version = user_version(conn)?;
    if version > TS_SCHEMA_VERSION {
        bail!(
            "database schema version {} is newer than the vendored Target \
             Scheduler schema ({}) — update PSF Guard's vendored schema first",
            version,
            TS_SCHEMA_VERSION
        );
    }
    if !was_empty && version < 22 {
        bail!(
            "database is at schema version {} — migrating an existing \
             pre-v22 Target Scheduler database requires data repairs \
             (gradingStatus remap, GUID backfill) that only the plugin \
             performs. Open it in a recent N.I.N.A. + Target Scheduler to \
             upgrade it first.",
            version
        );
    }

    for (idx, script) in MIGRATIONS.iter().enumerate() {
        let number = (idx + 1) as i64;
        if number <= version {
            continue;
        }
        // Each migration is one transaction, mirroring upstream. The script
        // itself sets `PRAGMA user_version = number` as its last statement.
        let batch = format!("BEGIN;\n{}\nCOMMIT;", script);
        conn.execute_batch(&batch)
            .with_context(|| format!("applying Target Scheduler migration {}", number))?;
        let now = user_version(conn)?;
        if now != number {
            bail!(
                "migration {} left user_version at {} (expected {})",
                number,
                now,
                number
            );
        }
    }

    Ok(())
}

fn user_version(conn: &Connection) -> Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("reading PRAGMA user_version")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        apply_schema(&conn).unwrap();
        conn
    }

    fn table_columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({})", table))
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn fresh_db_lands_on_vendored_version() {
        let conn = fresh_conn();
        assert_eq!(user_version(&conn).unwrap(), TS_SCHEMA_VERSION);
    }

    #[test]
    fn fresh_db_has_all_tables() {
        let conn = fresh_conn();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for expected in [
            "acquiredimage",
            "exposureplan",
            "exposuretemplate",
            "filtercadenceitem",
            "flathistory",
            "imagedata",
            "overrideexposureorderitem",
            "profilepreference",
            "project",
            "ruleweight",
            "target",
        ] {
            assert!(tables.iter().any(|t| t == expected), "missing {expected}");
        }
    }

    /// Golden shape check for the columns PSF Guard (and TS v22+ features)
    /// relies on. Guards against drift when re-vendoring a newer snapshot.
    #[test]
    fn golden_columns_present() {
        let conn = fresh_conn();

        let acquired = table_columns(&conn, "acquiredimage");
        for col in [
            "Id",
            "projectId",
            "targetId",
            "acquireddate",
            "filtername",
            "gradingStatus", // renamed from `accepted` by migration 17
            "metadata",
            "rejectreason",
            "profileId",
            "exposureId",
            "guid",
        ] {
            assert!(acquired.iter().any(|c| c == col), "acquiredimage.{col}");
        }
        // Migration 17 renamed this away; its presence means replay went wrong.
        assert!(!acquired.iter().any(|c| c == "accepted"));

        let project = table_columns(&conn, "project");
        for col in ["Id", "profileId", "name", "state", "isMosaic", "guid"] {
            assert!(project.iter().any(|c| c == col), "project.{col}");
        }
        // Migration 1 dropped these.
        assert!(!project.iter().any(|c| c == "startdate" || c == "enddate"));

        let target = table_columns(&conn, "target");
        for col in ["Id", "name", "ra", "dec", "epochcode", "projectid", "guid"] {
            assert!(target.iter().any(|c| c == col), "target.{col}");
        }

        for (table, col) in [
            ("exposureplan", "guid"),
            ("exposureplan", "enabled"),
            ("exposuretemplate", "guid"),
            ("exposuretemplate", "ditherevery"),
            ("profilepreference", "guid"),
            ("profilepreference", "apiPort"),
        ] {
            assert!(
                table_columns(&conn, table).iter().any(|c| c == col),
                "{table}.{col}"
            );
        }
    }

    #[test]
    fn apply_schema_is_idempotent() {
        let conn = fresh_conn();
        apply_schema(&conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), TS_SCHEMA_VERSION);
    }

    #[test]
    fn populated_pre_v22_db_is_refused() {
        // A TS4-era database (schema present, user_version < 22) needs the
        // plugin's data repairs; SQL-only replay must refuse it.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(INITIAL_SCHEMA).unwrap();
        conn.pragma_update(None, "user_version", 16).unwrap();
        let err = apply_schema(&conn).unwrap_err().to_string();
        assert!(err.contains("pre-v22"), "unexpected error: {err}");
    }

    #[test]
    fn v22_db_is_upgraded_in_place() {
        // v22 → v23 is pure SQL (profilepreference API columns); replaying
        // it on an existing database is safe and expected.
        let conn = Connection::open_in_memory().unwrap();
        apply_schema(&conn).unwrap();
        conn.execute_batch(
            "ALTER TABLE profilepreference DROP COLUMN enableAPI;
             ALTER TABLE profilepreference DROP COLUMN apiPort;
             ALTER TABLE profilepreference DROP COLUMN apiPrettyPrint;",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 22).unwrap();
        apply_schema(&conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), TS_SCHEMA_VERSION);
        assert!(table_columns(&conn, "profilepreference")
            .iter()
            .any(|c| c == "apiPort"));
    }

    #[test]
    fn newer_db_is_refused() {
        let conn = fresh_conn();
        conn.pragma_update(None, "user_version", TS_SCHEMA_VERSION + 1)
            .unwrap();
        let err = apply_schema(&conn).unwrap_err().to_string();
        assert!(err.contains("newer"), "unexpected error: {err}");
    }

    #[test]
    fn created_db_passes_guid_guards() {
        let conn = fresh_conn();
        // Mirrors sync's require_target_scheduler_guid / require_pull_capable.
        for table in [
            "acquiredimage",
            "project",
            "target",
            "exposureplan",
            "exposuretemplate",
        ] {
            assert!(
                table_columns(&conn, table)
                    .iter()
                    .any(|c| c.eq_ignore_ascii_case("guid")),
                "{table} missing guid"
            );
        }
    }

    #[test]
    fn new_guid_is_dotnet_shaped() {
        let g = new_guid();
        assert_eq!(g.len(), 36);
        assert_eq!(g, g.to_lowercase());
        assert_eq!(g.matches('-').count(), 4);
    }

    #[test]
    fn create_fresh_db_refuses_existing_file() {
        let dir = std::env::temp_dir().join(format!("psfguard-ts-schema-{}", new_guid()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("existing.sqlite");
        std::fs::write(&path, b"not empty").unwrap();
        let err = create_fresh_db(&path).unwrap_err().to_string();
        assert!(err.contains("already exists"), "unexpected error: {err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
