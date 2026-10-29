//! Lights catalogued twice because their file was renamed after it was
//! imported (#1020). An import knows a light by its file name, so a renamed
//! file looked new: it became a second light, and the first light's file
//! was gone, so the first light showed as "Catalog only".
//!
//! - An import that has seen every image folder gives a renamed file the
//!   row of the light whose file is gone ([`relink_renamed`]), instead of a
//!   second row. The row keeps its id, guid and grade.
//! - [`plan`] finds the twins earlier imports left, and [`merge`] folds each
//!   into the first light: the first light takes the renamed file, and the
//!   twin's grade when it has none of its own. The twin's row is removed,
//!   with a record so that a sync does not bring it back.
//! - Either way the light's new name is recorded beside the name it had.
//!   A sync from the light's source, a rig or Target Scheduler, sends the
//!   old name again: the light keeps its new one ([`synced_metadata`]), and
//!   a peer that pulls this catalog gets the old one
//!   ([`exported_metadata`]), which is what its own copy of the file has.
//!
//! Two rows are the same exposure when calibrated-copy pairing would say so:
//! the same capture time, exposure, filter and target. A pair also needs
//! different file names, with one file found and the other not. Anything
//! less certain is left alone, and [`plan`] reports why.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use rusqlite::{params, Connection, TransactionBehavior};
use serde::Serialize;

use crate::directory_tree::DirectoryTree;
use crate::frame_derivatives::{identities_match, identity_of, light_identity, FrameIdentity};
use crate::image_io::FrameKind;

/// PSF Guard's record of each twin [`merge`] removed, so a sync from a
/// catalog that still has the twin does not bring it back.
const MERGED_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS psf_guard_merged_light (
    removed_guid TEXT PRIMARY KEY NOT NULL,
    removed_id INTEGER NOT NULL,
    kept_guid TEXT NOT NULL,
    file_name TEXT NOT NULL,
    merged_at INTEGER NOT NULL
)";

/// PSF Guard's record of each light it moved to a renamed file: the name
/// the light had before, which its source still knows, and the name it has
/// here.
const RENAMED_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS psf_guard_renamed_light (
    guid TEXT PRIMARY KEY NOT NULL,
    source_name TEXT NOT NULL,
    file_name TEXT NOT NULL,
    renamed_at INTEGER NOT NULL
)";

/// Whether a light's file can be found, from the `FileName` its row
/// records. Cloned into import options, so it owns what it looks in.
#[derive(Clone)]
pub struct FileFinder(Arc<dyn Fn(&str) -> bool + Send + Sync>);

impl FileFinder {
    pub fn new(found: impl Fn(&str) -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(found))
    }

    /// Look for a light's file as the server does: at the recorded path, by
    /// name in the directory tree, and at the top of each image folder.
    /// `walked` adds the lower-case names a walk of the folders just found,
    /// which a tree built before the walk may lack.
    pub fn in_folders(
        tree: Option<Arc<DirectoryTree>>,
        roots: Vec<PathBuf>,
        walked: Option<HashSet<String>>,
    ) -> Self {
        Self::new(move |file_name| {
            let name = base_name(file_name);
            if name.is_empty() {
                return false;
            }
            std::path::Path::new(file_name).is_file()
                || walked
                    .as_ref()
                    .is_some_and(|walked| walked.contains(&name.to_lowercase()))
                || tree
                    .as_ref()
                    .and_then(|tree| tree.find_file(name))
                    .is_some_and(|paths| paths.iter().any(|path| path.is_file()))
                || roots.iter().any(|root| root.join(name).is_file())
        })
    }

    pub fn found(&self, file_name: &str) -> bool {
        (self.0)(file_name)
    }
}

impl std::fmt::Debug for FileFinder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileFinder")
    }
}

/// A light this catalog moved to a renamed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renamed {
    /// The name the light had before, which its source still sends.
    pub source_name: String,
    /// The name it has here.
    pub file_name: String,
}

/// The lights this catalog moved to a renamed file, by guid.
pub fn renamed(conn: &Connection) -> Result<HashMap<String, Renamed>> {
    if !table_exists(conn, "psf_guard_renamed_light") {
        return Ok(HashMap::new());
    }
    let mut statement =
        conn.prepare("SELECT guid, source_name, file_name FROM psf_guard_renamed_light")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get(0)?,
            Renamed {
                source_name: row.get(1)?,
                file_name: row.get(2)?,
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Record that a light now uses `file_name`. A light renamed again keeps
/// the name it had first: that is the one its source knows.
fn record_rename(
    conn: &Connection,
    guid: &str,
    old_name: &str,
    file_name: &str,
    at: i64,
) -> Result<()> {
    conn.execute_batch(RENAMED_SCHEMA)?;
    conn.execute(
        "INSERT INTO psf_guard_renamed_light (guid, source_name, file_name, renamed_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (guid) DO UPDATE
             SET file_name = excluded.file_name, renamed_at = excluded.renamed_at",
        params![guid, old_name, file_name, at],
    )?;
    Ok(())
}

fn file_name_in(metadata: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(metadata)
        .ok()?
        .get("FileName")?
        .as_str()
        .map(str::to_string)
}

/// The metadata with another `FileName`, written as the renames wrote it,
/// so a sync that changes nothing else finds the row unchanged.
fn with_file_name(conn: &Connection, metadata: &str, file_name: &str) -> Result<String> {
    Ok(conn.query_row(
        "SELECT json_set(?1, '$.FileName', ?2)",
        params![metadata, file_name],
        |row| row.get(0),
    )?)
}

/// What a sync writes for a light this catalog renamed, given the metadata
/// its source sent: the metadata with this catalog's name while the source
/// sends the name the light had before. `None` leaves the metadata as it
/// came. When the source sends a name that is neither, it renamed the file
/// itself: its name stands, and the record goes.
pub fn synced_metadata(
    conn: &Connection,
    guid: &str,
    renamed: &Renamed,
    incoming: &str,
) -> Result<Option<String>> {
    let Some(name) = file_name_in(incoming) else {
        return Ok(None);
    };
    if same_name(&name, &renamed.source_name) {
        return with_file_name(conn, incoming, &renamed.file_name).map(Some);
    }
    if !same_name(&name, &renamed.file_name) {
        conn.execute(
            "DELETE FROM psf_guard_renamed_light WHERE guid = ?1",
            [guid],
        )?;
    }
    Ok(None)
}

/// What an export sends for a light this catalog renamed: its metadata
/// under the name it had before, which a peer's own copy of the file has.
/// `None` leaves the metadata as it is.
pub fn exported_metadata(
    conn: &Connection,
    renamed: &Renamed,
    metadata: &str,
) -> Result<Option<String>> {
    match file_name_in(metadata) {
        Some(name) if same_name(&name, &renamed.file_name) => {
            with_file_name(conn, metadata, &renamed.source_name).map(Some)
        }
        _ => Ok(None),
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// A file name without its folders, across either separator: Target
/// Scheduler records the capture computer's path.
fn base_name(file_name: &str) -> &str {
    file_name.rsplit(['/', '\\']).next().unwrap_or(file_name)
}

/// One catalogued light, as a pair shows it.
#[derive(Debug, Clone, Serialize)]
pub struct Light {
    pub id: i64,
    pub guid: String,
    pub file_name: String,
    pub target: Option<String>,
    /// `gradingStatus`: 0 pending, 1 accepted, 2 rejected.
    pub grade: i64,
    pub reject_reason: Option<String>,
    /// What the light's cache files are keyed by, beside its id.
    #[serde(skip)]
    pub project_id: Option<i64>,
    #[serde(skip)]
    pub target_id: Option<i64>,
    #[serde(skip)]
    exposure_id: Option<i64>,
    #[serde(skip)]
    identity: FrameIdentity,
}

/// A light whose file is gone, and the twin an import made of its renamed
/// file.
#[derive(Debug, Clone, Serialize)]
pub struct RenamedPair {
    /// The first light. It keeps its id and guid and takes the twin's file.
    pub kept: Light,
    /// The second light, which is removed.
    pub twin: Light,
    /// Whether the kept light takes the twin's grade: it had none.
    pub takes_grade: bool,
}

/// Twins left alone, and why.
#[derive(Debug, Clone, Serialize)]
pub struct HeldPair {
    pub kept: Light,
    pub twin: Light,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RenamedReport {
    pub pairs: Vec<RenamedPair>,
    pub held: Vec<HeldPair>,
}

/// What [`merge`] did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MergeOutcome {
    pub merged: Vec<RenamedPair>,
    pub held: Vec<HeldPair>,
}

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")
        .and_then(|mut statement| statement.exists([table]))
        .unwrap_or(false)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    conn.prepare(&format!(
        "SELECT 1 FROM pragma_table_info('{table}') WHERE lower(name) = lower(?1)"
    ))
    .and_then(|mut statement| statement.exists([column]))
    .unwrap_or(false)
}

fn strings(conn: &Connection, table: &str, sql: &str) -> Result<HashSet<String>> {
    if !table_exists(conn, table) {
        return Ok(HashSet::new());
    }
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// What else the catalog records about its lights that decides whether a
/// row may take a file over or be removed.
struct Records {
    /// Lights moved to the reject archive: their file is elsewhere.
    archived: HashSet<String>,
    /// Lights that arrived by remote upload, found through their record.
    uploaded: HashSet<String>,
    /// Lights catalogued from a calibrated copy: the row's file is the copy.
    from_copy: HashSet<String>,
    /// Lights with calibrated or registered copies recorded against them.
    with_copies: HashSet<String>,
}

impl Records {
    fn read(conn: &Connection) -> Result<Self> {
        Ok(Self {
            archived: strings(
                conn,
                "psf_guard_archive",
                "SELECT acquired_image_guid FROM psf_guard_archive",
            )?,
            uploaded: strings(
                conn,
                "psf_guard_remote_image_file",
                "SELECT CAST(acquiredimage_id AS TEXT) FROM psf_guard_remote_image_file",
            )?,
            from_copy: strings(
                conn,
                "psf_guard_frame_derivative",
                "SELECT acquired_image_guid FROM psf_guard_frame_derivative WHERE primary_source = 1",
            )?,
            with_copies: strings(
                conn,
                "psf_guard_frame_derivative",
                "SELECT acquired_image_guid FROM psf_guard_frame_derivative",
            )?,
        })
    }

    /// Why this light may not take a renamed file over, if it may not.
    fn keeps_own_file(&self, light: &Light) -> Option<&'static str> {
        if self.archived.contains(&light.guid) {
            Some("its file is in the reject archive")
        } else if self.uploaded.contains(&light.id.to_string()) {
            Some("it arrived by remote upload")
        } else if self.from_copy.contains(&light.guid) {
            Some("it was catalogued from a calibrated copy")
        } else {
            None
        }
    }

    /// Why this light may not be removed as a twin, if it may not.
    fn must_stay(&self, light: &Light, protected: &BTreeSet<String>) -> Option<&'static str> {
        if protected.contains(&light.guid) {
            Some("it is in a collaboration report")
        } else if self.archived.contains(&light.guid) {
            Some("its file is in the reject archive")
        } else if self.uploaded.contains(&light.id.to_string()) {
            Some("it arrived by remote upload")
        } else if self.with_copies.contains(&light.guid) {
            Some("calibrated copies are recorded against it")
        } else {
            None
        }
    }
}

/// Every light with a guid and a file name. A light without a guid cannot
/// be recorded as merged; `fill-guids` gives it one.
fn lights(conn: &Connection) -> Result<Vec<Light>> {
    if !has_column(conn, "acquiredimage", "guid") {
        return Ok(Vec::new());
    }
    let exposure = if has_column(conn, "acquiredimage", "exposureId") {
        "a.exposureId"
    } else {
        "NULL"
    };
    let mut statement = conn.prepare(&format!(
        "SELECT a.Id, a.guid, a.metadata, a.acquireddate, a.filtername, t.name,
                a.gradingStatus, a.rejectreason, {exposure}, a.projectId, a.targetId
         FROM acquiredimage a LEFT JOIN target t ON t.Id = a.targetId
         WHERE a.guid IS NOT NULL AND trim(a.guid) <> ''"
    ))?;
    let rows = statement.query_map([], |row| {
        let metadata = row.get::<_, Option<String>>(2)?.unwrap_or_default();
        let target: Option<String> = row.get(5)?;
        let identity = light_identity(&metadata, row.get(3)?, row.get(4)?, target.clone());
        let file_name = serde_json::from_str::<serde_json::Value>(&metadata)
            .ok()
            .and_then(|json| {
                json.get("FileName")
                    .and_then(|name| name.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        Ok(Light {
            id: row.get(0)?,
            guid: row.get(1)?,
            file_name,
            target,
            grade: row.get::<_, Option<i64>>(6)?.unwrap_or(0),
            reject_reason: row.get(7)?,
            exposure_id: row.get(8)?,
            project_id: row.get(9)?,
            target_id: row.get(10)?,
            identity,
        })
    })?;
    let mut lights = Vec::new();
    for light in rows {
        let light = light?;
        if !base_name(&light.file_name).is_empty() && light.identity.captured_at.is_some() {
            lights.push(light);
        }
    }
    Ok(lights)
}

fn same_name(left: &str, right: &str) -> bool {
    base_name(left).eq_ignore_ascii_case(base_name(right))
}

/// New lights that are renamed copies of a catalogued light whose file is
/// gone. Each takes that light's row: the row's `FileName` moves to the new
/// file, and its id, guid and grade stay. Returns the indices of the frames
/// placed this way.
///
/// A frame takes a row only when exactly one light is the same exposure
/// under another name and that light's file is not found. When a matching
/// light's file is found, the frame is a copy of a light the catalog
/// already sees, and the import decides about it as before.
pub fn relink_renamed(
    conn: &Connection,
    frames: &[crate::commands::import::headers::FrameMeta],
    finder: &FileFinder,
) -> Result<Vec<usize>> {
    let candidates: Vec<(usize, FrameIdentity)> = frames
        .iter()
        .enumerate()
        .filter(|(_, frame)| frame.class.kind == FrameKind::Raw && !frame.processed)
        .map(|(index, frame)| (index, identity_of(frame)))
        .filter(|(_, identity)| identity.captured_at.is_some())
        .collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let seconds: HashSet<i64> = candidates
        .iter()
        .filter_map(|(_, identity)| identity.captured_at.map(|at| at.seconds))
        .collect();
    let catalogued: Vec<Light> = lights(conn)?
        .into_iter()
        .filter(|light| {
            light
                .identity
                .captured_at
                .is_some_and(|at| seconds.contains(&at.seconds))
        })
        .collect();
    if catalogued.is_empty() {
        return Ok(Vec::new());
    }
    let records = Records::read(conn)?;
    let mut taken: HashSet<i64> = HashSet::new();
    let mut relinked = Vec::new();
    for (index, identity) in candidates {
        let frame = &frames[index];
        let path = frame.path.to_string_lossy();
        let matches: Vec<&Light> = catalogued
            .iter()
            .filter(|light| {
                identities_match(&light.identity, &identity) && !same_name(&light.file_name, &path)
            })
            .collect();
        let [light] = matches.as_slice() else {
            continue;
        };
        if taken.contains(&light.id)
            || records.keeps_own_file(light).is_some()
            || finder.found(&light.file_name)
        {
            continue;
        }
        conn.execute(
            "UPDATE acquiredimage SET metadata = json_set(metadata, '$.FileName', ?1)
             WHERE Id = ?2",
            params![path, light.id],
        )?;
        record_rename(conn, &light.guid, &light.file_name, &path, now())?;
        taken.insert(light.id);
        relinked.push(index);
    }
    Ok(relinked)
}

/// The twins in the catalog: each light whose file is gone, with the one
/// light an import made of its renamed file. `protected` lists guids a
/// collaboration report names, which stay.
pub fn plan(
    conn: &Connection,
    finder: &FileFinder,
    protected: &BTreeSet<String>,
) -> Result<RenamedReport> {
    let mut report = RenamedReport::default();
    let all = lights(conn)?;
    let mut by_second: HashMap<i64, Vec<usize>> = HashMap::new();
    for (index, light) in all.iter().enumerate() {
        if let Some(at) = light.identity.captured_at {
            by_second.entry(at.seconds).or_default().push(index);
        }
    }
    let records = Records::read(conn)?;
    let mut found: HashMap<i64, bool> = HashMap::new();
    let mut is_found = |light: &Light| {
        *found
            .entry(light.id)
            .or_insert_with(|| finder.found(&light.file_name))
    };
    let mut groups: Vec<Vec<usize>> = by_second
        .into_values()
        .filter(|group| group.len() > 1)
        .collect();
    groups.sort();
    for group in groups {
        // Within one capture second: which lights are the same exposure
        // under another name, and whose file is found.
        let twins_of = |index: usize| -> Vec<usize> {
            group
                .iter()
                .copied()
                .filter(|other| *other != index)
                .filter(|other| {
                    identities_match(&all[index].identity, &all[*other].identity)
                        && !same_name(&all[index].file_name, &all[*other].file_name)
                })
                .collect()
        };
        for &index in &group {
            let kept = &all[index];
            let twins = twins_of(index);
            if twins.is_empty() || is_found(kept) {
                continue;
            }
            let present: Vec<usize> = twins
                .iter()
                .copied()
                .filter(|twin| is_found(&all[*twin]))
                .collect();
            let [twin_index] = present.as_slice() else {
                continue;
            };
            let twin = &all[*twin_index];
            let hold = |reason: String| HeldPair {
                kept: kept.clone(),
                twin: twin.clone(),
                reason,
            };
            if twins.len() > 1 {
                report.held.push(hold(
                    "more than one light is this exposure under another name".into(),
                ));
                continue;
            }
            // The twin's other matches: another missing light makes the
            // choice of which to keep a guess.
            if twins_of(*twin_index).len() > 1 {
                report.held.push(hold(
                    "more than one light whose file is gone is this exposure".into(),
                ));
                continue;
            }
            if let Some(reason) = records.keeps_own_file(kept) {
                report
                    .held
                    .push(hold(format!("the first light stays: {reason}")));
                continue;
            }
            if let Some(reason) = records.must_stay(twin, protected) {
                report
                    .held
                    .push(hold(format!("the second light stays: {reason}")));
                continue;
            }
            let takes_grade = kept.grade == 0 && twin.grade != 0;
            if kept.grade != 0 && twin.grade != 0 && kept.grade != twin.grade {
                report.held.push(hold(
                    "the two lights have different grades; grade one to match the other".into(),
                ));
                continue;
            }
            report.pairs.push(RenamedPair {
                kept: kept.clone(),
                twin: twin.clone(),
                takes_grade,
            });
        }
    }
    report.pairs.sort_by_key(|pair| pair.kept.id);
    report.held.sort_by_key(|held| held.kept.id);
    Ok(report)
}

/// Fold each twin [`plan`] finds now into its first light, in one
/// transaction. The plan is made again inside it, so what merges is what
/// the catalog holds at that moment.
pub fn merge(
    conn: &mut Connection,
    finder: &FileFinder,
    protected: &BTreeSet<String>,
) -> Result<MergeOutcome> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let report = plan(&tx, finder, protected)?;
    if report.pairs.is_empty() {
        return Ok(MergeOutcome {
            merged: Vec::new(),
            held: report.held,
        });
    }
    tx.execute_batch(MERGED_SCHEMA)?;
    let now = now();
    let has_exposure_plans = has_column(&tx, "exposureplan", "acquired");
    for pair in &report.pairs {
        let (kept, twin) = (&pair.kept, &pair.twin);
        tx.execute(
            "UPDATE acquiredimage SET metadata = json_set(metadata, '$.FileName', ?1)
             WHERE Id = ?2",
            params![twin.file_name, kept.id],
        )?;
        record_rename(&tx, &kept.guid, &kept.file_name, &twin.file_name, now)?;
        if pair.takes_grade {
            tx.execute(
                "UPDATE acquiredimage SET gradingStatus = ?1, rejectreason = ?2 WHERE Id = ?3",
                params![twin.grade, twin.reject_reason, kept.id],
            )?;
        }
        // A flat chosen by hand for the twin applies to the light it was.
        if table_exists(&tx, "psf_guard_flat_light_choice") {
            tx.execute(
                "UPDATE OR IGNORE psf_guard_flat_light_choice SET light_guid = ?1
                 WHERE light_guid = ?2",
                params![kept.guid, twin.guid],
            )?;
        }
        for (table, key, value) in [
            ("imagedata", "acquiredimageid", twin.id.to_string()),
            (
                "psf_guard_rejected_at",
                "acquired_image_guid",
                twin.guid.clone(),
            ),
            (
                "psf_guard_flat_light_choice",
                "light_guid",
                twin.guid.clone(),
            ),
            (
                "psf_guard_flat_light_angle",
                "light_id",
                twin.id.to_string(),
            ),
            ("psf_guard_ts_sync_grade", "guid", twin.guid.clone()),
            ("psf_guard_ts_sync_pending", "guid", twin.guid.clone()),
            ("psf_guard_renamed_light", "guid", twin.guid.clone()),
        ] {
            if table_exists(&tx, table) {
                tx.execute(
                    &format!("DELETE FROM \"{table}\" WHERE CAST(\"{key}\" AS TEXT) = ?1"),
                    [value],
                )?;
            }
        }
        // The twin's import counted the exposure a second time.
        if has_exposure_plans && let Some(exposure_id) = twin.exposure_id {
            tx.execute(
                "UPDATE exposureplan SET acquired = acquired - 1
                 WHERE Id = ?1 AND acquired > 0",
                [exposure_id],
            )?;
        }
        tx.execute("DELETE FROM acquiredimage WHERE Id = ?1", [twin.id])?;
        tx.execute(
            "INSERT OR REPLACE INTO psf_guard_merged_light
                 (removed_guid, removed_id, kept_guid, file_name, merged_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![twin.guid, twin.id, kept.guid, twin.file_name, now],
        )?;
    }
    crate::db::reconcile_accepted_counts(&tx)?;
    crate::db::record_rejection_times(&tx)?;
    tx.commit()?;
    Ok(MergeOutcome {
        merged: report.pairs,
        held: report.held,
    })
}

/// Guids of the twins [`merge`] removed, which a sync does not bring back.
pub fn merged_guids(conn: &Connection) -> Result<HashSet<String>> {
    strings(
        conn,
        "psf_guard_merged_light",
        "SELECT removed_guid FROM psf_guard_merged_light",
    )
}

/// What a merge on a database file did, and the copy taken first; no copy
/// when there was nothing to merge.
#[derive(Debug, Serialize)]
pub struct RepairOutcome {
    pub outcome: MergeOutcome,
    pub backup: Option<PathBuf>,
}

/// The whole repair on a database file: refuse a file this process cannot
/// write, copy it beside itself when there is something to merge, then
/// merge. Runs on its own connection.
pub fn repair_database(
    path: &std::path::Path,
    finder: &FileFinder,
    protected: &BTreeSet<String>,
) -> Result<RepairOutcome> {
    let mut conn = crate::ts_guids::open_existing(path, true)?;
    if conn.is_readonly(rusqlite::MAIN_DB)? {
        anyhow::bail!(
            "{} is read only here; nothing was copied or changed",
            path.display()
        );
    }
    let report = plan(&conn, finder, protected)?;
    if report.pairs.is_empty() {
        return Ok(RepairOutcome {
            outcome: MergeOutcome {
                merged: Vec::new(),
                held: report.held,
            },
            backup: None,
        });
    }
    let backup = crate::ts_guids::backup_beside_as(path, "renamed-merge")?;
    let outcome = merge(&mut conn, finder, protected)?;
    Ok(RepairOutcome {
        outcome,
        backup: Some(backup),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::import::headers::FrameMeta;
    use crate::commands::import::{import_frames, ImportOptions};
    use std::path::Path;

    const TARGET: &str = "Jellyfish Nebula SHO";
    const OLD_NAME: &str =
        "Jellyfish Nebula SHO_LIGHT_SII_300.00s_2026-10-27_23-06-46_Session_2026-10-27_0051.fits";
    const NEW_NAME: &str =
        "Jellyfish Nebula SHO_LIGHT_SII_300.00s_2026-10-27_23-06-46_Session_2026-10-27A_0051.fits";
    const CAPTURED: &str = "2026-10-27T23:06:46";

    fn frame(path: &Path) -> FrameMeta {
        FrameMeta {
            path: path.to_path_buf(),
            readable: true,
            image_type: Some("LIGHT".into()),
            object: Some(TARGET.into()),
            filter: Some("SII".into()),
            timestamp: chrono::NaiveDateTime::parse_from_str(CAPTURED, "%Y-%m-%dT%H:%M:%S")
                .ok()
                .map(|at| at.and_utc().timestamp()),
            date_obs: Some(CAPTURED.into()),
            date_obs_utc: Some(CAPTURED.into()),
            exposure_s: Some(300.0),
            gain: Some(100),
            offset: Some(30),
            binning_x: Some(1),
            binning_y: Some(1),
            ra_deg: Some(94.3),
            dec_deg: Some(22.5),
            telescope: Some("EdgeHD".into()),
            camera: Some("ASI2600".into()),
            focal_length_mm: Some(1960.0),
            ..Default::default()
        }
    }

    /// A catalog file and an image folder, with one light imported from
    /// `OLD_NAME`.
    struct Catalog {
        _dir: tempfile::TempDir,
        db: PathBuf,
        images: PathBuf,
        conn: Connection,
    }

    impl Catalog {
        fn with_first_import() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("catalog.sqlite");
            let images = dir.path().join("images");
            std::fs::create_dir_all(&images).unwrap();
            let mut conn = Connection::open(&db).unwrap();
            crate::ts_schema::apply_schema(&conn).unwrap();
            std::fs::write(images.join(OLD_NAME), b"fits").unwrap();
            let outcome = import_frames(
                &mut conn,
                vec![frame(&images.join(OLD_NAME))],
                &ImportOptions::default(),
            )
            .unwrap();
            assert_eq!(outcome.imported, 1);
            Self {
                _dir: dir,
                db,
                images,
                conn,
            }
        }

        fn rename(&self) {
            std::fs::rename(self.images.join(OLD_NAME), self.images.join(NEW_NAME)).unwrap();
        }

        fn finder(&self, walked: Option<&[&str]>) -> FileFinder {
            FileFinder::in_folders(
                None,
                vec![self.images.clone()],
                walked.map(|names| names.iter().map(|name| name.to_lowercase()).collect()),
            )
        }

        fn import_new_name(
            &mut self,
            finder: Option<FileFinder>,
        ) -> crate::commands::import::ImportOutcome {
            import_frames(
                &mut self.conn,
                vec![frame(&self.images.join(NEW_NAME))],
                &ImportOptions {
                    find_files: finder,
                    ..ImportOptions::default()
                },
            )
            .unwrap()
        }

        /// (Id, guid, FileName, grade) of each light, by Id.
        fn lights(&self) -> Vec<(i64, String, String, i64)> {
            let mut statement = self
                .conn
                .prepare(
                    "SELECT Id, guid, json_extract(metadata, '$.FileName'), gradingStatus
                     FROM acquiredimage ORDER BY Id",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        }

        fn grade(&self, id: i64, grade: i64) {
            self.conn
                .execute(
                    "UPDATE acquiredimage SET gradingStatus = ?1 WHERE Id = ?2",
                    params![grade, id],
                )
                .unwrap();
        }

        /// (acquired, accepted) of the light's exposure plan.
        fn plan_counts(&self) -> (i64, i64) {
            self.conn
                .query_row("SELECT acquired, accepted FROM exposureplan", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .unwrap()
        }

        /// The state the reported catalog was in: the renamed file was
        /// imported as a second light.
        fn with_twin() -> Self {
            let mut catalog = Self::with_first_import();
            catalog.rename();
            let outcome = catalog.import_new_name(None);
            assert_eq!(outcome.imported, 1, "without a finder, as before");
            assert_eq!(catalog.lights().len(), 2);
            catalog
        }
    }

    #[test]
    fn a_renamed_file_takes_over_the_light_its_old_name_made() {
        let mut catalog = Catalog::with_first_import();
        let [(id, guid, _, _)] = catalog.lights().try_into().unwrap();
        catalog.grade(id, 1);
        catalog.rename();

        let finder = catalog.finder(Some(&[NEW_NAME]));
        let outcome = catalog.import_new_name(Some(finder));

        assert_eq!((outcome.renamed, outcome.imported), (1, 0));
        let new_path = catalog.images.join(NEW_NAME).to_string_lossy().into_owned();
        assert_eq!(catalog.lights(), vec![(id, guid, new_path, 1)]);
    }

    #[test]
    fn a_copy_of_a_light_whose_file_is_there_imports_as_before() {
        let mut catalog = Catalog::with_first_import();
        std::fs::copy(catalog.images.join(OLD_NAME), catalog.images.join(NEW_NAME)).unwrap();

        let finder = catalog.finder(Some(&[OLD_NAME, NEW_NAME]));
        let outcome = catalog.import_new_name(Some(finder));

        assert_eq!((outcome.renamed, outcome.imported), (0, 1));
        assert_eq!(catalog.lights().len(), 2);
    }

    #[test]
    fn a_different_exposure_is_not_taken_for_a_renamed_file() {
        let mut catalog = Catalog::with_first_import();
        catalog.rename();
        let finder = catalog.finder(Some(&[NEW_NAME]));
        let mut later = frame(&catalog.images.join(NEW_NAME));
        later.date_obs_utc = Some("2026-10-27T23:12:01".into());
        later.timestamp = later.timestamp.map(|at| at + 315);

        let outcome = import_frames(
            &mut catalog.conn,
            vec![later],
            &ImportOptions {
                find_files: Some(finder),
                ..ImportOptions::default()
            },
        )
        .unwrap();

        assert_eq!((outcome.renamed, outcome.imported), (0, 1));
    }

    #[test]
    fn merge_folds_the_twin_into_the_first_light() {
        let mut catalog = Catalog::with_twin();
        let lights = catalog.lights();
        let (first, twin) = (lights[0].clone(), lights[1].clone());
        // The renamed file was graded, as it is the one with pixels.
        catalog.grade(twin.0, 1);
        crate::db::reconcile_accepted_counts(&catalog.conn).unwrap();
        assert_eq!(catalog.plan_counts(), (2, 1));
        let finder = catalog.finder(None);
        let protected = BTreeSet::new();

        let report = plan(&catalog.conn, &finder, &protected).unwrap();
        assert_eq!(report.pairs.len(), 1, "{report:?}");
        assert_eq!(
            (report.pairs[0].kept.id, report.pairs[0].twin.id),
            (first.0, twin.0)
        );
        assert!(report.pairs[0].takes_grade);

        let outcome = merge(&mut catalog.conn, &finder, &protected).unwrap();

        assert_eq!(outcome.merged.len(), 1);
        assert_eq!(
            catalog.lights(),
            vec![(first.0, first.1.clone(), twin.2.clone(), 1)]
        );
        assert_eq!(catalog.plan_counts(), (1, 1));
        // A sync that sends the first light's old name leaves the new one.
        assert_eq!(
            renamed(&catalog.conn).unwrap()[&first.1],
            Renamed {
                source_name: first.2.clone(),
                file_name: twin.2.clone(),
            }
        );
        // A pull from a catalog that still has the twin leaves it out.
        assert!(
            crate::commands::reject_removal::removed_guids(&catalog.conn)
                .unwrap()
                .contains(&twin.1)
        );
        // Nothing is left to merge.
        assert!(plan(&catalog.conn, &finder, &protected)
            .unwrap()
            .pairs
            .is_empty());
    }

    #[test]
    fn twins_with_different_grades_or_a_report_stay() {
        let catalog = Catalog::with_twin();
        let lights = catalog.lights();
        let finder = catalog.finder(None);

        catalog.grade(lights[0].0, 2);
        catalog.grade(lights[1].0, 1);
        let report = plan(&catalog.conn, &finder, &BTreeSet::new()).unwrap();
        assert!(report.pairs.is_empty());
        assert!(
            report.held[0].reason.contains("different grades"),
            "{report:?}"
        );

        catalog.grade(lights[0].0, 0);
        let protected = BTreeSet::from([lights[1].1.clone()]);
        let report = plan(&catalog.conn, &finder, &protected).unwrap();
        assert!(report.pairs.is_empty());
        assert!(
            report.held[0].reason.contains("collaboration"),
            "{report:?}"
        );
    }

    #[test]
    fn twins_stay_while_both_files_are_there() {
        let catalog = Catalog::with_twin();
        std::fs::write(catalog.images.join(OLD_NAME), b"fits").unwrap();

        let report = plan(&catalog.conn, &catalog.finder(None), &BTreeSet::new()).unwrap();

        assert!(
            report.pairs.is_empty() && report.held.is_empty(),
            "{report:?}"
        );
    }

    fn pull(source: &Connection, catalog: &Connection) -> crate::commands::sync::PullSummary {
        crate::commands::sync::sync_pull(
            source,
            catalog,
            &crate::commands::sync::PullOptions {
                dry_run: false,
                with_image_data: false,
                project_filter: None,
                deletions: Default::default(),
            },
        )
        .unwrap()
    }

    /// The catalog's light as a sync's source would send it: renamed here
    /// after the copy is taken.
    fn relinked_with_source() -> (Catalog, Connection, String) {
        let mut catalog = Catalog::with_first_import();
        let copy = catalog.db.with_file_name("source.sqlite");
        catalog
            .conn
            .execute("VACUUM INTO ?1", [copy.to_string_lossy()])
            .unwrap();
        catalog.rename();
        let finder = catalog.finder(Some(&[NEW_NAME]));
        assert_eq!(catalog.import_new_name(Some(finder)).renamed, 1);
        let new_path = catalog.images.join(NEW_NAME).to_string_lossy().into_owned();
        (catalog, Connection::open(copy).unwrap(), new_path)
    }

    #[test]
    fn a_sync_that_sends_the_old_name_leaves_the_new_one() {
        let (mut catalog, source, new_path) = relinked_with_source();
        let [(_, guid, _, _)] = catalog.lights().try_into().unwrap();
        let old_path = catalog.images.join(OLD_NAME).to_string_lossy().into_owned();
        assert_eq!(
            renamed(&catalog.conn).unwrap(),
            HashMap::from([(
                guid.clone(),
                Renamed {
                    source_name: old_path.clone(),
                    file_name: new_path.clone(),
                }
            )])
        );

        // A rig's sync: nothing else changed, so nothing is written.
        let summary = pull(&source, &catalog.conn);
        assert_eq!(
            (
                summary.acquiredimage.unchanged,
                summary.acquiredimage.updated
            ),
            (1, 0)
        );
        assert_eq!(catalog.lights()[0].2, new_path);

        // A Target Scheduler sync.
        let tx = catalog.conn.transaction().unwrap();
        crate::commands::sync::sync_arrivals(&source, &tx, &HashSet::new(), &Default::default())
            .unwrap();
        tx.commit().unwrap();
        assert_eq!(catalog.lights()[0].2, new_path);

        // A peer that pulls this catalog gets the name its own copy has.
        let bundle = crate::server::remote_sync::export_bundle(
            &catalog.db,
            "catalog",
            crate::server::remote_sync::SyncOperation::Merge,
            false,
            false,
            Default::default(),
        )
        .unwrap();
        let table = &bundle.tables["acquiredimage"];
        let metadata = table
            .columns
            .iter()
            .position(|column| column.name.eq_ignore_ascii_case("metadata"))
            .unwrap();
        let sent = table.rows[0].values[metadata].value.as_deref().unwrap();
        assert_eq!(file_name_in(sent).as_deref(), Some(old_path.as_str()));
    }

    #[test]
    fn a_name_the_source_gives_the_file_itself_stands() {
        let (catalog, source, _) = relinked_with_source();
        let theirs = r"C:\Users\nina\Images\Jellyfish_SII_0051_final.fits";
        source
            .execute(
                "UPDATE acquiredimage SET metadata = json_set(metadata, '$.FileName', ?1)",
                [theirs],
            )
            .unwrap();

        pull(&source, &catalog.conn);

        assert_eq!(catalog.lights()[0].2, theirs);
        assert!(renamed(&catalog.conn).unwrap().is_empty());
    }

    #[test]
    fn a_light_renamed_twice_keeps_the_name_its_source_knows() {
        let (mut catalog, source, _) = relinked_with_source();
        const NEWEST: &str =
            "Jellyfish Nebula SHO_LIGHT_SII_300.00s_2026-10-27_23-06-46_Session_2026-10-27C_0051.fits";
        std::fs::rename(catalog.images.join(NEW_NAME), catalog.images.join(NEWEST)).unwrap();
        let finder = catalog.finder(Some(&[NEWEST]));
        let outcome = import_frames(
            &mut catalog.conn,
            vec![frame(&catalog.images.join(NEWEST))],
            &ImportOptions {
                find_files: Some(finder),
                ..ImportOptions::default()
            },
        )
        .unwrap();
        assert_eq!(outcome.renamed, 1);

        pull(&source, &catalog.conn);

        let newest = catalog.images.join(NEWEST).to_string_lossy().into_owned();
        assert_eq!(catalog.lights()[0].2, newest);
        let [(_, record)] = renamed(&catalog.conn)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        assert!(record.source_name.ends_with(OLD_NAME), "{record:?}");
    }

    #[test]
    fn the_repair_copies_the_database_first_and_only_when_it_merges() {
        let catalog = Catalog::with_twin();
        let finder = catalog.finder(None);
        let backups = |catalog: &Catalog| {
            std::fs::read_dir(catalog.db.parent().unwrap())
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .contains(".before-renamed-merge-")
                })
                .count()
        };

        let repaired = repair_database(&catalog.db, &finder, &BTreeSet::new()).unwrap();
        assert_eq!(repaired.outcome.merged.len(), 1);
        assert!(repaired.backup.is_some());
        assert_eq!(backups(&catalog), 1);
        // The copy holds both lights, as they were.
        let copy = Connection::open(repaired.backup.unwrap()).unwrap();
        let rows: i64 = copy
            .query_row("SELECT count(*) FROM acquiredimage", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 2);

        let again = repair_database(&catalog.db, &finder, &BTreeSet::new()).unwrap();
        assert!(again.outcome.merged.is_empty() && again.backup.is_none());
        assert_eq!(backups(&catalog), 1);
    }
}
