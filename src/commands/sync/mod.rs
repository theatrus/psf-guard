//! Two-database sync between N.I.N.A. Target Scheduler databases.
//!
//! - [`sync_grades`] pushes grading state (our DB → telescope), matched by
//!   `acquiredimage.guid`.
//! - [`sync_pull`] pulls structure + captured images (telescope → our DB),
//!   matched by guid with FK remapping, preserving local grading work.
//! - [`sync_planning`] pushes project, target, template, plan, and rule settings
//!   back to the telescope without changing capture history or grades.
//!
//! All three cores are pure DB logic; CLI glue (argument resolution,
//! connection opening, reporting) lives in `cli_main.rs`. Shared helpers —
//! `--from`/`--to` resolution, status parsing, the v22 capability guard — live
//! here in the module root.

mod grades;
mod planning;
mod pull;

pub use grades::{sync_grades, GradeChange, SyncGradesOptions, SyncSummary};
pub use planning::{sync_planning, PlanningOptions, PlanningSummary};
pub use pull::{sync_pull, PullOptions, PullSummary, TableCounts};

use crate::db_registry::DbRegistry;
use crate::models::GradingStatus;
use anyhow::{anyhow, Result};
use rusqlite::{Connection, OptionalExtension};
use std::path::PathBuf;

/// Parse a grading-status filter string (`pending|accepted|rejected`).
pub fn parse_status(s: &str) -> Result<GradingStatus> {
    match s.to_lowercase().as_str() {
        "pending" => Ok(GradingStatus::Pending),
        "accepted" => Ok(GradingStatus::Accepted),
        "rejected" => Ok(GradingStatus::Rejected),
        other => Err(anyhow!(
            "Invalid status '{}'. Use pending, accepted, or rejected",
            other
        )),
    }
}

/// Resolve a `--from`/`--to` argument to a database file path. Prefers a
/// registry slug match (when a registry is available); otherwise treats the
/// argument as a path to an existing `.sqlite` file. Image directories are
/// irrelevant for sync, so raw paths need no registry entry.
pub fn resolve_db_path(registry: Option<&DbRegistry>, arg: &str) -> Result<PathBuf> {
    if let Some(reg) = registry
        && let Some(entry) = reg.find(arg)
    {
        return Ok(PathBuf::from(&entry.db_path));
    }
    let path = PathBuf::from(arg);
    if path.is_file() {
        return Ok(path);
    }
    Err(anyhow!(
        "Could not resolve '{}': not a known registry slug and not an existing file path",
        arg
    ))
}

/// Core entity tables that must carry a `guid` column (TS plugin schema v22+)
/// for `sync pull` to match rows across databases.
pub const PULL_GUID_TABLES: &[&str] = &[
    "project",
    "target",
    "exposuretemplate",
    "exposureplan",
    "acquiredimage",
];

/// Planning tables that must carry stable guids. Unlike a full pull, a
/// planning push never reads or writes acquired images.
pub const PLANNING_GUID_TABLES: &[&str] =
    &["project", "target", "exposuretemplate", "exposureplan"];

/// Return true if `table` has a column named `column` (case-insensitive).
pub fn table_has_column(conn: &Connection, table: &str, column: &str) -> bool {
    // PRAGMA table_info can't be parameterized; table names here are constants.
    conn.query_row(
        &format!(
            "SELECT 1 FROM pragma_table_info('{}') WHERE lower(name) = lower(?1)",
            table
        ),
        [column],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

/// Refuse to operate against a database that lacks `guid` columns on the core
/// entity tables (added in TS plugin schema v22). `sync pull` matches rows by
/// guid, so without it we cannot reliably remap entities across databases.
pub fn require_pull_capable(conn: &Connection) -> Result<()> {
    require_guid_tables(conn, PULL_GUID_TABLES, "sync pull")
}

/// Refuse planning sync when scheduler structure lacks stable guids.
pub fn require_planning_capable(conn: &Connection) -> Result<()> {
    require_guid_tables(conn, PLANNING_GUID_TABLES, "sync planning")
}

fn require_guid_tables(conn: &Connection, tables: &[&'static str], operation: &str) -> Result<()> {
    let missing: Vec<&str> = tables
        .iter()
        .copied()
        .filter(|t| !table_has_column(conn, t, "guid"))
        .collect();
    if !missing.is_empty() {
        return Err(anyhow!(
            "This Target Scheduler database is missing the `guid` column on: {} \
             (added in plugin schema v22). `{}` matches rows by guid to \
             remap entities across databases.\n\nUpgrade by opening the database \
             in a recent N.I.N.A. + Target Scheduler version.",
            missing.join(", "),
            operation,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_db_path_prefers_path_when_no_registry() {
        // A non-existent arg with no registry errors out.
        assert!(resolve_db_path(None, "definitely-not-a-real-file.sqlite").is_err());
    }

    #[test]
    fn table_has_column_detects_guid() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE project (Id INTEGER, name TEXT, guid TEXT);")
            .unwrap();
        assert!(table_has_column(&conn, "project", "guid"));
        assert!(table_has_column(&conn, "project", "GUID")); // case-insensitive
        assert!(!table_has_column(&conn, "project", "nope"));
    }
}
