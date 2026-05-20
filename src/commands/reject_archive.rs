//! Out-of-tree reject archive support.
//!
//! Owns the `psf_guard_archive` sibling table inside the Target Scheduler
//! database. Each row records that psf-guard moved a rejected FITS file
//! (and its same-stem sidecars) out of the tree PixInsight loads in bulk,
//! keyed on the upstream `acquiredimage.guid` so it stays joinable across
//! TS exports/reimports.
//!
//! The plan, history, and design rationale live in
//! [REJECT_ARCHIVE_PLAN.md](../../../REJECT_ARCHIVE_PLAN.md). Phase A1 is
//! this module: schema bootstrap + read helpers + a schema-version guard.
//! Subsequent phases add destination computation (A3), sidecar discovery
//! (A4), the `move-rejects` CLI handler (A5), and an integration test (A7).

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::db::SchemaCapabilities;

/// `psf-guard`'s view of a single archived rejected image. One row per
/// `acquired_image_guid` (upstream's stable cross-tool join key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveRecord {
    pub acquired_image_guid: String,
    pub acquired_image_id: i64,
    /// Unix seconds (UTC) at which the move was committed to the DB.
    pub moved_at: i64,
    pub original_path: String,
    pub archive_path: String,
    /// Folder segment inserted between project and the rest of the path.
    /// Recorded so a future `restore-rejects` can rebuild the move plan
    /// even if the per-DB config changed since the move ran.
    pub segment_name: String,
    /// Depth at which `segment_name` was inserted. Same rationale.
    pub archive_depth: u32,
    /// Sidecar filenames (basename only, relative to the archive directory)
    /// that travelled alongside the primary. Serialized as a JSON array of
    /// strings in storage; deserialized eagerly here for ergonomics.
    pub sidecar_files: Vec<String>,
    /// Which registry slug owns this DB at the time of the move. Optional
    /// for forward-compatibility with non-multi-DB callers; in practice
    /// the v1 CLI always populates it.
    pub source_db_slug: Option<String>,
}

/// Create the archive table + index if they don't already exist.
///
/// Safe to call repeatedly — the statements are `IF NOT EXISTS`. Schema is
/// owned by psf-guard; no migrations from upstream Target Scheduler touch
/// it. See REJECT_ARCHIVE_PLAN.md §4.4 for the rationale.
pub fn ensure_archive_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS psf_guard_archive (
            acquired_image_guid TEXT PRIMARY KEY,
            acquired_image_id   INTEGER NOT NULL,
            moved_at            INTEGER NOT NULL,
            original_path       TEXT NOT NULL,
            archive_path        TEXT NOT NULL,
            segment_name        TEXT NOT NULL,
            archive_depth       INTEGER NOT NULL,
            sidecar_files       TEXT NOT NULL DEFAULT '[]',
            source_db_slug      TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_psf_guard_archive_image_id
            ON psf_guard_archive(acquired_image_id);
        "#,
    )
    .context("creating psf_guard_archive table")?;
    Ok(())
}

/// Refuse to operate against a Target Scheduler database that pre-dates the
/// `acquiredimage.guid` column (migration 22). Without `guid`, we have no
/// stable cross-export key to anchor archive rows against; falling back to
/// `Id` would silently desync after any TS DB export/reimport.
///
/// The error message is user-facing — keep it actionable.
pub fn require_target_scheduler_guid(conn: &Connection) -> Result<()> {
    let caps = SchemaCapabilities::detect(conn);
    if !caps.has_acquiredimage_guid {
        return Err(anyhow::anyhow!(
            "This Target Scheduler database is too old: it lacks the \
             `acquiredimage.guid` column (added in plugin schema v22) which \
             psf-guard's reject-archive feature uses to track moves across \
             DB exports/reimports.\n\nUpgrade by opening the database in a \
             recent N.I.N.A. + Target Scheduler version, or run earlier \
             psf-guard commands (`filter-rejected`) that don't need it."
        ));
    }
    Ok(())
}

/// Look up the archive record for an image by its TS guid. Returns
/// `Ok(None)` if the image was never archived by psf-guard.
pub fn get_archive_record_by_guid(conn: &Connection, guid: &str) -> Result<Option<ArchiveRecord>> {
    conn.query_row(
        "SELECT acquired_image_guid, acquired_image_id, moved_at,
                original_path, archive_path, segment_name, archive_depth,
                sidecar_files, source_db_slug
         FROM psf_guard_archive
         WHERE acquired_image_guid = ?1",
        params![guid],
        row_to_record,
    )
    .optional()
    .context("querying psf_guard_archive by guid")
}

/// Look up the archive record by the TS internal `acquiredimage.Id`. Slightly
/// less stable than guid (auto-increment IDs renumber on export/reimport)
/// but useful for in-process callers that already have the row id from a
/// query.
pub fn get_archive_record_by_image_id(
    conn: &Connection,
    image_id: i64,
) -> Result<Option<ArchiveRecord>> {
    conn.query_row(
        "SELECT acquired_image_guid, acquired_image_id, moved_at,
                original_path, archive_path, segment_name, archive_depth,
                sidecar_files, source_db_slug
         FROM psf_guard_archive
         WHERE acquired_image_id = ?1",
        params![image_id],
        row_to_record,
    )
    .optional()
    .context("querying psf_guard_archive by image_id")
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArchiveRecord> {
    let sidecar_raw: String = row.get("sidecar_files")?;
    let sidecar_files = serde_json::from_str::<Vec<String>>(&sidecar_raw).unwrap_or_default();
    Ok(ArchiveRecord {
        acquired_image_guid: row.get("acquired_image_guid")?,
        acquired_image_id: row.get("acquired_image_id")?,
        moved_at: row.get("moved_at")?,
        original_path: row.get("original_path")?,
        archive_path: row.get("archive_path")?,
        segment_name: row.get("segment_name")?,
        archive_depth: row.get::<_, i64>("archive_depth")? as u32,
        sidecar_files,
        source_db_slug: row.get("source_db_slug")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_with_acquiredimage(guid: bool) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        let guid_col = if guid { ", guid TEXT" } else { "" };
        conn.execute_batch(&format!(
            "CREATE TABLE acquiredimage (
                Id INTEGER PRIMARY KEY,
                projectId INTEGER NOT NULL,
                targetId INTEGER NOT NULL,
                gradingStatus INTEGER NOT NULL DEFAULT 0,
                metadata TEXT NOT NULL DEFAULT '{{}}'{guid_col}
            );",
        ))
        .unwrap();
        conn
    }

    #[test]
    fn ensure_archive_schema_is_idempotent_and_creates_index() {
        let conn = open_with_acquiredimage(true);
        // Call twice; second call must not error.
        ensure_archive_schema(&conn).unwrap();
        ensure_archive_schema(&conn).unwrap();

        // Table exists.
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='table' AND name='psf_guard_archive'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 1);

        // Index exists.
        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='index' AND name='idx_psf_guard_archive_image_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 1);
    }

    #[test]
    fn require_target_scheduler_guid_errors_without_column() {
        let conn = open_with_acquiredimage(false);
        let err = require_target_scheduler_guid(&conn).unwrap_err();
        let msg = format!("{err}");
        // Both keywords appear in the message so users grepping logs can find it.
        assert!(msg.contains("guid"), "msg should mention guid: {msg}");
        assert!(
            msg.contains("v22") || msg.contains("22"),
            "msg should mention schema version: {msg}"
        );
    }

    #[test]
    fn require_target_scheduler_guid_passes_with_column() {
        let conn = open_with_acquiredimage(true);
        require_target_scheduler_guid(&conn).unwrap();
    }

    #[test]
    fn lookup_returns_none_when_no_row() {
        let conn = open_with_acquiredimage(true);
        ensure_archive_schema(&conn).unwrap();
        assert!(get_archive_record_by_guid(&conn, "nope").unwrap().is_none());
        assert!(get_archive_record_by_image_id(&conn, 999)
            .unwrap()
            .is_none());
    }

    #[test]
    fn lookup_returns_record_after_insert() {
        let conn = open_with_acquiredimage(true);
        ensure_archive_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO psf_guard_archive
             (acquired_image_guid, acquired_image_id, moved_at,
              original_path, archive_path, segment_name, archive_depth,
              sidecar_files, source_db_slug)
             VALUES ('abc', 42, 1700000000,
                     '/src/img.fits', '/src/REJECT/img.fits',
                     'REJECT', 1, '[\"img.xisf\"]', 'imaging-rig')",
            [],
        )
        .unwrap();

        let by_guid = get_archive_record_by_guid(&conn, "abc").unwrap().unwrap();
        assert_eq!(by_guid.acquired_image_id, 42);
        assert_eq!(by_guid.moved_at, 1700000000);
        assert_eq!(by_guid.original_path, "/src/img.fits");
        assert_eq!(by_guid.archive_path, "/src/REJECT/img.fits");
        assert_eq!(by_guid.segment_name, "REJECT");
        assert_eq!(by_guid.archive_depth, 1);
        assert_eq!(by_guid.sidecar_files, vec!["img.xisf"]);
        assert_eq!(by_guid.source_db_slug.as_deref(), Some("imaging-rig"));

        let by_id = get_archive_record_by_image_id(&conn, 42).unwrap().unwrap();
        assert_eq!(by_id, by_guid);
    }

    #[test]
    fn corrupt_sidecar_json_falls_back_to_empty() {
        let conn = open_with_acquiredimage(true);
        ensure_archive_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO psf_guard_archive
             (acquired_image_guid, acquired_image_id, moved_at,
              original_path, archive_path, segment_name, archive_depth,
              sidecar_files)
             VALUES ('x', 1, 0, '/o', '/a', 'REJECT', 1, 'not-json')",
            [],
        )
        .unwrap();
        let r = get_archive_record_by_guid(&conn, "x").unwrap().unwrap();
        assert!(r.sidecar_files.is_empty());
    }
}
