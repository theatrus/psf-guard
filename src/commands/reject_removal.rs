//! Reject removal: take old rejected frames out of a catalog, move their
//! files to a trash folder, and keep a tombstone that can bring both back.
//! See docs/design/reject-removal.md.
//!
//! A run is a [`plan`] and then an [`apply`] of that plan. Each frame is
//! removed on its own: its files move to the trash first, then one
//! transaction writes its tombstone and deletes its rows; when that fails
//! the files move back. [`restore`] reverses a removal while the files are
//! still in the trash, and [`empty_trash`] deletes the files of removals
//! past their retention.

use crate::commands::filter_rejected::get_possible_paths;
use crate::commands::reject_archive;
use crate::directory_tree::DirectoryTree;
use anyhow::{bail, Context, Result};
use base64::Engine;
use rusqlite::{
    params, types::Value as SqlValue, Connection, OptionalExtension, Transaction,
    TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The folder, in each image directory, that removed files wait in. Folder
/// scans and imports skip it.
pub const TRASH_DIR: &str = ".psf-guard-trash";
/// How long a reject must have been rejected before it can be removed.
pub const DEFAULT_MIN_AGE_DAYS: u32 = 7;
/// How long removed files stay in the trash before emptying it deletes them.
pub const DEFAULT_RETENTION_DAYS: u32 = 14;

/// What a long step is doing, for a job's progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Indexing the image folders, by folder.
    Indexing,
    /// Finding each qualifying frame's files, by frame.
    Checking,
    /// Moving frames' files to the trash, by frame.
    Moving,
    /// Bringing frames back from the trash, by frame.
    Restoring,
    /// Deleting the files of removals past their retention, by frame.
    Deleting,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Self::Indexing => "indexing",
            Self::Checking => "checking",
            Self::Moving => "moving",
            Self::Restoring => "restoring",
            Self::Deleting => "deleting",
        }
    }
}

/// How a step reports where it stands, and learns it should stop. A stop
/// lands between frames: each frame's files and rows move together, so the
/// frames done before it stay done. The command line passes [`Work::quiet`].
pub struct Work<'a> {
    pub progress: &'a (dyn Fn(Stage, usize, usize) + Sync),
    pub stop: &'a (dyn Fn() -> bool + Sync),
}

fn no_progress(_: Stage, _: usize, _: usize) {}

fn never() -> bool {
    false
}

impl Work<'static> {
    /// No progress, and no stop.
    pub fn quiet() -> Self {
        Work {
            progress: &no_progress,
            stop: &never,
        }
    }
}

impl Work<'_> {
    fn stopped(&self) -> bool {
        (self.stop)()
    }
}

/// A preview stopped before it finished; it changed nothing.
#[derive(Debug)]
pub struct Stopped;

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Stopped")
    }
}

impl std::error::Error for Stopped {}
const DAY: i64 = 86_400;

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS psf_guard_removed_image (
    acquired_image_guid TEXT PRIMARY KEY NOT NULL,
    acquired_image_id INTEGER NOT NULL,
    project_id INTEGER,
    target_id INTEGER,
    exposure_id INTEGER,
    file_name TEXT,
    batch_id TEXT NOT NULL,
    removed_at INTEGER NOT NULL,
    rejected_at INTEGER,
    trash_until INTEGER NOT NULL,
    files_deleted_at INTEGER,
    purged_at INTEGER,
    bytes INTEGER NOT NULL DEFAULT 0,
    rows_json TEXT NOT NULL,
    files_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_psf_guard_removed_image_batch ON psf_guard_removed_image(batch_id);
CREATE INDEX IF NOT EXISTS idx_psf_guard_removed_image_target ON psf_guard_removed_image(target_id)";

/// Create the tombstone table. Safe to call repeatedly.
pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)
        .context("creating psf_guard_removed_image")?;
    // Tables from before purging lack its column.
    if !has_column(conn, "psf_guard_removed_image", "purged_at") {
        conn.execute(
            "ALTER TABLE psf_guard_removed_image ADD COLUMN purged_at INTEGER",
            [],
        )
        .context("adding psf_guard_removed_image.purged_at")?;
    }
    Ok(())
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

/// GUIDs of every removed frame, including frames removed with their
/// project, so a catalog pull leaves them out. Empty when nothing was ever
/// removed.
pub fn removed_guids(conn: &Connection) -> Result<HashSet<String>> {
    let mut guids: HashSet<String> = crate::commands::project_removal::removed_frame_guids(conn)?
        .into_iter()
        .collect();
    if table_exists(conn, "psf_guard_removed_image") {
        let mut statement =
            conn.prepare("SELECT acquired_image_guid FROM psf_guard_removed_image")?;
        for guid in statement.query_map([], |row| row.get::<_, String>(0))? {
            guids.insert(guid?);
        }
    }
    Ok(guids)
}

/// Lower-case file names of removed lights, so an import or upload of the
/// same file does not bring the frame back as a new one.
pub fn removed_file_names(conn: &Connection) -> Result<HashSet<String>> {
    let mut names: HashSet<String> = crate::commands::project_removal::removed_file_names(conn)?
        .into_iter()
        .collect();
    if table_exists(conn, "psf_guard_removed_image") {
        let mut statement = conn.prepare(
            "SELECT file_name FROM psf_guard_removed_image WHERE file_name IS NOT NULL AND file_name <> ''",
        )?;
        for name in statement.query_map([], |row| row.get::<_, String>(0))? {
            names.insert(name?);
        }
    }
    Ok(names)
}

/// Whether a removed reject's file has this name (compared in lower case),
/// so an upload of it is refused rather than bringing the frame back.
pub fn is_removed_reject_file(conn: &Connection, file_name: &str) -> Result<bool> {
    if !table_exists(conn, "psf_guard_removed_image") {
        return Ok(false);
    }
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM psf_guard_removed_image WHERE lower(file_name) = lower(?1))",
        [file_name],
        |row| row.get(0),
    )?)
}

/// Which frames a run may take: one project, one target, or the database.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub project_id: Option<i64>,
    pub target_id: Option<i64>,
}

/// What a removed file was to its frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    /// The light itself, where it is or in the reject archive.
    Light,
    /// A sidecar the reject archive moved with it.
    Sidecar,
    /// A calibrated or registered copy of the light.
    Copy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedFile {
    pub kind: FileKind,
    pub path: PathBuf,
    pub bytes: u64,
}

/// A frame the plan takes, and its files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedFrame {
    pub image_id: i64,
    pub guid: String,
    pub project_id: Option<i64>,
    pub target_id: Option<i64>,
    pub exposure_id: Option<i64>,
    pub target_name: String,
    pub file_name: Option<String>,
    pub acquired_at: Option<i64>,
    pub rejected_at: i64,
    pub reject_reason: Option<String>,
    pub files: Vec<PlannedFile>,
}

/// A rejected frame the plan leaves out, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedFrame {
    pub image_id: i64,
    pub guid: Option<String>,
    pub target_name: String,
    pub file_name: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalPlan {
    pub scope: Scope,
    pub min_age_days: u32,
    pub frames: Vec<PlannedFrame>,
    pub skipped: Vec<SkippedFrame>,
    /// Rejects in scope still inside the grace period.
    pub waiting: usize,
    /// When the first of them qualifies, in Unix seconds.
    pub next_eligible_at: Option<i64>,
    /// Frames whose files were not found; their rows still go.
    pub without_files: usize,
    pub bytes: u64,
    /// What Apply checks it is still removing: the frames and their files.
    pub digest: String,
}

pub struct PlanOptions<'a> {
    pub scope: Scope,
    pub min_age_days: u32,
    pub now: i64,
    /// Frames to leave in place whatever their grade: GUIDs a collaboration
    /// capture or report names.
    pub protected_guids: &'a BTreeSet<String>,
}

struct RejectRow {
    image_id: i64,
    guid: Option<String>,
    project_id: Option<i64>,
    target_id: Option<i64>,
    exposure_id: Option<i64>,
    target_name: String,
    acquired_at: Option<i64>,
    metadata: String,
    reject_reason: Option<String>,
    rejected_at: Option<i64>,
}

/// Index each image directory once; a directory that cannot be read is
/// left out.
fn trees_for(image_dirs: &[String], work: &Work<'_>) -> Result<Vec<Option<DirectoryTree>>> {
    let mut trees = Vec::with_capacity(image_dirs.len());
    for (index, dir) in image_dirs.iter().enumerate() {
        if work.stopped() {
            bail!(Stopped);
        }
        (work.progress)(Stage::Indexing, index, image_dirs.len());
        trees.push(DirectoryTree::build_multiple(&[Path::new(dir)]).ok());
    }
    (work.progress)(Stage::Indexing, image_dirs.len(), image_dirs.len());
    Ok(trees)
}

/// Which rejected frames qualify, with their files and the reason for each
/// one left out. Dates any rejects not yet dated first.
pub fn plan(
    conn: &Connection,
    image_dirs: &[String],
    options: &PlanOptions<'_>,
) -> Result<RemovalPlan> {
    plan_with(conn, image_dirs, options, &Work::quiet())
}

/// [`plan`], saying where it stands and stopping when asked, with
/// [`Stopped`].
pub fn plan_with(
    conn: &Connection,
    image_dirs: &[String],
    options: &PlanOptions<'_>,
    work: &Work<'_>,
) -> Result<RemovalPlan> {
    if !has_column(conn, "acquiredimage", "guid")
        || !has_column(conn, "acquiredimage", "gradingStatus")
    {
        bail!(
            "This database's Target Scheduler schema has no image GUIDs; removal needs them to restore frames."
        );
    }
    // Stamp new rejects with the preview's clock, so the stamps and the
    // cutoff below share one reading.
    crate::db::record_rejection_times_at(conn, options.now).context("dating rejected frames")?;
    let exposure = if has_column(conn, "acquiredimage", "exposureId") {
        "ai.exposureId"
    } else {
        "NULL"
    };
    let reason = if has_column(conn, "acquiredimage", "rejectreason") {
        "ai.rejectreason"
    } else {
        "NULL"
    };
    let mut statement = conn.prepare(&format!(
        "SELECT ai.Id, ai.guid, ai.projectId, ai.targetId, {exposure}, IFNULL(t.name, ''),
                ai.acquireddate, IFNULL(ai.metadata, ''), {reason}, r.rejected_at
         FROM acquiredimage ai
         LEFT JOIN target t ON t.Id = ai.targetId
         LEFT JOIN psf_guard_rejected_at r ON r.acquired_image_guid = ai.guid
         WHERE ai.gradingStatus = 2
           AND (?1 IS NULL OR ai.projectId = ?1)
           AND (?2 IS NULL OR ai.targetId = ?2)
         ORDER BY ai.targetId, ai.acquireddate, ai.Id"
    ))?;
    let rows = statement
        .query_map(
            params![options.scope.project_id, options.scope.target_id],
            |row| {
                Ok(RejectRow {
                    image_id: row.get(0)?,
                    guid: row
                        .get::<_, Option<String>>(1)?
                        .filter(|guid| !guid.trim().is_empty()),
                    project_id: row.get(2)?,
                    target_id: row.get(3)?,
                    exposure_id: row.get(4)?,
                    target_name: row.get(5)?,
                    acquired_at: row.get(6)?,
                    metadata: row.get(7)?,
                    reject_reason: row.get(8)?,
                    rejected_at: row.get(9)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // A file two rows name belongs to both: it stays while the other row does.
    let mut by_name: HashMap<String, Vec<i64>> = HashMap::new();
    let mut every = conn.prepare("SELECT Id, IFNULL(metadata, '') FROM acquiredimage")?;
    for row in every.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })? {
        let (id, metadata) = row?;
        if let Some(name) = reject_archive::parse_filename_from_metadata(&metadata) {
            by_name.entry(name.to_lowercase()).or_default().push(id);
        }
    }

    let cutoff = options.now - i64::from(options.min_age_days) * DAY;
    let trees = trees_for(image_dirs, work)?;
    let mut plan = RemovalPlan {
        scope: options.scope,
        min_age_days: options.min_age_days,
        ..Default::default()
    };
    let mut taking: Vec<(RejectRow, String)> = Vec::new();
    for row in rows {
        let file_name = reject_archive::parse_filename_from_metadata(&row.metadata);
        let skip = |plan: &mut RemovalPlan, row: &RejectRow, reason: String| {
            plan.skipped.push(SkippedFrame {
                image_id: row.image_id,
                guid: row.guid.clone(),
                target_name: row.target_name.clone(),
                file_name: file_name.clone(),
                reason,
            })
        };
        let Some(guid) = row.guid.clone() else {
            skip(
                &mut plan,
                &row,
                "it has no GUID; fill in GUIDs first".into(),
            );
            continue;
        };
        // New rejects are stamped with `now` above. A stamp past it (another
        // machine's clock, or one set back since) still counts as no younger
        // than now against the cutoff; the date it qualifies keeps the stamp.
        let stamped = row.rejected_at.unwrap_or(options.now);
        if stamped.min(options.now) > cutoff {
            plan.waiting += 1;
            let eligible = stamped + i64::from(options.min_age_days) * DAY;
            plan.next_eligible_at = Some(
                plan.next_eligible_at
                    .map_or(eligible, |at| at.min(eligible)),
            );
            continue;
        }
        if options.protected_guids.contains(&guid) {
            skip(&mut plan, &row, "it is in a collaboration report".into());
            continue;
        }
        if let Some(name) = &file_name
            && let Some(others) = by_name.get(&name.to_lowercase())
            && let Some(other) = others.iter().find(|id| **id != row.image_id)
        {
            skip(
                &mut plan,
                &row,
                format!("its file also backs frame {other}"),
            );
            continue;
        }
        taking.push((row, guid));
    }

    let checking = taking.len();
    for (index, (row, guid)) in taking.into_iter().enumerate() {
        if work.stopped() {
            bail!(Stopped);
        }
        (work.progress)(Stage::Checking, index, checking);
        let file_name = reject_archive::parse_filename_from_metadata(&row.metadata);
        let files = frame_files(conn, image_dirs, &trees, &row, &guid, file_name.as_deref())?;
        if !files.iter().any(|file| file.kind == FileKind::Light) {
            plan.without_files += 1;
        }
        plan.bytes += files.iter().map(|file| file.bytes).sum::<u64>();
        plan.frames.push(PlannedFrame {
            image_id: row.image_id,
            guid,
            project_id: row.project_id,
            target_id: row.target_id,
            exposure_id: row.exposure_id,
            target_name: row.target_name,
            file_name,
            acquired_at: row.acquired_at,
            rejected_at: row.rejected_at.unwrap_or(options.now),
            reject_reason: row.reject_reason,
            files,
        });
    }
    plan.digest = digest_of(&plan.frames);
    Ok(plan)
}

/// The frames and their files, in order, as a hex SHA-256.
fn digest_of(frames: &[PlannedFrame]) -> String {
    let mut hasher = Sha256::new();
    for frame in frames {
        hasher.update(frame.guid.as_bytes());
        hasher.update([0]);
        for file in &frame.files {
            hasher.update(file.path.to_string_lossy().as_bytes());
            hasher.update([0]);
        }
        hasher.update([1]);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn planned(kind: FileKind, path: PathBuf) -> Option<PlannedFile> {
    let metadata = std::fs::metadata(&path).ok()?;
    metadata.is_file().then_some(PlannedFile {
        kind,
        bytes: metadata.len(),
        path,
    })
}

/// A frame's files: the light (from its upload, the reject archive, or the
/// image directories), the archive's sidecars, and its paired copies.
fn frame_files(
    conn: &Connection,
    image_dirs: &[String],
    trees: &[Option<DirectoryTree>],
    row: &RejectRow,
    guid: &str,
    file_name: Option<&str>,
) -> Result<Vec<PlannedFile>> {
    let mut files = Vec::new();
    let archived = if reject_archive::archive_table_exists(conn) {
        reject_archive::get_archive_record_by_guid(conn, guid)?
    } else {
        None
    };
    let mut light: Option<PathBuf> = None;
    if let Some(record) = &archived {
        let path = PathBuf::from(&record.archive_path);
        if path.is_file() {
            light = Some(path.clone());
            if let Some(dir) = path.parent() {
                for sidecar in &record.sidecar_files {
                    files.extend(planned(FileKind::Sidecar, dir.join(sidecar)));
                }
            }
        }
        for copy in &record.copy_files {
            files.extend(planned(FileKind::Copy, PathBuf::from(&copy.archive_path)));
        }
    }
    if light.is_none() && table_exists(conn, "psf_guard_remote_image_file") {
        let uploaded: Option<String> = conn
            .query_row(
                "SELECT source_path FROM psf_guard_remote_image_file WHERE acquiredimage_id = ?1",
                [row.image_id],
                |row| row.get(0),
            )
            .optional()?;
        light = uploaded.map(PathBuf::from).filter(|path| path.is_file());
    }
    if light.is_none()
        && let Some(name) = file_name
    {
        let day = row
            .acquired_at
            .and_then(|at| chrono::DateTime::from_timestamp(at, 0))
            .map(|at| at.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        'dirs: for (index, dir) in image_dirs.iter().enumerate() {
            for candidate in get_possible_paths(dir, &day, &row.target_name, name) {
                if candidate.is_file() {
                    light = Some(candidate);
                    break 'dirs;
                }
            }
            if let Some(Some(tree)) = trees.get(index)
                && let Some(path) = tree.find_file_first(name)
                && path.is_file()
            {
                light = Some(path.clone());
                break;
            }
        }
    }
    if let Some(path) = &light {
        files.extend(planned(FileKind::Light, path.clone()));
    }
    let archived_copies: HashSet<String> = archived
        .iter()
        .flat_map(|record| {
            record
                .copy_files
                .iter()
                .map(|copy| copy.derivative_uuid.clone())
        })
        .collect();
    for record in crate::frame_derivatives::derivatives_for_light(conn, guid)? {
        if record.primary_source || archived_copies.contains(&record.derivative_uuid) {
            continue;
        }
        let candidates = trees
            .iter()
            .flatten()
            .filter_map(|tree| tree.find_file(&record.file_name))
            .flatten();
        if let Some(path) = crate::frame_derivatives::resolve_record_path(&record, candidates)
            && Some(&path) != light.as_ref()
        {
            files.extend(planned(FileKind::Copy, path));
        }
    }
    files.dedup_by(|a, b| a.path == b.path);
    Ok(files)
}

/// A removed file, where it was and where it waits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashedFile {
    pub kind: FileKind,
    pub original: PathBuf,
    pub trash: PathBuf,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedFrame {
    pub image_id: i64,
    pub guid: String,
    pub project_id: Option<i64>,
    pub target_id: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyReport {
    pub batch_id: String,
    pub removed: Vec<RemovedFrame>,
    /// Frames the plan took that Apply could not remove, and why.
    pub failed: Vec<SkippedFrame>,
    pub files_moved: usize,
    pub bytes: u64,
    pub trash_until: i64,
    /// Stopped before every frame was removed: `removed` lists the frames
    /// that went, and the rest stay in the catalog.
    #[serde(default)]
    pub stopped: bool,
}

/// Where a file waits: under the image directory that holds it, the same
/// path below `.psf-guard-trash/<batch>`; outside every image directory, in
/// a `.psf-guard-trash/<batch>` folder beside it. Never an existing file.
fn trash_path(image_dirs: &[String], batch: &str, file: &Path) -> PathBuf {
    let placed = image_dirs.iter().find_map(|dir| {
        let relative = file.strip_prefix(dir).ok()?;
        Some(Path::new(dir).join(TRASH_DIR).join(batch).join(relative))
    });
    let desired = placed.unwrap_or_else(|| {
        let parent = file.parent().unwrap_or_else(|| Path::new("."));
        parent
            .join(TRASH_DIR)
            .join(batch)
            .join(file.file_name().unwrap_or_default())
    });
    free_path(&desired)
}

/// `desired`, or the first `name.N.ext` beside it that does not exist.
fn free_path(desired: &Path) -> PathBuf {
    if !desired.exists() {
        return desired.to_path_buf();
    }
    let dir = desired.parent().unwrap_or_else(|| Path::new("."));
    let stem = desired.file_stem().unwrap_or_default().to_string_lossy();
    let ext = desired.extension().map(|ext| ext.to_string_lossy());
    for n in 1u32.. {
        let name = match &ext {
            Some(ext) => format!("{stem}.{n}.{ext}"),
            None => format!("{stem}.{n}"),
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("no free name beside {}", desired.display())
}

/// Move a file, across file systems when a rename cannot.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if to.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
    }
}

/// Put moved files back, newest first. Best effort: a file that cannot go
/// back is left where it is and reported.
fn move_back(moved: &[(PathBuf, PathBuf)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (original, now) in moved.iter().rev() {
        if let Err(error) = move_file(now, original) {
            problems.push(format!(
                "{} could not move back from {}: {error}",
                original.display(),
                now.display()
            ));
        }
    }
    problems
}

fn new_batch_id(now: i64) -> String {
    let stamp = chrono::DateTime::from_timestamp(now, 0)
        .map(|at| at.format("%Y%m%d-%H%M%S").to_string())
        .unwrap_or_else(|| now.to_string());
    format!(
        "{stamp}-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

/// Remove the plan's frames. Refuses when the frames or their files changed
/// since the plan was made (`expected_digest`): a new plan is taken here and
/// compared.
pub fn apply(
    conn: &Connection,
    image_dirs: &[String],
    options: &PlanOptions<'_>,
    expected_digest: &str,
    retention_days: u32,
) -> Result<ApplyReport> {
    apply_with(
        conn,
        image_dirs,
        options,
        expected_digest,
        retention_days,
        &Work::quiet(),
    )
}

/// [`apply`], saying where it stands. A stop while it checks changes
/// nothing ([`Stopped`]); one while it moves files ends after the frame in
/// hand, with the frames removed so far and `stopped` set.
pub fn apply_with(
    conn: &Connection,
    image_dirs: &[String],
    options: &PlanOptions<'_>,
    expected_digest: &str,
    retention_days: u32,
    work: &Work<'_>,
) -> Result<ApplyReport> {
    let current = plan_with(conn, image_dirs, options, work)?;
    if current.digest != expected_digest {
        bail!(StaleRemovalPlan);
    }
    ensure_schema(conn)?;
    let batch_id = new_batch_id(options.now);
    let trash_until = options.now + i64::from(retention_days) * DAY;
    let mut report = ApplyReport {
        batch_id: batch_id.clone(),
        trash_until,
        ..Default::default()
    };
    for (index, frame) in current.frames.iter().enumerate() {
        if work.stopped() {
            report.stopped = true;
            break;
        }
        (work.progress)(Stage::Moving, index, current.frames.len());
        match remove_one(conn, image_dirs, &batch_id, frame, options.now, trash_until) {
            Ok(trashed) => {
                report.files_moved += trashed.len();
                report.bytes += trashed.iter().map(|file| file.bytes).sum::<u64>();
                report.removed.push(RemovedFrame {
                    image_id: frame.image_id,
                    guid: frame.guid.clone(),
                    project_id: frame.project_id,
                    target_id: frame.target_id,
                });
            }
            Err(error) => report.failed.push(SkippedFrame {
                image_id: frame.image_id,
                guid: Some(frame.guid.clone()),
                target_name: frame.target_name.clone(),
                file_name: frame.file_name.clone(),
                reason: format!("{error:#}"),
            }),
        }
    }
    if !report.removed.is_empty() {
        crate::db::reconcile_accepted_counts(conn)?;
        crate::db::record_rejection_times(conn)?;
    }
    Ok(report)
}

/// Apply found the frames or their files changed since the preview.
#[derive(Debug)]
pub struct StaleRemovalPlan;

impl std::fmt::Display for StaleRemovalPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The rejects changed since the preview; preview again.")
    }
}

impl std::error::Error for StaleRemovalPlan {}

/// Everything a frame's rows were, so restore can put them back. Empty
/// once purged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct RowSnapshot {
    acquiredimage: Map<String, Json>,
    imagedata: Vec<Map<String, Json>>,
    frame_derivative: Vec<Map<String, Json>>,
    remote_image_file: Option<Map<String, Json>>,
    archive: Option<Map<String, Json>>,
}

fn to_json(value: SqlValue) -> Json {
    match value {
        SqlValue::Null => Json::Null,
        SqlValue::Integer(n) => Json::from(n),
        SqlValue::Real(x) => serde_json::Number::from_f64(x).map_or(Json::Null, Json::Number),
        SqlValue::Text(text) => Json::String(text),
        SqlValue::Blob(bytes) => {
            let mut blob = Map::new();
            blob.insert(
                "$blob".into(),
                Json::String(base64::engine::general_purpose::STANDARD.encode(bytes)),
            );
            Json::Object(blob)
        }
    }
}

fn from_json(value: &Json) -> SqlValue {
    match value {
        Json::Null => SqlValue::Null,
        Json::Bool(flag) => SqlValue::Integer(i64::from(*flag)),
        Json::Number(n) => n
            .as_i64()
            .map(SqlValue::Integer)
            .unwrap_or_else(|| SqlValue::Real(n.as_f64().unwrap_or(0.0))),
        Json::String(text) => SqlValue::Text(text.clone()),
        Json::Object(object) => object
            .get("$blob")
            .and_then(Json::as_str)
            .and_then(|text| base64::engine::general_purpose::STANDARD.decode(text).ok())
            .map_or(SqlValue::Null, SqlValue::Blob),
        Json::Array(_) => SqlValue::Text(value.to_string()),
    }
}

fn snapshot(
    conn: &Connection,
    table: &str,
    key: &str,
    value: &dyn rusqlite::ToSql,
) -> Result<Vec<Map<String, Json>>> {
    if !table_exists(conn, table) {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(&format!("SELECT * FROM \"{table}\" WHERE \"{key}\" = ?1"))?;
    let names: Vec<String> = statement
        .column_names()
        .iter()
        .map(|name| name.to_string())
        .collect();
    let mut rows = statement.query([value])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut map = Map::new();
        for (index, name) in names.iter().enumerate() {
            map.insert(name.clone(), to_json(row.get::<_, SqlValue>(index)?));
        }
        out.push(map);
    }
    Ok(out)
}

fn remove_one(
    conn: &Connection,
    image_dirs: &[String],
    batch_id: &str,
    frame: &PlannedFrame,
    now: i64,
    trash_until: i64,
) -> Result<Vec<TrashedFile>> {
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut trashed = Vec::new();
    for file in &frame.files {
        let trash = trash_path(image_dirs, batch_id, &file.path);
        if let Err(error) = move_file(&file.path, &trash) {
            let problems = move_back(&moved);
            bail!(
                "{} could not move to the trash: {error}{}",
                file.path.display(),
                if problems.is_empty() {
                    String::new()
                } else {
                    format!("; {}", problems.join("; "))
                }
            );
        }
        moved.push((file.path.clone(), trash.clone()));
        trashed.push(TrashedFile {
            kind: file.kind,
            original: file.path.clone(),
            trash,
            bytes: file.bytes,
        });
    }
    let committed = (|| -> Result<()> {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        let rows = RowSnapshot {
            acquiredimage: snapshot(&tx, "acquiredimage", "Id", &frame.image_id)?
                .into_iter()
                .next()
                .context("the frame's row is gone")?,
            imagedata: snapshot(&tx, "imagedata", "acquiredimageid", &frame.image_id)?,
            frame_derivative: snapshot(
                &tx,
                "psf_guard_frame_derivative",
                "acquired_image_guid",
                &frame.guid,
            )?,
            remote_image_file: snapshot(
                &tx,
                "psf_guard_remote_image_file",
                "acquiredimage_id",
                &frame.image_id,
            )?
            .into_iter()
            .next(),
            archive: snapshot(&tx, "psf_guard_archive", "acquired_image_guid", &frame.guid)?
                .into_iter()
                .next(),
        };
        tx.execute(
            "INSERT INTO psf_guard_removed_image (
                acquired_image_guid, acquired_image_id, project_id, target_id, exposure_id,
                file_name, batch_id, removed_at, rejected_at, trash_until, bytes, rows_json, files_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                frame.guid,
                frame.image_id,
                frame.project_id,
                frame.target_id,
                frame.exposure_id,
                frame.file_name.as_ref().map(|name| name.to_lowercase()),
                batch_id,
                now,
                frame.rejected_at,
                trash_until,
                trashed.iter().map(|file| file.bytes).sum::<u64>() as i64,
                serde_json::to_string(&rows)?,
                serde_json::to_string(&trashed)?,
            ],
        )?;
        for (table, key, value) in [
            (
                "imagedata",
                "acquiredimageid",
                Box::new(frame.image_id) as Box<dyn rusqlite::ToSql>,
            ),
            (
                "psf_guard_frame_derivative",
                "acquired_image_guid",
                Box::new(frame.guid.clone()),
            ),
            (
                "psf_guard_remote_image_file",
                "acquiredimage_id",
                Box::new(frame.image_id),
            ),
            (
                "psf_guard_archive",
                "acquired_image_guid",
                Box::new(frame.guid.clone()),
            ),
            (
                "psf_guard_rejected_at",
                "acquired_image_guid",
                Box::new(frame.guid.clone()),
            ),
        ] {
            if table_exists(&tx, table) {
                tx.execute(
                    &format!("DELETE FROM \"{table}\" WHERE \"{key}\" = ?1"),
                    [value],
                )?;
            }
        }
        tx.execute("DELETE FROM acquiredimage WHERE Id = ?1", [frame.image_id])?;
        tx.commit()?;
        Ok(())
    })();
    if let Err(error) = committed {
        let problems = move_back(&moved);
        bail!(
            "its rows could not be removed: {error:#}{}",
            if problems.is_empty() {
                String::new()
            } else {
                format!("; {}", problems.join("; "))
            }
        );
    }
    Ok(trashed)
}

/// Folders of a database's cache whose files a frame's row Id names.
const IMAGE_CACHE_DIRS: [&str; 5] = ["previews", "annotated", "stars", "psf_multi", "stats"];

/// Delete the cache files a database's cache folder (`<cache root>/<slug>`)
/// keeps for these frames: previews, annotated previews, star lists, PSF
/// views, statistics, plate solves and satellite predictions. Target
/// Scheduler can give a removed frame's row Id to the next frame, which must
/// not inherit them. Returns how many files went.
pub fn forget_image_caches(cache_dir: &Path, frames: &[RemovedFrame]) -> usize {
    let mut removed = 0;
    for frame in frames {
        for folder in ["astrometry", "satellites"] {
            if std::fs::remove_file(
                cache_dir
                    .join(folder)
                    .join(format!("{}.json", frame.image_id)),
            )
            .is_ok()
            {
                removed += 1;
            }
        }
    }
    // Keys open with the frame's row, project and target Ids, after a name
    // and version such as `annotated_v4_` when there is one.
    let owners: HashSet<(i64, i64, i64)> = frames
        .iter()
        .filter_map(|frame| Some((frame.image_id, frame.project_id?, frame.target_id?)))
        .collect();
    let owner = |name: &str| -> Option<(i64, i64, i64)> {
        let mut parts = name
            .split('_')
            .skip_while(|part| !part.chars().all(|c| c.is_ascii_digit()) || part.is_empty());
        Some((
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        ))
    };
    for folder in IMAGE_CACHE_DIRS {
        let Ok(entries) = std::fs::read_dir(cache_dir.join(folder)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if owner(&name).is_some_and(|ids| owners.contains(&ids))
                && std::fs::remove_file(entry.path()).is_ok()
            {
                removed += 1;
            }
        }
    }
    removed
}

/// One removal batch, as the list of removals shows it./// One removal batch, as the list of removals shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedBatch {
    pub batch_id: String,
    pub removed_at: i64,
    pub frames: usize,
    pub bytes: i64,
    pub trash_until: i64,
    /// Frames whose files are already gone from the trash.
    pub files_deleted: usize,
    /// Frames whose saved rows were purged; only their markers remain.
    pub purged: usize,
}

/// A removed frame, as the list of removals shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedEntry {
    pub guid: String,
    pub image_id: i64,
    pub batch_id: String,
    pub target_id: Option<i64>,
    pub file_name: Option<String>,
    pub removed_at: i64,
    pub rejected_at: Option<i64>,
    pub trash_until: i64,
    pub files_deleted_at: Option<i64>,
    pub bytes: i64,
}

/// Every removal batch, newest first.
pub fn batches(conn: &Connection) -> Result<Vec<RemovedBatch>> {
    if !table_exists(conn, "psf_guard_removed_image") {
        return Ok(Vec::new());
    }
    let purged = if has_column(conn, "psf_guard_removed_image", "purged_at") {
        "SUM(purged_at IS NOT NULL)"
    } else {
        "0"
    };
    let mut statement = conn.prepare(&format!(
        "SELECT batch_id, MIN(removed_at), COUNT(*), SUM(bytes), MAX(trash_until),
                SUM(files_deleted_at IS NOT NULL), {purged}
         FROM psf_guard_removed_image GROUP BY batch_id ORDER BY MIN(removed_at) DESC, batch_id"
    ))?;
    let rows = statement
        .query_map([], |row| {
            Ok(RemovedBatch {
                batch_id: row.get(0)?,
                removed_at: row.get(1)?,
                frames: row.get::<_, i64>(2)? as usize,
                bytes: row.get(3)?,
                trash_until: row.get(4)?,
                files_deleted: row.get::<_, i64>(5)? as usize,
                purged: row.get::<_, i64>(6)? as usize,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// The frames of one batch, or of every batch.
pub fn removed(conn: &Connection, batch_id: Option<&str>) -> Result<Vec<RemovedEntry>> {
    if !table_exists(conn, "psf_guard_removed_image") {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT acquired_image_guid, acquired_image_id, batch_id, target_id, file_name,
                removed_at, rejected_at, trash_until, files_deleted_at, bytes
         FROM psf_guard_removed_image WHERE ?1 IS NULL OR batch_id = ?1
         ORDER BY removed_at DESC, target_id, file_name",
    )?;
    let rows = statement
        .query_map([batch_id], |row| {
            Ok(RemovedEntry {
                guid: row.get(0)?,
                image_id: row.get(1)?,
                batch_id: row.get(2)?,
                target_id: row.get(3)?,
                file_name: row.get(4)?,
                removed_at: row.get(5)?,
                rejected_at: row.get(6)?,
                trash_until: row.get(7)?,
                files_deleted_at: row.get(8)?,
                bytes: row.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Which removals to restore: a batch, or frames by GUID.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreSelection {
    pub batch_id: Option<String>,
    #[serde(default)]
    pub guids: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoredFrame {
    pub guid: String,
    /// The frame's row Id now: its old one when that was still free.
    pub image_id: i64,
    pub project_id: Option<i64>,
    pub target_id: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreReport {
    pub restored: Vec<RestoredFrame>,
    pub failed: Vec<SkippedFrame>,
    /// Files that came back under another name because theirs was taken.
    pub renamed: Vec<String>,
    /// Stopped before every frame came back: the rest stay removed.
    #[serde(default)]
    pub stopped: bool,
}

struct Tombstone {
    guid: String,
    image_id: i64,
    project_id: Option<i64>,
    target_id: Option<i64>,
    file_name: Option<String>,
    rejected_at: Option<i64>,
    files_deleted_at: Option<i64>,
    purged: bool,
    removed_at: i64,
    rows: RowSnapshot,
    files: Vec<TrashedFile>,
}

fn tombstones(conn: &Connection, selection: &RestoreSelection) -> Result<Vec<Tombstone>> {
    if !table_exists(conn, "psf_guard_removed_image") {
        return Ok(Vec::new());
    }
    let purged = if has_column(conn, "psf_guard_removed_image", "purged_at") {
        "purged_at IS NOT NULL"
    } else {
        "0"
    };
    let mut statement = conn.prepare(&format!(
        "SELECT acquired_image_guid, acquired_image_id, target_id, file_name, rejected_at,
                files_deleted_at, rows_json, files_json, batch_id, project_id, {purged}, removed_at
         FROM psf_guard_removed_image ORDER BY removed_at, acquired_image_id"
    ))?;
    let wanted: HashSet<&str> = selection.guids.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let guid: String = row.get(0)?;
        let batch: String = row.get(8)?;
        let chosen =
            selection.batch_id.as_deref() == Some(batch.as_str()) || wanted.contains(guid.as_str());
        if !chosen {
            continue;
        }
        out.push(Tombstone {
            guid,
            image_id: row.get(1)?,
            project_id: row.get(9)?,
            target_id: row.get(2)?,
            file_name: row.get(3)?,
            rejected_at: row.get(4)?,
            files_deleted_at: row.get(5)?,
            purged: row.get(10)?,
            removed_at: row.get(11)?,
            rows: serde_json::from_str(&row.get::<_, String>(6)?)?,
            files: serde_json::from_str(&row.get::<_, String>(7)?)?,
        });
    }
    Ok(out)
}

/// Insert a snapshot row into `table`: only the columns the table still
/// has, with `overrides` replacing values (and `None` dropping a column so
/// SQLite picks it). Returns the new rowid.
fn insert_row(
    conn: &Connection,
    table: &str,
    row: &Map<String, Json>,
    overrides: &[(&str, Option<SqlValue>)],
) -> Result<i64> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
    let present: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;
    let mut columns = Vec::new();
    let mut values = Vec::new();
    for column in &present {
        let replaced = overrides
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(column));
        match replaced {
            Some((_, None)) => continue,
            Some((_, Some(value))) => values.push(value.clone()),
            None => match row
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(column))
            {
                Some((_, value)) => values.push(from_json(value)),
                None => continue,
            },
        }
        columns.push(format!("\"{column}\""));
    }
    conn.execute(
        &format!(
            "INSERT INTO \"{table}\" ({}) VALUES ({})",
            columns.join(", "),
            vec!["?"; columns.len()].join(", ")
        ),
        rusqlite::params_from_iter(values),
    )?;
    Ok(conn.last_insert_rowid())
}

/// Put removed frames back: their files from the trash, their rows from the
/// tombstone. A frame whose files were deleted from the trash, or whose
/// GUID is in the catalog again, stays removed and is named.
pub fn restore(conn: &Connection, selection: &RestoreSelection) -> Result<RestoreReport> {
    restore_with(conn, selection, &Work::quiet())
}

/// [`restore`], saying where it stands, and stopping after the frame in
/// hand when asked.
pub fn restore_with(
    conn: &Connection,
    selection: &RestoreSelection,
    work: &Work<'_>,
) -> Result<RestoreReport> {
    let mut report = RestoreReport::default();
    let tombstones = tombstones(conn, selection)?;
    let total = tombstones.len();
    for (index, tombstone) in tombstones.into_iter().enumerate() {
        if work.stopped() {
            report.stopped = true;
            break;
        }
        (work.progress)(Stage::Restoring, index, total);
        let fail = |report: &mut RestoreReport, reason: String| {
            report.failed.push(SkippedFrame {
                image_id: tombstone.image_id,
                guid: Some(tombstone.guid.clone()),
                target_name: String::new(),
                file_name: tombstone.file_name.clone(),
                reason,
            })
        };
        if tombstone.purged {
            fail(&mut report, "its saved rows were purged".into());
            continue;
        }
        if tombstone.files_deleted_at.is_some() && !tombstone.files.is_empty() {
            fail(
                &mut report,
                "its files were deleted when the trash was emptied".into(),
            );
            continue;
        }
        let present: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM acquiredimage WHERE guid = ?1)",
            [&tombstone.guid],
            |row| row.get(0),
        )?;
        if present {
            fail(
                &mut report,
                "the catalog has a frame with its GUID again".into(),
            );
            continue;
        }
        // A project deleted after this frame was removed may have given its
        // Id to a new project, which the frame must not join.
        if let Some(project) = tombstone.project_id
            && crate::commands::project_removal::project_removed_since(
                conn,
                project,
                tombstone.removed_at,
            )?
        {
            fail(
                &mut report,
                "its project is no longer in the catalog".into(),
            );
            continue;
        }
        // Files first, each to its old place or a free name beside it.
        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut placed: HashMap<PathBuf, PathBuf> = HashMap::new();
        let mut problem = None;
        for file in &tombstone.files {
            let destination = if file.original.exists() {
                reject_archive::unique_restore_dest(&file.original)
            } else {
                file.original.clone()
            };
            if let Err(error) = move_file(&file.trash, &destination) {
                problem = Some(format!(
                    "{} could not come back from the trash: {error}",
                    file.original.display()
                ));
                break;
            }
            if destination != file.original {
                report.renamed.push(destination.display().to_string());
            }
            moved.push((file.trash.clone(), destination.clone()));
            placed.insert(file.original.clone(), destination);
        }
        if let Some(problem) = problem {
            // Each pair is (trash, where it went), so this puts them back in the trash.
            let mut problems = move_back(&moved);
            problems.insert(0, problem);
            fail(&mut report, problems.join("; "));
            continue;
        }
        let committed = (|| -> Result<i64> {
            let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
            let rows = &tombstone.rows;
            let free: bool = !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM acquiredimage WHERE Id = ?1)",
                [tombstone.image_id],
                |row| row.get::<_, bool>(0),
            )?;
            // The frame returns to its target as the catalog has it now. A
            // target merge records the destination in the tombstone, and a
            // target move or project merge changes the target's project.
            let saved_target = rows
                .acquiredimage
                .iter()
                .find(|(column, _)| column.eq_ignore_ascii_case("targetId"))
                .and_then(|(_, value)| value.as_i64());
            let target_id = tombstone.target_id.or(saved_target);
            let current_project: Option<i64> = match target_id {
                Some(target_id) => tx
                    .query_row(
                        "SELECT projectid FROM target WHERE Id = ?1",
                        [target_id],
                        |row| row.get::<_, Option<i64>>(0),
                    )
                    .optional()?
                    .flatten(),
                None => None,
            };
            let mut overrides = vec![("Id", free.then_some(SqlValue::Integer(tombstone.image_id)))];
            if let (Some(target), Some(project)) = (target_id, current_project) {
                overrides.push(("targetId", Some(SqlValue::Integer(target))));
                overrides.push(("projectId", Some(SqlValue::Integer(project))));
            }
            let id = insert_row(&tx, "acquiredimage", &rows.acquiredimage, &overrides)?;
            for thumbnail in &rows.imagedata {
                insert_row(
                    &tx,
                    "imagedata",
                    thumbnail,
                    &[
                        ("Id", None),
                        ("acquiredimageid", Some(SqlValue::Integer(id))),
                    ],
                )?;
            }
            if !rows.frame_derivative.is_empty() {
                crate::frame_derivatives::ensure_schema(&tx)?;
                for record in &rows.frame_derivative {
                    insert_row(&tx, "psf_guard_frame_derivative", record, &[])?;
                }
            }
            if let Some(upload) = &rows.remote_image_file
                && table_exists(&tx, "psf_guard_remote_image_file")
            {
                let path = upload
                    .get("source_path")
                    .and_then(Json::as_str)
                    .map(PathBuf::from)
                    .and_then(|path| placed.get(&path).cloned().or(Some(path)));
                insert_row(
                    &tx,
                    "psf_guard_remote_image_file",
                    upload,
                    &[
                        ("acquiredimage_id", Some(SqlValue::Integer(id))),
                        (
                            "source_path",
                            path.map(|path| SqlValue::Text(path.display().to_string())),
                        ),
                    ],
                )?;
            }
            if let Some(archive) = &rows.archive {
                reject_archive::ensure_archive_schema(&tx)?;
                let path = archive
                    .get("archive_path")
                    .and_then(Json::as_str)
                    .map(PathBuf::from)
                    .and_then(|path| placed.get(&path).cloned().or(Some(path)));
                insert_row(
                    &tx,
                    "psf_guard_archive",
                    archive,
                    &[
                        ("acquired_image_id", Some(SqlValue::Integer(id))),
                        (
                            "archive_path",
                            path.map(|path| SqlValue::Text(path.display().to_string())),
                        ),
                    ],
                )?;
            }
            if let Some(rejected_at) = tombstone.rejected_at {
                tx.execute(
                    "INSERT OR REPLACE INTO psf_guard_rejected_at (acquired_image_guid, rejected_at) VALUES (?1, ?2)",
                    params![tombstone.guid, rejected_at],
                )
                .ok();
            }
            tx.execute(
                "DELETE FROM psf_guard_removed_image WHERE acquired_image_guid = ?1",
                [&tombstone.guid],
            )?;
            tx.commit()?;
            Ok(id)
        })();
        match committed {
            Ok(id) => report.restored.push(RestoredFrame {
                guid: tombstone.guid.clone(),
                image_id: id,
                project_id: tombstone.project_id,
                target_id: tombstone.target_id,
            }),
            Err(error) => {
                let mut problems = move_back(&moved);
                problems.insert(0, format!("its rows could not come back: {error:#}"));
                fail(&mut report, problems.join("; "));
            }
        }
    }
    if !report.restored.is_empty() {
        crate::db::reconcile_accepted_counts(conn)?;
    }
    Ok(report)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgeReport {
    pub frames: usize,
    /// Bytes of saved rows and file lists dropped from the database.
    pub bytes: u64,
}

/// Forget the saved rows of removals whose files are gone from the trash:
/// those frames can no longer be restored, so their row data, thumbnails
/// and file lists only take space. Each keeps a marker (its GUID, file
/// name, target and plan) so Sync pulls and imports still leave it out and
/// plan progress still counts it. One batch, or every one when `None`.
pub fn purge(conn: &Connection, batch_id: Option<&str>, now: i64) -> Result<PurgeReport> {
    let mut report = PurgeReport::default();
    if !table_exists(conn, "psf_guard_removed_image") {
        return Ok(report);
    }
    ensure_schema(conn)?;
    let due =
        "files_deleted_at IS NOT NULL AND purged_at IS NULL AND (?1 IS NULL OR batch_id = ?1)";
    let (frames, bytes): (i64, Option<i64>) = conn.query_row(
        &format!("SELECT COUNT(*), SUM(LENGTH(rows_json) + LENGTH(files_json)) FROM psf_guard_removed_image WHERE {due}"),
        [batch_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    conn.execute(
        &format!("UPDATE psf_guard_removed_image SET rows_json = '{{}}', files_json = '[]', purged_at = ?2 WHERE {due}"),
        params![batch_id, now],
    )?;
    report.frames = frames as usize;
    report.bytes = bytes.unwrap_or(0) as u64;
    Ok(report)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashReport {
    pub frames: usize,
    pub files_deleted: usize,
    pub bytes: u64,
    pub problems: Vec<String>,
    /// Stopped before every removal past its retention was emptied.
    #[serde(default)]
    pub stopped: bool,
}

/// Delete the files of every removal past its retention, and mark those
/// frames as having none. The tombstones stay.
pub fn empty_trash(conn: &Connection, now: i64) -> Result<TrashReport> {
    empty_trash_with(conn, now, &Work::quiet())
}

/// [`empty_trash`], saying where it stands, and stopping after the frame in
/// hand when asked.
pub fn empty_trash_with(conn: &Connection, now: i64, work: &Work<'_>) -> Result<TrashReport> {
    let mut report = TrashReport::default();
    if !table_exists(conn, "psf_guard_removed_image") {
        return Ok(report);
    }
    let due: Vec<(String, String)> = {
        let mut statement = conn.prepare(
            "SELECT acquired_image_guid, files_json FROM psf_guard_removed_image
             WHERE files_deleted_at IS NULL AND trash_until <= ?1",
        )?;
        statement
            .query_map([now], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut emptied_dirs: BTreeSet<PathBuf> = BTreeSet::new();
    let total = due.len();
    for (index, (guid, files)) in due.into_iter().enumerate() {
        if work.stopped() {
            report.stopped = true;
            break;
        }
        (work.progress)(Stage::Deleting, index, total);
        let files: Vec<TrashedFile> = serde_json::from_str(&files)?;
        let mut kept = false;
        for file in &files {
            match std::fs::remove_file(&file.trash) {
                Ok(()) => {
                    report.files_deleted += 1;
                    report.bytes += file.bytes;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    kept = true;
                    report.problems.push(format!(
                        "{} could not be deleted: {error}",
                        file.trash.display()
                    ));
                }
            }
            if let Some(parent) = file.trash.parent() {
                emptied_dirs.insert(parent.to_path_buf());
            }
        }
        if !kept {
            conn.execute(
                "UPDATE psf_guard_removed_image SET files_deleted_at = ?2 WHERE acquired_image_guid = ?1",
                params![guid, now],
            )?;
            report.frames += 1;
        }
    }
    // Empty folders go, up to the trash folder itself.
    for dir in emptied_dirs.into_iter().rev() {
        let mut current = Some(dir.as_path());
        while let Some(path) = current {
            if path.file_name().is_some_and(|name| name == TRASH_DIR) {
                break;
            }
            if std::fs::remove_dir(path).is_err() {
                break;
            }
            current = path.parent();
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_derivatives::{self, DerivativeRecord};
    use crate::image_io::{FrameKind, KindEvidence, ProcessingSteps, Producer};

    const NOW: i64 = 2_000_000_000;

    struct Catalog {
        _dir: tempfile::TempDir,
        root: PathBuf,
        conn: Connection,
    }

    impl Catalog {
        fn images(&self) -> Vec<String> {
            vec![self.root.join("images").display().to_string()]
        }
        fn count(&self, sql: &str) -> i64 {
            self.conn.query_row(sql, [], |row| row.get(0)).unwrap()
        }
    }

    /// One target with three frames on disk: two rejected (one with a
    /// thumbnail and a calibrated copy), one accepted.
    fn catalog() -> Catalog {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let light = root.join("images/M31/2026-10-01/LIGHT");
        let process = root.join("images/M31/_Process/calibrated");
        std::fs::create_dir_all(&light).unwrap();
        std::fs::create_dir_all(&process).unwrap();
        for name in ["M31_L_001.fits", "M31_L_002.fits", "M31_L_003.fits"] {
            std::fs::write(light.join(name), vec![7u8; 1000]).unwrap();
        }
        std::fs::write(process.join("M31_L_001_c.xisf"), vec![9u8; 500]).unwrap();
        let conn = Connection::open(root.join("catalog.sqlite")).unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        conn.execute_batch(
            r#"INSERT INTO project (Id, profileId, name, guid) VALUES (1, 'p', 'M31', 'pg');
             INSERT INTO target (Id, name, active, epochcode, projectId, guid) VALUES (10, 'M31', 1, 0, 1, 'tg');
             INSERT INTO exposuretemplate (Id, profileId, name, filtername, guid) VALUES (1, 'p', 'L', 'L', 'xg');
             INSERT INTO exposureplan (Id, profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, guid)
                VALUES (100, 'p', 300, 10, 3, 5, 10, 1, 'eg');
             INSERT INTO acquiredimage
                (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid, exposureId, rejectreason) VALUES
                (1, 1, 10, 1759300000, 'L', 2, '{"FileName":"C:\\N\\M31_L_001.fits"}', 'g1', 100, 'Clouds'),
                (2, 1, 10, 1759300300, 'L', 2, '{"FileName":"M31_L_002.fits"}', 'g2', 100, 'Stars'),
                (3, 1, 10, 1759300600, 'L', 1, '{"FileName":"M31_L_003.fits"}', 'g3', 100, NULL);
             INSERT INTO imagedata (tag, imagedata, acquiredimageid) VALUES ('thumb', X'0102', 1);"#,
        )
        .unwrap();
        frame_derivatives::ensure_schema(&conn).unwrap();
        frame_derivatives::record_pairing(
            &conn,
            &DerivativeRecord {
                derivative_uuid: "d1".into(),
                acquired_image_guid: "g1".into(),
                kind: FrameKind::Calibrated,
                steps: ProcessingSteps::CALIBRATED,
                primary_source: false,
                file_name: "M31_L_001_c.xisf".into(),
                source_tail: None,
                size: Some(500),
                mtime: None,
                width: None,
                height: None,
                producer: Producer::Pixinsight,
                evidence: KindEvidence::Name,
                created_at: 1,
                updated_at: 1,
            },
        )
        .unwrap();
        // Rejected a month before NOW.
        crate::db::record_rejection_times_at(&conn, NOW - 30 * DAY).unwrap();
        Catalog {
            _dir: dir,
            root,
            conn,
        }
    }

    fn options(protected: &BTreeSet<String>, min_age_days: u32) -> PlanOptions<'_> {
        PlanOptions {
            scope: Scope::default(),
            min_age_days,
            now: NOW,
            protected_guids: protected,
        }
    }

    /// A stop lands between frames: a preview stopped changes nothing; a
    /// removal stopped after its first frame keeps that frame removed, leaves
    /// the second in the catalog, and says it stopped; progress names each
    /// stage as it goes.
    #[test]
    fn a_stopped_removal_keeps_the_frames_it_finished() {
        let c = catalog();
        let none = BTreeSet::new();
        let always = || true;
        let quiet = |_: Stage, _: usize, _: usize| {};
        let stopped = plan_with(
            &c.conn,
            &c.images(),
            &options(&none, 7),
            &Work {
                progress: &quiet,
                stop: &always,
            },
        )
        .unwrap_err();
        assert!(stopped.downcast_ref::<Stopped>().is_some(), "{stopped:#}");

        let plan = plan(&c.conn, &c.images(), &options(&none, 7)).unwrap();
        assert_eq!(plan.frames.len(), 2);
        let stages = std::sync::Mutex::new(Vec::new());
        let progress = |stage: Stage, done: usize, total: usize| {
            stages.lock().unwrap().push((stage, done, total));
        };
        // Stop once the first frame has started moving: the check before the
        // second frame sees it.
        let moving = || {
            stages
                .lock()
                .unwrap()
                .iter()
                .any(|(stage, ..)| *stage == Stage::Moving)
        };
        let report = apply_with(
            &c.conn,
            &c.images(),
            &options(&none, 7),
            &plan.digest,
            14,
            &Work {
                progress: &progress,
                stop: &moving,
            },
        )
        .unwrap();
        assert!(report.stopped, "{report:?}");
        assert_eq!(report.removed.len(), 1, "{report:?}");
        assert_eq!(report.removed[0].guid, "g1");
        assert_eq!(
            c.count("SELECT COUNT(*) FROM acquiredimage WHERE guid = 'g2'"),
            1
        );
        assert!(c
            .root
            .join("images/M31/2026-10-01/LIGHT/M31_L_002.fits")
            .is_file());
        let seen: Vec<Stage> = stages
            .lock()
            .unwrap()
            .iter()
            .map(|(stage, ..)| *stage)
            .collect();
        for stage in [Stage::Indexing, Stage::Checking, Stage::Moving] {
            assert!(seen.contains(&stage), "{stage:?} in {seen:?}");
        }

        // The frame that went comes back, and a stop before it leaves it out.
        let restored = restore_with(
            &c.conn,
            &RestoreSelection {
                batch_id: Some(report.batch_id.clone()),
                guids: Vec::new(),
            },
            &Work {
                progress: &quiet,
                stop: &always,
            },
        )
        .unwrap();
        assert!(
            restored.stopped && restored.restored.is_empty(),
            "{restored:?}"
        );
        let restored = restore(
            &c.conn,
            &RestoreSelection {
                batch_id: Some(report.batch_id),
                guids: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(restored.restored.len(), 1, "{restored:?}");
    }

    #[test]
    fn restore_refuses_frames_whose_project_was_deleted() {
        let c = catalog();
        let none = BTreeSet::new();
        let plan = plan(&c.conn, &c.images(), &options(&none, 7)).unwrap();
        let report = apply(&c.conn, &c.images(), &options(&none, 7), &plan.digest, 14).unwrap();
        assert_eq!(report.removed.len(), 2);
        let project = crate::commands::project_removal::plan(&c.conn, 1, &none)
            .unwrap()
            .unwrap();
        crate::commands::project_removal::remove(&c.conn, 1, &project.digest, &none, NOW).unwrap();
        // A new project takes the deleted one's Id.
        c.conn
            .execute(
                "INSERT INTO project (Id, profileId, name, guid) VALUES (1, 'p', 'New', 'new')",
                [],
            )
            .unwrap();
        let restored = restore(
            &c.conn,
            &RestoreSelection {
                batch_id: Some(report.batch_id.clone()),
                guids: vec![],
            },
        )
        .unwrap();
        assert!(restored.restored.is_empty(), "{restored:?}");
        assert_eq!(restored.failed.len(), 2);
        assert!(restored
            .failed
            .iter()
            .all(|frame| frame.reason == "its project is no longer in the catalog"));
        assert_eq!(c.count("SELECT COUNT(*) FROM acquiredimage"), 0);
    }

    #[test]
    fn removal_trashes_the_files_keeps_a_tombstone_and_restore_puts_it_all_back() {
        let c = catalog();
        let none = BTreeSet::new();
        let plan = plan(&c.conn, &c.images(), &options(&none, 7)).unwrap();
        assert_eq!(plan.frames.len(), 2, "{plan:?}");
        let first = &plan.frames[0];
        assert_eq!(first.guid, "g1");
        let kinds: Vec<FileKind> = first.files.iter().map(|file| file.kind).collect();
        assert_eq!(kinds, [FileKind::Light, FileKind::Copy]);
        assert_eq!(plan.bytes, 2500);

        let report = apply(&c.conn, &c.images(), &options(&none, 7), &plan.digest, 14).unwrap();
        assert_eq!(report.removed.len(), 2, "{report:?}");
        assert_eq!(report.files_moved, 3);
        assert!(report.failed.is_empty());
        // Rows and side rows gone; tombstones kept; the files in the trash.
        assert_eq!(c.count("SELECT COUNT(*) FROM acquiredimage"), 1);
        assert_eq!(c.count("SELECT COUNT(*) FROM imagedata"), 0);
        assert_eq!(
            c.count("SELECT COUNT(*) FROM psf_guard_frame_derivative"),
            0
        );
        assert_eq!(c.count("SELECT COUNT(*) FROM psf_guard_removed_image"), 2);
        let light = c.root.join("images/M31/2026-10-01/LIGHT/M31_L_001.fits");
        assert!(!light.exists());
        let trashed = c
            .root
            .join("images")
            .join(TRASH_DIR)
            .join(&report.batch_id)
            .join("M31/2026-10-01/LIGHT/M31_L_001.fits");
        assert!(trashed.is_file());
        assert_eq!(
            removed_guids(&c.conn).unwrap(),
            HashSet::from(["g1".to_string(), "g2".to_string()])
        );
        assert!(removed_file_names(&c.conn)
            .unwrap()
            .contains("m31_l_001.fits"));
        // The accepted frame's count is untouched.
        assert_eq!(
            c.count("SELECT accepted FROM exposureplan WHERE Id = 100"),
            1
        );

        // The trash is not emptied before its retention ends.
        assert_eq!(empty_trash(&c.conn, NOW + DAY).unwrap().frames, 0);

        let restored = restore(
            &c.conn,
            &RestoreSelection {
                batch_id: Some(report.batch_id.clone()),
                guids: vec![],
            },
        )
        .unwrap();
        assert_eq!(restored.restored.len(), 2, "{restored:?}");
        assert!(light.is_file());
        assert!(c
            .root
            .join("images/M31/_Process/calibrated/M31_L_001_c.xisf")
            .is_file());
        assert_eq!(
            c.count("SELECT COUNT(*) FROM acquiredimage WHERE Id IN (1, 2) AND gradingStatus = 2"),
            2
        );
        assert_eq!(
            c.count(
                "SELECT COUNT(*) FROM imagedata WHERE acquiredimageid = 1 AND imagedata = X'0102'"
            ),
            1
        );
        assert_eq!(
            c.count("SELECT COUNT(*) FROM psf_guard_frame_derivative WHERE derivative_uuid = 'd1'"),
            1
        );
        assert_eq!(c.count("SELECT COUNT(*) FROM psf_guard_removed_image"), 0);
        // Its rejection date came back too, so it qualifies at once again.
        assert_eq!(
            c.count(
                "SELECT rejected_at FROM psf_guard_rejected_at WHERE acquired_image_guid = 'g1'"
            ),
            NOW - 30 * DAY
        );
    }

    #[test]
    fn a_restored_reject_returns_to_its_targets_current_project() {
        let c = catalog();
        let none = BTreeSet::new();
        let plan = plan(&c.conn, &c.images(), &options(&none, 7)).unwrap();
        let report = apply(&c.conn, &c.images(), &options(&none, 7), &plan.digest, 14).unwrap();
        assert_eq!(report.removed.len(), 2, "{report:?}");
        // The target moves to another project while its rejects are removed.
        c.conn
            .execute_batch(
                "INSERT INTO project (Id, profileId, name, guid) VALUES (2, 'p', 'Andromeda', 'pg2');
                 UPDATE target SET projectId = 2 WHERE Id = 10;
                 UPDATE acquiredimage SET projectId = 2 WHERE targetId = 10;",
            )
            .unwrap();
        let restored = restore(
            &c.conn,
            &RestoreSelection {
                batch_id: Some(report.batch_id),
                guids: vec![],
            },
        )
        .unwrap();
        assert_eq!(restored.restored.len(), 2, "{restored:?}");
        assert_eq!(
            c.count("SELECT COUNT(*) FROM acquiredimage WHERE targetId = 10 AND projectId = 2"),
            3
        );
    }

    #[test]
    fn a_restored_reject_returns_to_the_target_its_tombstone_records() {
        let c = catalog();
        let none = BTreeSet::new();
        let plan = plan(&c.conn, &c.images(), &options(&none, 7)).unwrap();
        let report = apply(&c.conn, &c.images(), &options(&none, 7), &plan.digest, 14).unwrap();
        assert_eq!(report.removed.len(), 2, "{report:?}");
        // A target merge points the tombstones at the destination target.
        c.conn
            .execute_batch(
                "INSERT INTO project (Id, profileId, name, guid) VALUES (2, 'p', 'Andromeda', 'pg2');
                 INSERT INTO target (Id, name, active, epochcode, projectId, guid) VALUES (11, 'M 31', 1, 0, 2, 'tg2');
                 UPDATE psf_guard_removed_image SET target_id = 11, project_id = 2;",
            )
            .unwrap();
        let restored = restore(
            &c.conn,
            &RestoreSelection {
                batch_id: Some(report.batch_id),
                guids: vec![],
            },
        )
        .unwrap();
        assert_eq!(restored.restored.len(), 2, "{restored:?}");
        assert_eq!(
            c.count("SELECT COUNT(*) FROM acquiredimage WHERE targetId = 11 AND projectId = 2"),
            2
        );
    }

    #[test]
    fn emptying_the_trash_deletes_old_files_and_ends_restore() {
        let c = catalog();
        let none = BTreeSet::new();
        let plan = plan(&c.conn, &c.images(), &options(&none, 7)).unwrap();
        let report = apply(&c.conn, &c.images(), &options(&none, 7), &plan.digest, 14).unwrap();
        let emptied = empty_trash(&c.conn, NOW + 15 * DAY).unwrap();
        assert_eq!(emptied.frames, 2);
        assert_eq!(emptied.files_deleted, 3);
        assert_eq!(emptied.bytes, 2500);
        assert!(!c
            .root
            .join("images")
            .join(TRASH_DIR)
            .join(&report.batch_id)
            .exists());
        let batches = batches(&c.conn).unwrap();
        assert_eq!(batches[0].files_deleted, 2);
        let restored = restore(
            &c.conn,
            &RestoreSelection {
                batch_id: Some(report.batch_id),
                guids: vec![],
            },
        )
        .unwrap();
        assert!(restored.restored.is_empty());
        assert_eq!(restored.failed.len(), 2);
        assert!(restored.failed[0].reason.contains("trash was emptied"));
    }

    #[test]
    fn purging_keeps_a_marker_that_still_keeps_the_frame_out() {
        let c = catalog();
        let none = BTreeSet::new();
        let plan = plan(&c.conn, &c.images(), &options(&none, 7)).unwrap();
        let report = apply(&c.conn, &c.images(), &options(&none, 7), &plan.digest, 14).unwrap();
        // Files still in the trash: nothing to purge yet.
        assert_eq!(purge(&c.conn, None, NOW).unwrap().frames, 0);
        empty_trash(&c.conn, NOW + 15 * DAY).unwrap();
        let purged = purge(&c.conn, Some(&report.batch_id), NOW + 16 * DAY).unwrap();
        assert_eq!(purged.frames, 2);
        assert!(purged.bytes > 0);
        assert_eq!(c.count("SELECT COUNT(*) FROM psf_guard_removed_image WHERE rows_json = '{}' AND files_json = '[]'"), 2);
        assert_eq!(batches(&c.conn).unwrap()[0].purged, 2);
        // Still kept out of pulls and imports, and counted by target.
        assert!(removed_guids(&c.conn).unwrap().contains("g1"));
        assert!(removed_file_names(&c.conn)
            .unwrap()
            .contains("m31_l_001.fits"));
        assert_eq!(
            c.count("SELECT COUNT(*) FROM psf_guard_removed_image WHERE target_id = 10"),
            2
        );
        let restored = restore(
            &c.conn,
            &RestoreSelection {
                batch_id: Some(report.batch_id),
                guids: vec![],
            },
        )
        .unwrap();
        assert!(restored.restored.is_empty());
        assert!(restored.failed[0].reason.contains("purged"));
    }

    #[test]
    fn a_table_from_before_purging_gains_its_column() {
        let c = catalog();
        c.conn
            .execute_batch(&SCHEMA.replace("    purged_at INTEGER,\n", ""))
            .unwrap();
        assert!(!has_column(&c.conn, "psf_guard_removed_image", "purged_at"));
        assert!(batches(&c.conn).unwrap().is_empty());
        ensure_schema(&c.conn).unwrap();
        assert!(has_column(&c.conn, "psf_guard_removed_image", "purged_at"));
    }

    #[test]
    fn a_reject_stamped_after_the_preview_began_qualifies_with_no_grace_period() {
        let c = catalog();
        // Recorded a second after the preview read its clock.
        c.conn
            .execute(
                "UPDATE psf_guard_rejected_at SET rejected_at = ?1 WHERE acquired_image_guid = 'g2'",
                [NOW + 1],
            )
            .unwrap();
        let protected = BTreeSet::new();
        let preview = plan(&c.conn, &c.images(), &options(&protected, 0)).unwrap();
        assert_eq!(preview.waiting, 0);
        assert!(preview.frames.iter().any(|frame| frame.guid == "g2"));

        // With a grace period it waits from its own stamp, not from now.
        let waiting = plan(&c.conn, &c.images(), &options(&protected, 7)).unwrap();
        assert_eq!(waiting.next_eligible_at, Some(NOW + 1 + 7 * DAY));
    }

    #[test]
    fn a_new_reject_is_dated_by_the_preview_clock() {
        let c = catalog();
        // Frame 2 has no date yet: the preview records one.
        c.conn
            .execute(
                "DELETE FROM psf_guard_rejected_at WHERE acquired_image_guid = 'g2'",
                [],
            )
            .unwrap();
        let protected = BTreeSet::new();
        let waiting = plan(&c.conn, &c.images(), &options(&protected, 7)).unwrap();
        let stamped: i64 = c
            .conn
            .query_row(
                "SELECT rejected_at FROM psf_guard_rejected_at WHERE acquired_image_guid = 'g2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stamped, NOW);
        assert_eq!(waiting.next_eligible_at, Some(NOW + 7 * DAY));
    }

    #[test]
    fn recent_rejects_wait_and_reported_or_shared_frames_stay() {
        let c = catalog();
        // Frame 2 was rejected yesterday; frame 1 a collaboration report names.
        c.conn
            .execute(
                "UPDATE psf_guard_rejected_at SET rejected_at = ?1 WHERE acquired_image_guid = 'g2'",
                [NOW - DAY],
            )
            .unwrap();
        let protected = BTreeSet::from(["g1".to_string()]);
        let waiting = plan(&c.conn, &c.images(), &options(&protected, 7)).unwrap();
        assert!(waiting.frames.is_empty());
        assert_eq!(waiting.waiting, 1);
        assert_eq!(waiting.next_eligible_at, Some(NOW + 6 * DAY));
        assert_eq!(waiting.skipped.len(), 1);
        assert!(waiting.skipped[0].reason.contains("collaboration"));
        // With no grace period it qualifies.
        assert_eq!(
            plan(&c.conn, &c.images(), &options(&protected, 0))
                .unwrap()
                .frames
                .len(),
            1
        );

        // A file another row also names stays with that row.
        c.conn
            .execute(
                r#"INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid)
                   VALUES (4, 1, 10, 1759300900, 'L', 0, '{"FileName":"M31_L_002.fits"}', 'g4')"#,
                [],
            )
            .unwrap();
        let shared = plan(&c.conn, &c.images(), &options(&BTreeSet::new(), 0)).unwrap();
        assert_eq!(shared.frames.len(), 1);
        assert!(shared.skipped[0].reason.contains("also backs frame 4"));
    }

    #[test]
    fn a_pull_from_a_copy_that_still_has_them_leaves_removed_frames_out() {
        let c = catalog();
        // The rig's copy keeps every frame.
        let rig = c.root.join("rig.sqlite");
        std::fs::copy(c.root.join("catalog.sqlite"), &rig).unwrap();
        let none = BTreeSet::new();
        let plan = plan(&c.conn, &c.images(), &options(&none, 7)).unwrap();
        apply(&c.conn, &c.images(), &options(&none, 7), &plan.digest, 14).unwrap();
        let source = Connection::open(&rig).unwrap();
        let summary = crate::commands::sync::sync_pull(
            &source,
            &c.conn,
            &crate::commands::sync::PullOptions {
                dry_run: false,
                with_image_data: true,
                project_filter: None,
            },
        )
        .unwrap();
        assert_eq!(c.count("SELECT COUNT(*) FROM acquiredimage"), 1);
        assert_eq!(c.count("SELECT COUNT(*) FROM imagedata"), 0);
        assert!(
            summary
                .changes
                .iter()
                .any(|change| change.contains("g1 (removed here)")),
            "{:?}",
            summary.changes
        );
    }

    #[test]
    fn a_removed_frames_caches_go_and_the_next_frames_stay() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path();
        let files = [
            "previews/12_1_10_1759300000_M31_L_001_fits_screen_stretch_1_2_abc.png",
            "previews/123_1_10_1759300000_M31_L_002_fits_screen_stretch_1_2_abc.png",
            "annotated/annotated_v4_12_1_10_1759300000_M31_screen_200_abc.png",
            "stars/stars_v4_12_1_10_1759300000_M31_abc_tok.json",
            "psf_multi/psf_multi_v2_12_1_10_1_M31_9_moffat_hfr_center_0_abc_tok.png",
            "stats/stats_v2_12_1_10_1759300000_abc_tok.json",
            "stats/stats_v2_12_2_10_1759300000_abc_tok.json",
            "astrometry/12.json",
            "astrometry/123.json",
            "satellites/12.json",
        ];
        for file in files {
            let path = cache.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
        let gone = forget_image_caches(
            cache,
            &[RemovedFrame {
                image_id: 12,
                guid: "g".into(),
                project_id: Some(1),
                target_id: Some(10),
            }],
        );
        assert_eq!(gone, 7);
        // Another frame's files stay, even one whose Id starts the same.
        for kept in [
            "previews/123_1_10_1759300000_M31_L_002_fits_screen_stretch_1_2_abc.png",
            "stats/stats_v2_12_2_10_1759300000_abc_tok.json",
            "astrometry/123.json",
        ] {
            assert!(cache.join(kept).exists(), "{kept}");
        }
    }

    #[test]
    fn apply_refuses_a_stale_preview() {
        let c = catalog();
        let none = BTreeSet::new();
        let plan = plan(&c.conn, &c.images(), &options(&none, 7)).unwrap();
        c.conn
            .execute(
                "UPDATE acquiredimage SET gradingStatus = 1 WHERE Id = 2",
                [],
            )
            .unwrap();
        let error = apply(&c.conn, &c.images(), &options(&none, 7), &plan.digest, 14).unwrap_err();
        assert!(
            error.downcast_ref::<StaleRemovalPlan>().is_some(),
            "{error:#}"
        );
        assert_eq!(c.count("SELECT COUNT(*) FROM acquiredimage"), 3);
    }

    #[test]
    fn imports_and_scans_skip_removed_frames_and_the_trash() {
        let c = catalog();
        let none = BTreeSet::new();
        let plan = plan(&c.conn, &c.images(), &options(&none, 7)).unwrap();
        let report = apply(&c.conn, &c.images(), &options(&none, 7), &plan.digest, 14).unwrap();
        let images = c.root.join("images");
        let found =
            crate::commands::import::collect_fits_files(std::slice::from_ref(&images)).unwrap();
        assert!(
            found
                .iter()
                .all(|path| !path.starts_with(images.join(TRASH_DIR))),
            "{found:?}"
        );
        // The same file arriving again is known, not a new frame.
        let again = images.join("M31/2026-10-01/LIGHT/M31_L_001.fits");
        std::fs::write(&again, vec![7u8; 1000]).unwrap();
        let known =
            crate::commands::import::known_files(&c.conn, std::slice::from_ref(&again)).unwrap();
        assert!(known.contains(&again));
        let tree = DirectoryTree::build_multiple(&[images.as_path()]).unwrap();
        assert!(tree.find_file("M31_L_002.fits").is_none());
        assert!(!report.batch_id.is_empty());
    }
}
