//! Project removal: delete a project from a catalog, with its targets,
//! exposure plans and frame rows, and the PSF Guard records that refer to
//! them. Image files stay where they are. See
//! docs/design/project-removal.md.
//!
//! A removal is a [`plan`] and then a [`remove`] of that plan, which checks
//! the plan's digest inside its transaction so a frame captured after the
//! preview is never removed unseen. Tombstones keep the project out of
//! later syncs and its files out of later imports.

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
    allowed_back_at INTEGER
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
    removed_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_psf_guard_removed_project_frame_guid
    ON psf_guard_removed_project_frame(acquired_image_guid);
CREATE INDEX IF NOT EXISTS idx_psf_guard_removed_project_frame_removal
    ON psf_guard_removed_project_frame(removal_id)";

/// Create the tombstone tables. Safe to call repeatedly.
pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)
        .context("creating the removed project tables")
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

/// GUIDs of deleted projects, lower case, so a catalog pull leaves them and
/// everything under them out. Empty when no project was ever deleted.
pub fn removed_project_guids(conn: &Connection) -> Result<BTreeSet<String>> {
    if !table_exists(conn, "psf_guard_removed_project") {
        return Ok(BTreeSet::new());
    }
    let mut statement = conn.prepare(
        "SELECT lower(project_guid) FROM psf_guard_removed_project
         WHERE project_guid IS NOT NULL AND project_guid <> ''",
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
pub fn deletions(conn: &Connection) -> Result<Vec<Deletion>> {
    if !table_exists(conn, "psf_guard_removed_project") {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT id, project_id, name, removed_at, frames FROM psf_guard_removed_project
         WHERE allowed_back_at IS NULL ORDER BY removed_at DESC, id DESC",
    )?;
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

/// Let syncs and imports add a deleted project back: its GUID and its
/// frames' tombstones go, so the next sync or import of its files adds it
/// as a new project. The deletion's date stays, so a reject removed from
/// the project before it was deleted still cannot be restored into a
/// project that took its Id. Returns false when there is no such deletion.
pub fn allow_back(conn: &Connection, id: i64, now: i64) -> Result<bool> {
    if !table_exists(conn, "psf_guard_removed_project") {
        return Ok(false);
    }
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let changed = tx.execute(
        "UPDATE psf_guard_removed_project SET project_guid = NULL, allowed_back_at = ?2
         WHERE id = ?1 AND allowed_back_at IS NULL",
        params![id, now],
    )?;
    if changed == 0 {
        return Ok(false);
    }
    tx.execute(
        "DELETE FROM psf_guard_removed_project_frame WHERE removal_id = ?1",
        [id],
    )?;
    tx.commit()?;
    Ok(true)
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
    let mut statement = conn.prepare(&format!(
        "SELECT Id, {frame_guid}, targetId, gradingStatus, metadata, projectId
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
/// tombstones and deletes every row; a plan that no longer matches deletes
/// nothing. Each table is deleted from in one statement: Target Scheduler
/// has no index on `imagedata.acquiredimageid`, so a statement per frame
/// would scan its thumbnails once for every frame.
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
    let tombstone_guid = plan.guid.as_deref().filter(|_| plan.unique_guid);
    tx.execute(
        "INSERT INTO psf_guard_removed_project (project_id, project_guid, name, removed_at, frames)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            project_id,
            tombstone_guid,
            plan.name,
            now,
            plan.frames.total as i64
        ],
    )?;
    let removal = tx.last_insert_rowid();

    // The frames, in a temporary table the deletes below select from.
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS psf_guard_deleting_frame (id INTEGER PRIMARY KEY, guid TEXT);
         DELETE FROM temp.psf_guard_deleting_frame;",
    )?;
    let mut frames = Vec::with_capacity(plan.frame_rows.len());
    {
        let mut listed =
            tx.prepare("INSERT INTO temp.psf_guard_deleting_frame (id, guid) VALUES (?1, ?2)")?;
        let mut tombstone = tx.prepare(
            "INSERT INTO psf_guard_removed_project_frame
                (removal_id, acquired_image_id, acquired_image_guid, project_id, project_guid, file_name, removed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for frame in &plan.frame_rows {
            listed.execute(params![frame.id, frame.guid])?;
            tombstone.execute(params![
                removal,
                frame.id,
                frame.guid,
                project_id,
                tombstone_guid,
                frame.file_name,
                now
            ])?;
            frames.push(RemovedFrame {
                image_id: frame.id,
                project_id: frame.project_id,
                target_id: frame.target_id,
            });
        }
    }
    // Calibrated copies paired with the frames: an import would otherwise
    // take one for a new light once its frame is gone.
    if table_exists(&tx, "psf_guard_frame_derivative") {
        tx.execute(
            "INSERT INTO psf_guard_removed_project_frame
                (removal_id, acquired_image_id, acquired_image_guid, project_id, project_guid, file_name, removed_at)
             SELECT ?4, deleting.id, copy.acquired_image_guid, ?1, ?2, lower(copy.file_name), ?3
             FROM psf_guard_frame_derivative copy
             JOIN temp.psf_guard_deleting_frame deleting ON deleting.guid = copy.acquired_image_guid",
            params![project_id, tombstone_guid, now, removal],
        )?;
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
        if table_exists(&tx, table) {
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
    if table_exists(&tx, "psf_guard_removed_image") {
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
        if table_exists(&tx, table) && has_column(&tx, table, column) {
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
        if !table_exists(&tx, table) {
            continue;
        }
        for guid in &plan.target_guids {
            tx.execute(
                &format!("DELETE FROM \"{table}\" WHERE target_guid = ?1"),
                [guid],
            )?;
        }
    }
    if let Some(guid) = tombstone_guid {
        // Planning's rows for targets deleted by hand before this.
        if table_exists(&tx, "psf_guard_director_target") {
            tx.execute(
                "DELETE FROM psf_guard_director_target WHERE lower(project_guid) = lower(?1)",
                [guid],
            )?;
        }
        if table_exists(&tx, "psf_guard_director_project") {
            tx.execute(
                "DELETE FROM psf_guard_director_project WHERE lower(project_guid) = lower(?1)",
                [guid],
            )?;
        }
    }
    tx.execute("DELETE FROM target WHERE projectid = ?1", [project_id])?;
    if table_exists(&tx, "ruleweight") {
        tx.execute("DELETE FROM ruleweight WHERE projectid = ?1", [project_id])?;
    }
    if table_exists(&tx, "psf_guard_project_processing") {
        tx.execute(
            "DELETE FROM psf_guard_project_processing WHERE project_key IN (?1, ?2)",
            params![
                format!("local-id:{project_id}"),
                tombstone_guid.map(|guid| format!("guid:{}", guid.to_ascii_lowercase()))
            ],
        )?;
    }
    tx.execute("DELETE FROM project WHERE Id = ?1", [project_id])?;
    tx.commit()?;
    Ok(Outcome::Removed { plan, frames })
}

#[cfg(test)]
mod tests;
