//! Project removal: delete a project from a catalog, with its targets,
//! exposure plans and frame rows, and the PSF Guard records that refer to
//! them. Image files stay where they are. See
//! docs/design/project-removal.md.
//!
//! A removal is a [`plan`] and then a [`remove`] of that plan, which checks
//! the plan's digest inside its transaction so a frame captured after the
//! preview is never removed unseen. Tombstones keep the project out of
//! later syncs and its files out of later imports. A sync from a catalog
//! that deleted a project deletes it here too, through
//! [`delete_project_rows`], when nothing was captured for it here.

use crate::commands::reject_archive;
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS psf_guard_removed_project (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL,
    project_guid TEXT,
    name TEXT NOT NULL,
    removed_at INTEGER NOT NULL,
    frames INTEGER NOT NULL,
    allowed_back_at INTEGER,
    left_out_at INTEGER,
    origin TEXT NOT NULL DEFAULT 'local',
    left_out_source_frames TEXT
);
CREATE INDEX IF NOT EXISTS idx_psf_guard_removed_project_id
    ON psf_guard_removed_project(project_id);
CREATE TABLE IF NOT EXISTS psf_guard_removed_project_frame (
    removal_id INTEGER NOT NULL,
    acquired_image_id INTEGER NOT NULL,
    acquired_image_guid TEXT,
    project_id INTEGER NOT NULL,
    project_guid TEXT,
    file_name TEXT,
    removed_at INTEGER NOT NULL,
    acquired_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_psf_guard_removed_project_frame_guid
    ON psf_guard_removed_project_frame(acquired_image_guid);
CREATE INDEX IF NOT EXISTS idx_psf_guard_removed_project_frame_removal
    ON psf_guard_removed_project_frame(removal_id)";

/// Create the tombstone tables. Safe to call repeatedly.
pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)
        .context("creating the removed project tables")?;
    // Tables from before these columns were added lack them.
    for (table, column, definition) in [
        ("psf_guard_removed_project", "left_out_at", "INTEGER"),
        (
            "psf_guard_removed_project",
            "origin",
            "TEXT NOT NULL DEFAULT 'local'",
        ),
        (
            "psf_guard_removed_project",
            "left_out_source_frames",
            "TEXT",
        ),
        ("psf_guard_removed_project_frame", "acquired_at", "INTEGER"),
    ] {
        if !has_column(conn, table, column) {
            conn.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                [],
            )
            .with_context(|| format!("adding {table}.{column}"))?;
        }
    }
    Ok(())
}

/// Seconds since the Unix epoch.
pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    conn.prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .and_then(|mut statement| {
            let names = statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(names.iter().any(|name| name.eq_ignore_ascii_case(column)))
        })
        .unwrap_or(false)
}

/// GUIDs of deleted projects not allowed back, lower case, so a catalog
/// pull leaves them and everything under them out. Empty when no project
/// was ever deleted.
pub fn removed_project_guids(conn: &Connection) -> Result<BTreeSet<String>> {
    if !table_exists(conn, "psf_guard_removed_project") {
        return Ok(BTreeSet::new());
    }
    let mut statement = conn.prepare(
        "SELECT lower(project_guid) FROM psf_guard_removed_project
         WHERE project_guid IS NOT NULL AND project_guid <> '' AND allowed_back_at IS NULL",
    )?;
    let guids = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(guids)
}

/// Whether the project with this Id was deleted at or after `since` (Unix
/// seconds), or its row is gone. Target Scheduler may give a deleted
/// project's Id to a new project, so a deletion before `since` belongs to
/// an earlier project and does not count.
pub fn project_removed_since(conn: &Connection, project_id: i64, since: i64) -> Result<bool> {
    let present: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM project WHERE Id = ?1)",
        [project_id],
        |row| row.get(0),
    )?;
    if !present {
        return Ok(true);
    }
    if !table_exists(conn, "psf_guard_removed_project") {
        return Ok(false);
    }
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM psf_guard_removed_project
                       WHERE project_id = ?1 AND removed_at >= ?2)",
        params![project_id, since],
        |row| row.get(0),
    )?)
}

/// A deleted project, as the Library lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Deletion {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub removed_at: i64,
    pub frames: i64,
}

/// Deleted projects that syncs and imports still leave out, newest first.
/// A project deleted without frames is listed only once a sync has left it
/// out: until then, keeping it out changes nothing anyone would see.
pub fn deletions(conn: &Connection) -> Result<Vec<Deletion>> {
    if !table_exists(conn, "psf_guard_removed_project") {
        return Ok(Vec::new());
    }
    // A table from before the column lists the deletions with frames.
    let left_out = if has_column(conn, "psf_guard_removed_project", "left_out_at") {
        "OR left_out_at IS NOT NULL"
    } else {
        ""
    };
    let mut statement = conn.prepare(&format!(
        "SELECT id, project_id, name, removed_at, frames FROM psf_guard_removed_project
         WHERE allowed_back_at IS NULL AND (frames > 0 {left_out})
         ORDER BY removed_at DESC, id DESC"
    ))?;
    let deletions = statement
        .query_map([], |row| {
            Ok(Deletion {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                removed_at: row.get(3)?,
                frames: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(deletions)
}

/// Let syncs and imports add a deleted project back: the deletion is dated
/// allowed back and its frames' tombstones go, so the next sync or import
/// of its files adds it as a new project. The row keeps its GUID and
/// dates: a sync passes them on, so a catalog that deleted the project
/// after this one allows it back too, and one that deleted it before does
/// not delete it here again. The deletion's date also keeps Restore in
/// reject removal from putting a reject removed before it into a project
/// that took its Id. Returns false when there is no such deletion.
pub fn allow_back(conn: &Connection, id: i64, now: i64) -> Result<bool> {
    if !table_exists(conn, "psf_guard_removed_project") {
        return Ok(false);
    }
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let allowed = allow_removal_back(&tx, id, now)?;
    tx.commit()?;
    Ok(allowed)
}

/// Date one standing deletion allowed back and delete its frame
/// tombstones. Returns false when it was already allowed back.
fn allow_removal_back(conn: &Connection, id: i64, allowed_back_at: i64) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE psf_guard_removed_project SET allowed_back_at = ?2
         WHERE id = ?1 AND allowed_back_at IS NULL",
        params![id, allowed_back_at],
    )?;
    if changed == 0 {
        return Ok(false);
    }
    conn.execute(
        "DELETE FROM psf_guard_removed_project_frame WHERE removal_id = ?1",
        [id],
    )?;
    Ok(true)
}

/// Follow another catalog's Allow again: each standing deletion here of
/// the project with this GUID (lower case) that a sync recorded (`origin`
/// `sync`), dated before `allowed_back_at`, is allowed back as of that
/// date. A deletion a person made in this catalog stands until Allow again
/// here, and so does a deletion dated later. Returns how many deletions
/// were allowed back.
pub(crate) fn follow_allow_back(
    conn: &Connection,
    project_guid: &str,
    allowed_back_at: i64,
) -> Result<usize> {
    if !table_exists(conn, "psf_guard_removed_project") {
        return Ok(0);
    }
    // A table from before `origin` holds only this catalog's deletions.
    if !has_column(conn, "psf_guard_removed_project", "origin") {
        return Ok(0);
    }
    let mut statement = conn.prepare(
        "SELECT id FROM psf_guard_removed_project
         WHERE lower(project_guid) = ?1 AND allowed_back_at IS NULL AND removed_at < ?2
           AND origin = 'sync'",
    )?;
    let ids = statement
        .query_map(params![project_guid, allowed_back_at], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut allowed = 0;
    for id in ids {
        if allow_removal_back(conn, id, allowed_back_at)? {
            allowed += 1;
        }
    }
    Ok(allowed)
}

/// Whether this catalog allowed the project with this GUID (lower case)
/// back after `removed_at`, the date another catalog deleted it. Such a
/// deletion is older than the choice made here to keep the project. With
/// no date, any Allow again here counts.
pub(crate) fn allowed_back_after(
    conn: &Connection,
    project_guid: &str,
    removed_at: Option<i64>,
) -> Result<bool> {
    if !table_exists(conn, "psf_guard_removed_project") {
        return Ok(false);
    }
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM psf_guard_removed_project
                       WHERE lower(project_guid) = ?1 AND allowed_back_at IS NOT NULL
                         AND (?2 IS NULL OR allowed_back_at > ?2))",
        params![project_guid, removed_at],
        |row| row.get(0),
    )?)
}

/// Record that a sync left out the deleted project with this GUID (lower
/// case). From then on, Deleted projects lists it even when it had no
/// frames. The first date stays.
pub(crate) fn mark_left_out(conn: &Connection, project_guid: &str, now: i64) -> Result<()> {
    ensure_schema(conn)?;
    conn.execute(
        "UPDATE psf_guard_removed_project SET left_out_at = ?2
         WHERE lower(project_guid) = ?1 AND allowed_back_at IS NULL AND left_out_at IS NULL",
        params![project_guid, now],
    )?;
    Ok(())
}

/// A frame a sync left out with its deleted project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LeftOutFrame {
    pub guid: Option<String>,
    /// Lower-case file name, without its folder.
    pub file_name: String,
    /// Capture time, Unix seconds.
    pub acquired_at: Option<i64>,
}

/// Add the frames a sync left out with the deleted project that has this
/// GUID (lower case) to its frame tombstones, so an upload or import of
/// their files is skipped as removed. `signature` describes the source's
/// frames for the project (count and highest Id); when it matches the last
/// sync's, nothing is read again. A frame already listed, by GUID or file
/// name, is not added again. Returns how many were added.
pub(crate) fn add_left_out_frames(
    conn: &Connection,
    project_guid: &str,
    signature: &str,
    frames: impl FnOnce() -> Result<Vec<LeftOutFrame>>,
    now: i64,
) -> Result<usize> {
    let removal: Option<(i64, i64, Option<String>)> = conn
        .query_row(
            "SELECT id, project_id, left_out_source_frames FROM psf_guard_removed_project
             WHERE lower(project_guid) = ?1 AND allowed_back_at IS NULL
             ORDER BY removed_at DESC, id DESC LIMIT 1",
            [project_guid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((removal, project_id, seen)) = removal else {
        return Ok(0);
    };
    if seen.as_deref() == Some(signature) {
        return Ok(0);
    }
    let frames = frames()?;
    let mut guids = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    {
        let mut listed = conn.prepare(
            "SELECT acquired_image_guid, file_name FROM psf_guard_removed_project_frame
             WHERE removal_id = ?1",
        )?;
        for row in listed.query_map([removal], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })? {
            let (guid, name) = row?;
            guids.extend(guid);
            names.extend(name);
        }
    }
    // Acquired image Id 0: the frame never had a row in this catalog.
    let mut insert = conn.prepare(
        "INSERT INTO psf_guard_removed_project_frame
            (removal_id, acquired_image_id, acquired_image_guid, project_id, project_guid,
             file_name, removed_at, acquired_at)
         VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    let mut added = 0;
    for frame in frames {
        let known = frame.guid.as_ref().is_some_and(|guid| guids.contains(guid))
            || names.contains(&frame.file_name);
        if known {
            continue;
        }
        insert.execute(params![
            removal,
            frame.guid,
            project_id,
            project_guid,
            frame.file_name,
            now,
            frame.acquired_at
        ])?;
        guids.extend(frame.guid);
        names.insert(frame.file_name);
        added += 1;
    }
    conn.execute(
        "UPDATE psf_guard_removed_project SET left_out_source_frames = ?2 WHERE id = ?1",
        params![removal, signature],
    )?;
    Ok(added)
}

/// What another catalog's tombstones say about one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceDeletionState {
    /// Deleted there, and not allowed back. The date is when, if known.
    Standing { removed_at: Option<i64> },
    /// Deleted there once, then allowed back on this date.
    AllowedBack { allowed_back_at: i64 },
}

/// A project another catalog deleted, from that catalog's tombstones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceDeletion {
    /// Lower case.
    pub guid: String,
    pub name: String,
    pub state: SourceDeletionState,
}

/// What a sync source's tombstones say about each project it deleted. A
/// peer's bundle carries `project_guid`, `name`, `removed_at` and
/// `allowed_back_at`; a local catalog's table has every column. A project
/// deleted more than once is standing if any deletion stands, and
/// otherwise allowed back as of its latest Allow again.
pub(crate) fn source_deletions(src: &Connection) -> Result<Vec<SourceDeletion>> {
    let table = "psf_guard_removed_project";
    if !table_exists(src, table) || !has_column(src, table, "project_guid") {
        return Ok(Vec::new());
    }
    let column = |name: &'static str| {
        if has_column(src, table, name) {
            name
        } else {
            "NULL"
        }
    };
    let mut statement = src.prepare(&format!(
        "SELECT lower(trim(project_guid)), {}, {}, {} FROM {table}
         WHERE project_guid IS NOT NULL AND trim(project_guid) <> ''",
        column("name"),
        column("removed_at"),
        column("allowed_back_at"),
    ))?;
    let mut by_guid: std::collections::BTreeMap<String, SourceDeletion> = Default::default();
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, Option<i64>>(3)?,
        ))
    })? {
        let (guid, name, removed_at, allowed_back_at) = row?;
        let state = match allowed_back_at {
            None => SourceDeletionState::Standing { removed_at },
            Some(allowed_back_at) => SourceDeletionState::AllowedBack { allowed_back_at },
        };
        let entry = by_guid.entry(guid.clone()).or_insert(SourceDeletion {
            guid,
            name: name.clone().unwrap_or_default(),
            state: state.clone(),
        });
        entry.state = match (&entry.state, state) {
            (
                SourceDeletionState::Standing { removed_at: kept },
                SourceDeletionState::Standing { removed_at },
            ) => SourceDeletionState::Standing {
                removed_at: (*kept).max(removed_at),
            },
            (SourceDeletionState::Standing { .. }, SourceDeletionState::AllowedBack { .. }) => {
                entry.state.clone()
            }
            (
                SourceDeletionState::AllowedBack { .. },
                standing @ SourceDeletionState::Standing { .. },
            ) => {
                if let Some(name) = name {
                    entry.name = name;
                }
                standing
            }
            (
                SourceDeletionState::AllowedBack {
                    allowed_back_at: kept,
                },
                SourceDeletionState::AllowedBack { allowed_back_at },
            ) => SourceDeletionState::AllowedBack {
                allowed_back_at: (*kept).max(allowed_back_at),
            },
        };
    }
    Ok(by_guid.into_values().collect())
}

/// Rejects removed from the project, or from its targets, that Restore can
/// still bring back: their saved rows were not purged and their files are
/// not deleted.
pub(crate) fn restorable_rejects(conn: &Connection, project_id: i64) -> Result<usize> {
    if !table_exists(conn, "psf_guard_removed_image") {
        return Ok(0);
    }
    let mut kept = Vec::new();
    for column in ["purged_at", "files_deleted_at"] {
        if has_column(conn, "psf_guard_removed_image", column) {
            kept.push(format!("AND {column} IS NULL"));
        }
    }
    Ok(conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM psf_guard_removed_image
             WHERE (project_id = ?1 OR target_id IN (SELECT Id FROM target WHERE projectid = ?1))
             {}",
            kept.join(" ")
        ),
        [project_id],
        |row| row.get::<_, i64>(0),
    )? as usize)
}

/// Removed rejects of the project, or of its targets, whose GUIDs are
/// among `reported`: frames reported to a collaboration.
pub(crate) fn reported_rejects(
    conn: &Connection,
    project_id: i64,
    reported: &BTreeSet<String>,
) -> Result<usize> {
    if reported.is_empty() || !table_exists(conn, "psf_guard_removed_image") {
        return Ok(0);
    }
    let mut statement = conn.prepare(
        "SELECT acquired_image_guid FROM psf_guard_removed_image
         WHERE project_id = ?1 OR target_id IN (SELECT Id FROM target WHERE projectid = ?1)",
    )?;
    let mut count = 0;
    for guid in statement.query_map([project_id], |row| row.get::<_, String>(0))? {
        if reported.contains(&guid?) {
            count += 1;
        }
    }
    Ok(count)
}

/// Record, in a PSF Guard catalog, a project a sync deleted because its
/// source catalog deleted it, so a later sync from a rig does not bring it
/// back. Nothing was captured for it here, so Deleted projects does not
/// list it until a sync leaves it out. The row is dated `removed_at`, when
/// the person deleted the project in the catalog it came from, so every
/// catalog compares Allow again with the same date.
pub(crate) fn record_sync_deletion(
    conn: &Connection,
    plan: &RemovalPlan,
    removed_at: i64,
) -> Result<()> {
    ensure_schema(conn)?;
    conn.execute(
        "INSERT INTO psf_guard_removed_project
            (project_id, project_guid, name, removed_at, frames, allowed_back_at, origin)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'sync')",
        params![
            plan.project_id,
            plan.guid.as_deref().filter(|_| plan.unique_guid),
            plan.name,
            removed_at,
            plan.frames.total as i64
        ],
    )?;
    Ok(())
}

/// GUIDs of frames deleted with their project.
pub fn removed_frame_guids(conn: &Connection) -> Result<Vec<String>> {
    if !table_exists(conn, "psf_guard_removed_project_frame") {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT DISTINCT acquired_image_guid FROM psf_guard_removed_project_frame
         WHERE acquired_image_guid IS NOT NULL AND acquired_image_guid <> ''",
    )?;
    let guids = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(guids)
}

/// Lower-case names of the files of frames deleted with their project, the
/// lights and the calibrated copies paired with them, so an import of the
/// files left on disk does not bring the project back.
pub fn removed_file_names(conn: &Connection) -> Result<Vec<String>> {
    if !table_exists(conn, "psf_guard_removed_project_frame") {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT DISTINCT file_name FROM psf_guard_removed_project_frame
         WHERE file_name IS NOT NULL AND file_name <> ''",
    )?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(names)
}

/// The deleted projects, not allowed back, whose frame tombstones list
/// this file name (compared in lower case), with each frame's capture
/// time when it was recorded.
pub fn deleted_project_frames_named(
    conn: &Connection,
    file_name: &str,
) -> Result<Vec<(String, Option<i64>)>> {
    if !table_exists(conn, "psf_guard_removed_project_frame")
        || !table_exists(conn, "psf_guard_removed_project")
    {
        return Ok(Vec::new());
    }
    let acquired_at = if has_column(conn, "psf_guard_removed_project_frame", "acquired_at") {
        "frame.acquired_at"
    } else {
        "NULL"
    };
    let mut statement = conn.prepare(&format!(
        "SELECT removal.name, {acquired_at} FROM psf_guard_removed_project_frame frame
         JOIN psf_guard_removed_project removal ON removal.id = frame.removal_id
         WHERE frame.file_name = lower(?1) AND removal.allowed_back_at IS NULL
         ORDER BY removal.removed_at DESC"
    ))?;
    let frames = statement
        .query_map([file_name], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(frames)
}

/// Frames by grade.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct FrameCounts {
    pub total: usize,
    pub accepted: usize,
    pub rejected: usize,
    pub pending: usize,
}

/// What a removal would delete, as the preview shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemovalPlan {
    pub project_id: i64,
    pub name: String,
    pub guid: Option<String>,
    /// No other project in the catalog has this GUID. A duplicate GUID,
    /// from a damaged catalog, identifies neither project: the deletion
    /// then writes no GUID tombstone and leaves Planning alone.
    #[serde(skip)]
    pub unique_guid: bool,
    /// Target Scheduler's state: 0 draft, 1 active, 2 inactive, 3 closed.
    pub state: Option<i64>,
    pub targets: usize,
    pub exposure_plans: usize,
    pub frames: FrameCounts,
    /// Frames reported to a collaboration's coordinator. A project with any
    /// cannot be deleted: the coordinator counts them.
    pub protected_frames: usize,
    /// GUIDs of the project's targets, for the Planning records keyed by them.
    #[serde(skip)]
    pub target_guids: Vec<String>,
    #[serde(skip)]
    frame_rows: Vec<FrameRow>,
    /// Changes when any target, exposure plan or frame does.
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FrameRow {
    id: i64,
    /// The frame's own project, which its cache keys use: another project's
    /// when the frame was recorded there on one of this project's targets.
    project_id: i64,
    guid: Option<String>,
    target_id: Option<i64>,
    grade: i64,
    file_name: Option<String>,
    /// Capture time, Unix seconds.
    acquired_at: Option<i64>,
}

/// A deleted frame, for the cache files the server deletes after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemovedFrame {
    pub image_id: i64,
    pub project_id: i64,
    pub target_id: Option<i64>,
}

/// How a removal ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Removed {
        plan: RemovalPlan,
        frames: Vec<RemovedFrame>,
    },
    /// The project changed since the preview, so nothing was deleted. The
    /// plan is the new preview.
    Changed(RemovalPlan),
    /// Some frames were reported to a collaboration.
    Protected(RemovalPlan),
    /// No such project.
    Missing,
}

/// What deleting `project_id` would delete, or `None` when there is no such
/// project. `protected` lists the frame GUIDs reported to a collaboration.
pub fn plan(
    conn: &Connection,
    project_id: i64,
    protected: &BTreeSet<String>,
) -> Result<Option<RemovalPlan>> {
    let guid_column = has_column(conn, "project", "guid");
    let state_column = has_column(conn, "project", "state");
    let project = conn
        .query_row(
            &format!(
                "SELECT name, {}, {} FROM project WHERE Id = ?1",
                if guid_column { "guid" } else { "NULL" },
                if state_column { "state" } else { "NULL" },
            ),
            [project_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(1)?
                        .map(|guid| guid.trim().to_string())
                        .filter(|guid| !guid.is_empty()),
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((name, guid, state)) = project else {
        return Ok(None);
    };
    let unique_guid = match &guid {
        Some(guid) => {
            conn.query_row(
                "SELECT COUNT(*) FROM project WHERE lower(trim(guid)) = lower(?1)",
                [guid],
                |row| row.get::<_, i64>(0),
            )? == 1
        }
        None => false,
    };

    let target_guid = if has_column(conn, "target", "guid") {
        "guid"
    } else {
        "NULL"
    };
    let mut statement = conn.prepare(&format!(
        "SELECT Id, {target_guid} FROM target WHERE projectid = ?1 ORDER BY Id"
    ))?;
    let targets = statement
        .query_map([project_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let target_ids: Vec<i64> = targets.iter().map(|(id, _)| *id).collect();
    let target_guids: Vec<String> = targets
        .iter()
        .filter_map(|(_, guid)| guid.clone().filter(|guid| !guid.is_empty()))
        .collect();

    let mut exposure_plan_ids = Vec::new();
    if table_exists(conn, "exposureplan") {
        let mut statement =
            conn.prepare("SELECT Id FROM exposureplan WHERE targetid = ?1 ORDER BY Id")?;
        for target in &target_ids {
            for id in statement.query_map([target], |row| row.get::<_, i64>(0))? {
                exposure_plan_ids.push(id?);
            }
        }
    }

    let frame_guid = if has_column(conn, "acquiredimage", "guid") {
        "guid"
    } else {
        "NULL"
    };
    let acquired_date = if has_column(conn, "acquiredimage", "acquireddate") {
        "acquireddate"
    } else {
        "NULL"
    };
    let mut statement = conn.prepare(&format!(
        "SELECT Id, {frame_guid}, targetId, gradingStatus, metadata, projectId, {acquired_date}
         FROM acquiredimage
         WHERE projectId = ?1 OR targetId IN (SELECT Id FROM target WHERE projectid = ?1)
         ORDER BY Id"
    ))?;
    let frame_rows = statement
        .query_map([project_id], |row| {
            let metadata: Option<String> = row.get(4)?;
            Ok(FrameRow {
                id: row.get(0)?,
                project_id: row.get::<_, Option<i64>>(5)?.unwrap_or(project_id),
                guid: row
                    .get::<_, Option<String>>(1)?
                    .filter(|guid| !guid.is_empty()),
                target_id: row.get(2)?,
                grade: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                file_name: metadata
                    .as_deref()
                    .and_then(reject_archive::parse_filename_from_metadata)
                    .filter(|name| !name.is_empty())
                    .map(|name| name.to_lowercase()),
                acquired_at: row.get::<_, Option<i64>>(6).ok().flatten(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut frames = FrameCounts {
        total: frame_rows.len(),
        ..FrameCounts::default()
    };
    for frame in &frame_rows {
        match frame.grade {
            1 => frames.accepted += 1,
            2 => frames.rejected += 1,
            _ => frames.pending += 1,
        }
    }
    let protected_frames = frame_rows
        .iter()
        .filter(|frame| {
            frame
                .guid
                .as_ref()
                .is_some_and(|guid| protected.contains(guid))
        })
        .count();

    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.update([0]);
    hasher.update(guid.as_deref().unwrap_or_default().as_bytes());
    for (label, ids) in [(b'T', &target_ids), (b'E', &exposure_plan_ids)] {
        hasher.update([label]);
        for id in ids {
            hasher.update(id.to_le_bytes());
        }
    }
    hasher.update(*b"F");
    for frame in &frame_rows {
        hasher.update(frame.id.to_le_bytes());
        hasher.update(frame.guid.as_deref().unwrap_or_default().as_bytes());
        hasher.update([0]);
    }
    let digest = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();

    Ok(Some(RemovalPlan {
        project_id,
        name,
        guid,
        unique_guid,
        state,
        targets: target_ids.len(),
        exposure_plans: exposure_plan_ids.len(),
        frames,
        protected_frames,
        target_guids,
        frame_rows,
        digest,
    }))
}

/// Delete `project_id` as planned with `digest`. One transaction writes the
/// tombstones and deletes every row with [`delete_project_rows`]; a plan
/// that no longer matches deletes nothing.
pub fn remove(
    conn: &Connection,
    project_id: i64,
    digest: &str,
    protected: &BTreeSet<String>,
    now: i64,
) -> Result<Outcome> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let Some(plan) = plan(&tx, project_id, protected)? else {
        return Ok(Outcome::Missing);
    };
    if plan.digest != digest {
        return Ok(Outcome::Changed(plan));
    }
    if plan.protected_frames > 0 {
        return Ok(Outcome::Protected(plan));
    }
    ensure_schema(&tx)?;
    // A GUID shared with another project identifies neither.
    let tombstone_guid = plan.guid.as_deref().filter(|_| plan.unique_guid);
    // Every deletion keeps its project out of later syncs, with frames or
    // without. A project without frames is not listed under Deleted
    // projects until a sync leaves it out.
    tx.execute(
        "INSERT INTO psf_guard_removed_project
            (project_id, project_guid, name, removed_at, frames, allowed_back_at, origin)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'local')",
        params![
            project_id,
            tombstone_guid,
            plan.name,
            now,
            plan.frames.total as i64
        ],
    )?;
    let removal = tx.last_insert_rowid();
    {
        let mut tombstone = tx.prepare(
            "INSERT INTO psf_guard_removed_project_frame
                (removal_id, acquired_image_id, acquired_image_guid, project_id, project_guid,
                 file_name, removed_at, acquired_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for frame in &plan.frame_rows {
            tombstone.execute(params![
                removal,
                frame.id,
                frame.guid,
                project_id,
                tombstone_guid,
                frame.file_name,
                now,
                frame.acquired_at
            ])?;
        }
    }
    // Calibrated copies paired with the frames: an import would otherwise
    // take one for a new light once its frame is gone.
    if table_exists(&tx, "psf_guard_frame_derivative") {
        tx.execute(
            "INSERT INTO psf_guard_removed_project_frame
                (removal_id, acquired_image_id, acquired_image_guid, project_id, project_guid,
                 file_name, removed_at, acquired_at)
             SELECT ?4, frame.acquired_image_id, copy.acquired_image_guid, ?1, ?2, lower(copy.file_name), ?3,
                    frame.acquired_at
             FROM psf_guard_frame_derivative copy
             JOIN psf_guard_removed_project_frame frame
               ON frame.removal_id = ?4 AND frame.acquired_image_guid = copy.acquired_image_guid",
            params![project_id, tombstone_guid, now, removal],
        )?;
    }
    let frames = delete_project_rows(&tx, &plan)?;
    tx.commit()?;
    Ok(Outcome::Removed { plan, frames })
}

/// Delete a planned project's rows: its frames and what PSF Guard keeps
/// about them, its targets with their exposure plans and Planning records,
/// its rule weights, and the project. Writes no tombstone; the caller
/// decides what to record. Runs in the caller's transaction. Each table is
/// deleted from in one statement: Target Scheduler has no index on
/// `imagedata.acquiredimageid`, so a statement per frame would scan its
/// thumbnails once for every frame.
pub(crate) fn delete_project_rows(
    tx: &Connection,
    plan: &RemovalPlan,
) -> Result<Vec<RemovedFrame>> {
    let project_id = plan.project_id;
    // A GUID shared with another project identifies neither.
    let guid = plan.guid.as_deref().filter(|_| plan.unique_guid);

    // The frames, in a temporary table the deletes below select from.
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS psf_guard_deleting_frame (id INTEGER PRIMARY KEY, guid TEXT);
         DELETE FROM temp.psf_guard_deleting_frame;",
    )?;
    let mut frames = Vec::with_capacity(plan.frame_rows.len());
    {
        let mut listed =
            tx.prepare("INSERT INTO temp.psf_guard_deleting_frame (id, guid) VALUES (?1, ?2)")?;
        for frame in &plan.frame_rows {
            listed.execute(params![frame.id, frame.guid])?;
            frames.push(RemovedFrame {
                image_id: frame.id,
                project_id: frame.project_id,
                target_id: frame.target_id,
            });
        }
    }
    for (table, key, column) in [
        ("imagedata", "acquiredimageid", "id"),
        ("psf_guard_remote_image_file", "acquiredimage_id", "id"),
        ("psf_guard_flat_light_angle", "light_id", "id"),
        ("psf_guard_frame_derivative", "acquired_image_guid", "guid"),
        ("psf_guard_archive", "acquired_image_guid", "guid"),
        ("psf_guard_rejected_at", "acquired_image_guid", "guid"),
        ("psf_guard_ts_sync_grade", "guid", "guid"),
        ("psf_guard_ts_sync_pending", "guid", "guid"),
        ("psf_guard_flat_light_choice", "light_guid", "guid"),
        ("acquiredimage", "Id", "id"),
    ] {
        if table_exists(tx, table) {
            tx.execute(
                &format!(
                    "DELETE FROM \"{table}\" WHERE \"{key}\" IN
                       (SELECT {column} FROM temp.psf_guard_deleting_frame WHERE {column} IS NOT NULL)"
                ),
                [],
            )?;
        }
    }
    tx.execute("DELETE FROM temp.psf_guard_deleting_frame", [])?;
    // Rejects removed earlier keep their tombstones for the trash, but no
    // longer count toward a target or exposure plan that takes these Ids.
    if table_exists(tx, "psf_guard_removed_image") {
        tx.execute(
            "UPDATE psf_guard_removed_image SET target_id = NULL, exposure_id = NULL
             WHERE project_id = ?1 OR target_id IN (SELECT Id FROM target WHERE projectid = ?1)",
            [project_id],
        )?;
    }

    // What belongs to each target: its exposure plans, the order and cadence
    // rows that index them, flat history, and Planning's records in this
    // rig's database. Target Scheduler declares no cascades.
    for (table, column) in [
        ("overrideexposureorderitem", "targetid"),
        ("filtercadenceitem", "targetid"),
        ("flathistory", "targetId"),
        ("exposureplan", "targetid"),
    ] {
        if table_exists(tx, table) && has_column(tx, table, column) {
            tx.execute(
                &format!(
                    "DELETE FROM \"{table}\" WHERE \"{column}\" IN
                       (SELECT Id FROM target WHERE projectid = ?1)"
                ),
                [project_id],
            )?;
        }
    }
    for table in [
        "psf_guard_director_target",
        "psf_guard_director_plan",
        "psf_guard_collaboration_plan",
        "psf_guard_scheduler_flat_history",
    ] {
        if !table_exists(tx, table) {
            continue;
        }
        for target_guid in &plan.target_guids {
            tx.execute(
                &format!("DELETE FROM \"{table}\" WHERE target_guid = ?1"),
                [target_guid],
            )?;
        }
    }
    if let Some(guid) = guid {
        // Planning's rows for targets deleted by hand before this.
        if table_exists(tx, "psf_guard_director_target") {
            tx.execute(
                "DELETE FROM psf_guard_director_target WHERE lower(project_guid) = lower(?1)",
                [guid],
            )?;
        }
        if table_exists(tx, "psf_guard_director_project") {
            tx.execute(
                "DELETE FROM psf_guard_director_project WHERE lower(project_guid) = lower(?1)",
                [guid],
            )?;
        }
    }
    tx.execute("DELETE FROM target WHERE projectid = ?1", [project_id])?;
    if table_exists(tx, "ruleweight") {
        tx.execute("DELETE FROM ruleweight WHERE projectid = ?1", [project_id])?;
    }
    if table_exists(tx, "psf_guard_project_processing") {
        tx.execute(
            "DELETE FROM psf_guard_project_processing WHERE project_key IN (?1, ?2)",
            params![
                format!("local-id:{project_id}"),
                guid.map(|guid| format!("guid:{}", guid.to_ascii_lowercase()))
            ],
        )?;
    }
    tx.execute("DELETE FROM project WHERE Id = ?1", [project_id])?;
    Ok(frames)
}

#[cfg(test)]
mod tests;
