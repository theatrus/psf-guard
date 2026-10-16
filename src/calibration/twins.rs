//! One calibration exposure catalogued more than once.
//!
//! Calibration rows are keyed by path, so every copy of a frame the scan
//! meets can become a row: a filer copies frames out of a drop folder and
//! deletes the originals a day later, a person keeps a second copy under
//! another folder the database scans, or an older build recorded both sides
//! of a move. Once the original goes, its row reads as a missing file for
//! good, and a master built while both copies were there averaged one
//! exposure twice.
//!
//! [`same_exposure`] decides whether two copies hold one exposure. The
//! import asks it before cataloguing a file ([`recorded_copy`]), and the
//! repair asks it to find twins already recorded ([`plan_repair`]). A repair
//! records each merge, and a sync carries those records, so a pull from a
//! catalog that still holds the twins does not bring them back
//! ([`sync_merges`], [`fold_merged`]). Nothing here moves or deletes a file.

use super::{canonical_text, hex_digest, CalibrationKind};
use crate::commands::import::headers::FrameMeta;
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::UNIX_EPOCH;

/// Copies of a frame that the catalog knows are not its recorded file. A
/// scan skips an unchanged copy here without reading its header, as long as
/// the recorded file is still there; once it is gone, the next scan
/// re-points the row to the copy. Paths are this host's, so the table is
/// never synced.
///
/// `psf_guard_calibration_merged` records each row a repair merged away and
/// the row it went into. It is synced, by frame uuid, so a pull never
/// re-inserts a merged row and folds one the destination still holds.
///
/// The capture index keeps the import's twin lookup from growing with the
/// library.
pub(crate) const TWIN_TABLE_SQL: &str = r#"
    CREATE TABLE IF NOT EXISTS psf_guard_calibration_twin (
        source_path        TEXT PRIMARY KEY,
        frame_uuid         TEXT NOT NULL,
        source_fingerprint TEXT NOT NULL,
        updated_at         INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_psf_guard_calibration_twin_frame
        ON psf_guard_calibration_twin(frame_uuid);
    CREATE TABLE IF NOT EXISTS psf_guard_calibration_merged (
        merged_uuid TEXT PRIMARY KEY,
        keep_uuid   TEXT NOT NULL,
        merged_at   INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_psf_guard_calibration_merged_keep
        ON psf_guard_calibration_merged(keep_uuid);
    CREATE INDEX IF NOT EXISTS idx_psf_guard_calibration_frame_capture
        ON psf_guard_calibration_frame(rig_uuid, kind, captured_at);
"#;

const TWIN_TABLE: &str = "psf_guard_calibration_twin";
const MERGED_TABLE: &str = "psf_guard_calibration_merged";

/// Columns a kept row takes from the twin whose file it moves to: the
/// file's header and what was measured from it. Its uuid, rig, kind,
/// validity mark and dates stay its own.
const FILE_COLUMNS: &[&str] = &[
    "source_fingerprint",
    "captured_at",
    "image_type_raw",
    "telescope",
    "camera",
    "width",
    "height",
    "channels",
    "binning_x",
    "binning_y",
    "gain",
    "offset",
    "readout_mode",
    "readout_mode_name",
    "is_master",
    "bayer_pattern",
    "bayer_x_offset",
    "bayer_y_offset",
    "exposure_s",
    "camera_temp",
    "filter_name",
    "focal_length_mm",
    "rotation",
    "dark_level",
    "dark_level_sigma",
    "stray_light",
    "file_size",
    "file_mtime_ns",
];

/// Columns of [`HeaderSettings`], in its order.
const SETTINGS_COLUMNS: &str =
    "exposure_s, gain, offset, camera_temp, filter_name, binning_x, binning_y, width, height";

/// Bytes read from each end of a file for the content check: the whole
/// header of a N.I.N.A. frame and the first rows of its pixels, and the
/// last rows.
const SAMPLE_BYTES: u64 = 32 * 1024;

/// Content checks remembered across previews, keyed by file, size and
/// modification time. Past this many the memory is cleared.
const MAX_REMEMBERED_SAMPLES: usize = 50_000;

/// What two copies of one exposure share in the catalog: the kind, the
/// capture time to the second from the header, the file name (case aside)
/// and the rig. The file size and the bytes themselves are checked by
/// [`same_exposure`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ExposureKey {
    kind: CalibrationKind,
    captured_at: Option<i64>,
    file_name: String,
    rig_uuid: String,
}

impl ExposureKey {
    pub(crate) fn new(
        kind: CalibrationKind,
        captured_at: Option<i64>,
        path: &Path,
        rig_uuid: &str,
    ) -> Option<Self> {
        let file_name = path.file_name()?.to_str()?.to_lowercase();
        Some(Self {
            kind,
            captured_at,
            file_name,
            rig_uuid: rig_uuid.to_string(),
        })
    }
}

/// The capture settings a copy's header records. Two copies of one
/// exposure carry the same header, so they agree on every one of these; a
/// side that records a setting the other does not is a different frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct HeaderSettings {
    exposure_s: Option<f64>,
    gain: Option<i64>,
    offset: Option<i64>,
    camera_temp: Option<f64>,
    filter: Option<String>,
    binning_x: Option<i64>,
    binning_y: Option<i64>,
    width: Option<i64>,
    height: Option<i64>,
}

impl HeaderSettings {
    pub(crate) fn of_frame(frame: &FrameMeta) -> Self {
        Self {
            exposure_s: frame.exposure_s,
            gain: frame.gain,
            offset: frame.offset,
            camera_temp: frame.camera_temp,
            filter: frame.filter.clone(),
            binning_x: frame.binning_x,
            binning_y: frame.binning_y,
            width: frame.width,
            height: frame.height,
        }
    }

    /// Read from a row whose [`SETTINGS_COLUMNS`] start at `start`.
    fn read(row: &rusqlite::Row<'_>, start: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            exposure_s: row.get(start)?,
            gain: row.get(start + 1)?,
            offset: row.get(start + 2)?,
            camera_temp: row.get(start + 3)?,
            filter: row.get(start + 4)?,
            binning_x: row.get(start + 5)?,
            binning_y: row.get(start + 6)?,
            width: row.get(start + 7)?,
            height: row.get(start + 8)?,
        })
    }

    /// Exposure as a master would match it, temperature within the master
    /// tolerance, and the rest exactly.
    fn agrees(&self, other: &Self) -> bool {
        fn both(left: Option<f64>, right: Option<f64>, same: impl Fn(f64, f64) -> bool) -> bool {
            match (left, right) {
                (None, None) => true,
                (Some(left), Some(right)) => same(left, right),
                _ => false,
            }
        }
        let filter = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_lowercase)
        };
        both(self.exposure_s, other.exposure_s, |left, right| {
            super::exposure_matches(Some(left), Some(right))
        }) && both(self.camera_temp, other.camera_temp, |left, right| {
            super::temperature_matches(Some(left), Some(right), super::rules::defaults())
        }) && self.gain == other.gain
            && self.offset == other.offset
            && filter(&self.filter) == filter(&other.filter)
            && self.binning_x == other.binning_x
            && self.binning_y == other.binning_y
            && self.width == other.width
            && self.height == other.height
    }
}

/// One copy: a recorded row or a scanned file.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ExposureCopy<'a> {
    pub key: &'a ExposureKey,
    pub settings: &'a HeaderSettings,
    pub path: &'a Path,
    /// The size the catalog recorded, for a copy whose file is gone.
    pub recorded_size: Option<i64>,
}

/// Where a recorded path is read on this machine, or `None` for nowhere.
type Resolve = dyn Fn(&Path) -> Option<PathBuf> + Send + Sync;

/// How a recorded path is read on this machine. The server reads only
/// inside a database's folders, as the calibration browser does; a dry run
/// against a copied catalog can map the recording host's mount onto this
/// one's.
#[derive(Clone)]
pub struct FileAccess {
    resolve: Arc<Resolve>,
    /// What [`check_files`] found, by recorded path: read from here
    /// rather than from disk again.
    known: Option<Arc<HashMap<PathBuf, KnownFile>>>,
}

/// One recorded path as [`check_files`] found it.
#[derive(Debug, Clone, Default)]
struct KnownFile {
    /// Its size, when it is a file this machine reads.
    size: Option<u64>,
    /// Its content sample, when one was taken: `Some(None)` for a file
    /// that could not be sampled.
    sample: Option<Option<String>>,
}

impl std::fmt::Debug for FileAccess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("FileAccess")
    }
}

impl FileAccess {
    /// Every path as it is written.
    pub fn disk() -> Self {
        Self::resolving(|path| Some(path.to_path_buf()))
    }

    /// A path read through `resolve`; `None` reads as a missing file.
    pub fn resolving(resolve: impl Fn(&Path) -> Option<PathBuf> + Send + Sync + 'static) -> Self {
        Self {
            resolve: Arc::new(resolve),
            known: None,
        }
    }

    /// Paths under `from` read under `to` instead; others as written.
    pub fn remapped(from: PathBuf, to: PathBuf) -> Self {
        Self::resolving(move |path| {
            Some(match path.strip_prefix(&from) {
                Ok(rest) => to.join(rest),
                Err(_) => path.to_path_buf(),
            })
        })
    }

    fn local(&self, path: &Path) -> Option<PathBuf> {
        (self.resolve)(path).filter(|local| local.is_file())
    }

    fn known(&self, path: &Path) -> Option<&KnownFile> {
        self.known.as_ref()?.get(path)
    }

    pub fn exists(&self, path: &Path) -> bool {
        match self.known(path) {
            Some(known) => known.size.is_some(),
            None => self.local(path).is_some(),
        }
    }

    fn size(&self, path: &Path) -> Option<u64> {
        if let Some(known) = self.known(path) {
            return known.size;
        }
        self.stat(path)
    }

    /// The size of the file `path` reads as here, with one metadata call.
    fn stat(&self, path: &Path) -> Option<u64> {
        let local = (self.resolve)(path)?;
        std::fs::metadata(local)
            .ok()
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.len())
    }

    fn sample(&self, path: &Path) -> Option<String> {
        if let Some(sample) = self.known(path).and_then(|known| known.sample.clone()) {
            return sample;
        }
        content_sample(&self.local(path)?)
    }
}

/// A file as it was sampled: its path, size and modification time.
type SampledFile = (PathBuf, u64, u128);

static SAMPLES: LazyLock<Mutex<HashMap<SampledFile, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A digest of a file's size, its first [`SAMPLE_BYTES`] (the header and
/// the first pixels) and its last. Two exposures never share sensor noise
/// at both ends, and a byte copy always does, so this tells a copy from a
/// different frame that reuses the name without reading the whole file.
fn content_sample(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    let size = metadata.len();
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let key = (path.to_path_buf(), size, modified);
    if let Some(known) = SAMPLES.lock().ok()?.get(&key) {
        return Some(known.clone());
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(size.to_le_bytes());
    let head_len = SAMPLE_BYTES.min(size);
    let mut head = vec![0_u8; head_len as usize];
    file.read_exact(&mut head).ok()?;
    hasher.update(&head);
    if size > head_len {
        let tail_start = size.saturating_sub(SAMPLE_BYTES).max(head_len);
        let mut tail = vec![0_u8; (size - tail_start) as usize];
        file.seek(SeekFrom::Start(tail_start)).ok()?;
        file.read_exact(&mut tail).ok()?;
        hasher.update(&tail);
    }
    let digest = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if let Ok(mut samples) = SAMPLES.lock() {
        if samples.len() >= MAX_REMEMBERED_SAMPLES {
            samples.clear();
        }
        samples.insert(key, digest.clone());
    }
    Some(digest)
}

/// Whether two copies hold one exposure.
///
/// They must agree on [`ExposureKey`] — kind, capture time, file name and
/// rig — on the capture settings their headers record ([`HeaderSettings`]),
/// and on file size, read from disk or, for a file that is gone, from the
/// catalog. When both files are there, their headers and the ends of their
/// pixels must also be byte for byte the same, so a different frame that
/// happens to share a name and second is never taken for a copy. When one
/// is gone there is nothing left to compare, and the catalog's record
/// decides — but only for a frame whose header dates it: without a capture
/// time, a name and settings are too little to call a missing file the
/// same exposure.
pub(crate) fn same_exposure(
    left: &ExposureCopy<'_>,
    right: &ExposureCopy<'_>,
    files: &FileAccess,
) -> bool {
    if left.key != right.key || !left.settings.agrees(right.settings) {
        return false;
    }
    let left_live = files.size(left.path);
    let right_live = files.size(right.path);
    let size = |live: Option<u64>, recorded: Option<i64>| {
        live.and_then(|size| i64::try_from(size).ok())
            .or(recorded.filter(|size| *size > 0))
    };
    if let (Some(left_size), Some(right_size)) = (
        size(left_live, left.recorded_size),
        size(right_live, right.recorded_size),
    ) && left_size != right_size
    {
        return false;
    }
    match (left_live, right_live) {
        (Some(_), Some(_)) => matches!(
            (files.sample(left.path), files.sample(right.path)),
            (Some(left), Some(right)) if left == right
        ),
        _ => left.key.captured_at.is_some(),
    }
}

/// A recorded row that a scanned file is a copy of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordedCopy {
    /// The row's own file is still there: the scanned file is a second copy.
    Live { frame_uuid: String },
    /// The row's file is gone: the frame has moved to the scanned file.
    Moved { id: i64 },
}

/// The recorded row whose exposure `path` holds, if any. A row whose file
/// is still there wins over one whose file is gone; among each, the oldest.
pub(crate) fn recorded_copy(
    conn: &Connection,
    key: &ExposureKey,
    settings: &HeaderSettings,
    path: &Path,
    files: &FileAccess,
) -> Result<Option<RecordedCopy>> {
    let mut statement = conn.prepare(&format!(
        "SELECT id, frame_uuid, source_path, file_size, {SETTINGS_COLUMNS}
         FROM psf_guard_calibration_frame
         WHERE rig_uuid = ?3 AND kind = ?1 AND captured_at IS ?2 AND source_path <> ?4
         ORDER BY id"
    ))?;
    let rows = statement
        .query_map(
            params![
                key.kind.as_str(),
                key.captured_at,
                key.rig_uuid,
                canonical_text(path)
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    HeaderSettings::read(row, 4)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let scanned = ExposureCopy {
        key,
        settings,
        path,
        recorded_size: None,
    };
    let mut moved = None;
    for (id, frame_uuid, recorded, recorded_size, recorded_settings) in rows {
        let recorded = PathBuf::from(recorded);
        let Some(recorded_key) =
            ExposureKey::new(key.kind, key.captured_at, &recorded, &key.rig_uuid)
        else {
            continue;
        };
        if recorded_key != *key {
            continue;
        }
        let copy = ExposureCopy {
            key: &recorded_key,
            settings: &recorded_settings,
            path: &recorded,
            recorded_size,
        };
        if !same_exposure(&scanned, &copy, files) {
            continue;
        }
        if files.exists(&recorded) {
            return Ok(Some(RecordedCopy::Live { frame_uuid }));
        }
        moved.get_or_insert(id);
    }
    Ok(moved.map(|id| RecordedCopy::Moved { id }))
}

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

fn twin_table_exists(conn: &Connection) -> bool {
    table_exists(conn, TWIN_TABLE)
}

/// A copy recorded as a twin: its fingerprint when recorded, and the path of
/// the row it belongs to.
pub(crate) struct RecordedTwin {
    pub source_fingerprint: String,
    pub row_path: String,
}

pub(crate) fn recorded_twin(conn: &Connection, source_path: &str) -> Result<Option<RecordedTwin>> {
    if !twin_table_exists(conn) {
        return Ok(None);
    }
    Ok(conn
        .query_row(
            "SELECT t.source_fingerprint, f.source_path
             FROM psf_guard_calibration_twin t
             JOIN psf_guard_calibration_frame f ON f.frame_uuid = t.frame_uuid
             WHERE t.source_path = ?1",
            [source_path],
            |row| {
                Ok(RecordedTwin {
                    source_fingerprint: row.get(0)?,
                    row_path: row.get(1)?,
                })
            },
        )
        .optional()?)
}

pub(crate) fn record_twin(
    conn: &Connection,
    source_path: &str,
    frame_uuid: &str,
    fingerprint: &str,
    now: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO psf_guard_calibration_twin
            (source_path, frame_uuid, source_fingerprint, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(source_path) DO UPDATE SET
            frame_uuid = excluded.frame_uuid,
            source_fingerprint = excluded.source_fingerprint,
            updated_at = excluded.updated_at",
        params![source_path, frame_uuid, fingerprint, now],
    )?;
    Ok(())
}

pub(crate) fn forget_twin(conn: &Connection, source_path: &str) -> Result<()> {
    if twin_table_exists(conn) {
        conn.execute(
            "DELETE FROM psf_guard_calibration_twin WHERE source_path = ?1",
            [source_path],
        )?;
    }
    Ok(())
}

/// Drop the twins of rows that are going.
pub(crate) fn forget_twins_of(conn: &Connection, frame_uuids: &[String]) -> Result<()> {
    if !twin_table_exists(conn) {
        return Ok(());
    }
    let mut statement =
        conn.prepare("DELETE FROM psf_guard_calibration_twin WHERE frame_uuid = ?1")?;
    for frame_uuid in frame_uuids {
        statement.execute([frame_uuid])?;
    }
    Ok(())
}

/// Recorded twins a scan can skip without a header read: those whose row's
/// own file is still there, by path and fingerprint. One whose row's file
/// has gone is left for the scan to read, so the row moves to it.
pub(crate) fn skippable_twins(conn: &Connection) -> Result<Vec<(String, String)>> {
    if !twin_table_exists(conn) {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT t.source_path, t.source_fingerprint, f.source_path
         FROM psf_guard_calibration_twin t
         JOIN psf_guard_calibration_frame f ON f.frame_uuid = t.frame_uuid",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter(|(_, _, row_path)| Path::new(row_path).is_file())
        .map(|(path, fingerprint, _)| (path, fingerprint))
        .collect())
}

/// Forget recorded twins whose file is gone: nothing is left to skip.
pub(crate) fn prune_twins(conn: &Connection) -> Result<usize> {
    if !twin_table_exists(conn) {
        return Ok(0);
    }
    let paths = {
        let mut statement = conn.prepare("SELECT source_path FROM psf_guard_calibration_twin")?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut pruned = 0;
    for path in paths.iter().filter(|path| !Path::new(path).is_file()) {
        pruned += conn.execute(
            "DELETE FROM psf_guard_calibration_twin WHERE source_path = ?1",
            [path],
        )?;
    }
    Ok(pruned)
}

/// Drop the merge records of rows that are forgotten, on either side: a
/// forgotten row is no longer anything's twin, and nothing is merged into
/// it, so a rescan or a pull may bring the files back.
pub(crate) fn forget_merges_of(conn: &Connection, frame_uuids: &[String]) -> Result<()> {
    if !table_exists(conn, MERGED_TABLE) {
        return Ok(());
    }
    let mut statement = conn.prepare(
        "DELETE FROM psf_guard_calibration_merged WHERE merged_uuid = ?1 OR keep_uuid = ?1",
    )?;
    for frame_uuid in frame_uuids {
        statement.execute([frame_uuid])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Repair
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct RecordedRow {
    id: i64,
    frame_uuid: String,
    rig_uuid: String,
    kind: CalibrationKind,
    source_path: String,
    source_fingerprint: String,
    captured_at: Option<i64>,
    file_size: Option<i64>,
    valid_direction: Option<String>,
    settings: HeaderSettings,
}

#[derive(Debug, Clone)]
struct MasterLink {
    uuid: String,
    kind: String,
    sources: Vec<String>,
    bias: Option<String>,
    dark: Option<String>,
    created_at: i64,
}

/// The rows and masters a repair is planned from, and a digest of the rows
/// that could be duplicates, which says whether they have changed since.
#[derive(Debug, Clone, Default)]
pub struct DuplicateSnapshot {
    rows: Vec<RecordedRow>,
    masters: Vec<MasterLink>,
    rig_names: HashMap<String, String>,
    digest: String,
}

/// Read what a repair needs, quickly: no file is opened.
pub fn duplicate_snapshot(conn: &Connection) -> Result<DuplicateSnapshot> {
    if !super::schema_exists(conn) {
        return Ok(DuplicateSnapshot {
            digest: hex_digest("empty"),
            ..Default::default()
        });
    }
    let has_validity = super::table_column_names(conn, "psf_guard_calibration_frame")?
        .iter()
        .any(|name| name.eq_ignore_ascii_case("valid_direction"));
    let validity = if has_validity {
        "valid_direction"
    } else {
        "NULL"
    };
    let mut statement = conn.prepare(&format!(
        "SELECT id, frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
                captured_at, file_size, {validity}, {SETTINGS_COLUMNS}
         FROM psf_guard_calibration_frame ORDER BY id"
    ))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(3)?,
                RecordedRow {
                    id: row.get(0)?,
                    frame_uuid: row.get(1)?,
                    rig_uuid: row.get(2)?,
                    kind: CalibrationKind::Bias,
                    source_path: row.get(4)?,
                    source_fingerprint: row.get(5)?,
                    captured_at: row.get(6)?,
                    file_size: row.get(7)?,
                    valid_direction: row.get(8)?,
                    settings: HeaderSettings::read(row, 9)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter_map(|(kind, row)| {
            Some(RecordedRow {
                kind: CalibrationKind::from_db(&kind)?,
                ..row
            })
        })
        .collect::<Vec<_>>();
    let masters = master_links(conn)?;
    let mut statement = conn.prepare("SELECT rig_uuid, name FROM psf_guard_rig")?;
    let rig_names = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<HashMap<String, String>>>()?;

    let digest = candidate_digest(&rows);
    Ok(DuplicateSnapshot {
        rows,
        masters,
        rig_names,
        digest,
    })
}

/// The rows that share an [`ExposureKey`] with another row, by key, oldest
/// first: the only rows a repair can plan from.
fn candidates(rows: &[RecordedRow]) -> BTreeMap<ExposureKey, Vec<&RecordedRow>> {
    let mut by_key: BTreeMap<ExposureKey, Vec<&RecordedRow>> = BTreeMap::new();
    for row in rows {
        if let Some(key) = ExposureKey::new(
            row.kind,
            row.captured_at,
            Path::new(&row.source_path),
            &row.rig_uuid,
        ) {
            by_key.entry(key).or_default().push(row);
        }
    }
    by_key.retain(|_, rows| rows.len() > 1);
    for rows in by_key.values_mut() {
        rows.sort_by_key(|row| row.id);
    }
    by_key
}

/// A digest of what a plan reads from the catalog: the rows that share an
/// exposure key with another. Grading, flat measuring, a new master or a
/// new frame of another exposure leave it as it is, so a preview stays
/// good while background work writes to the catalog.
fn candidate_digest(rows: &[RecordedRow]) -> String {
    let mut hasher = Sha256::new();
    for row in candidates(rows).values().flatten() {
        hasher.update(
            format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1e}",
                row.id,
                row.frame_uuid,
                row.rig_uuid,
                row.kind.as_str(),
                row.source_path,
                row.source_fingerprint,
                row.captured_at,
                row.file_size,
                row.valid_direction,
                row.settings,
            )
            .as_bytes(),
        );
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn master_links(conn: &Connection) -> Result<Vec<MasterLink>> {
    let mut statement = conn.prepare(
        "SELECT master_uuid, kind, source_frame_uuids, bias_master_uuid, dark_master_uuid,
                created_at
         FROM psf_guard_calibration_master",
    )?;
    let raw = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raw.into_iter()
        .map(|(uuid, kind, sources, bias, dark, created_at)| {
            Ok(MasterLink {
                uuid,
                kind,
                sources: serde_json::from_str(&sources)
                    .context("reading calibration master source provenance")?,
                bias,
                dark,
                created_at,
            })
        })
        .collect()
}

/// The masters built from any of `frame_uuids`, and every master built on
/// one of those, however deep.
pub(crate) fn masters_citing(
    conn: &Connection,
    frame_uuids: &HashSet<&str>,
) -> Result<HashSet<String>> {
    Ok(citing(&master_links(conn)?, frame_uuids))
}

fn citing(links: &[MasterLink], frame_uuids: &HashSet<&str>) -> HashSet<String> {
    let mut invalid = links
        .iter()
        .filter(|master| {
            master
                .sources
                .iter()
                .any(|source| frame_uuids.contains(source.as_str()))
        })
        .map(|master| master.uuid.clone())
        .collect::<HashSet<_>>();
    loop {
        let before = invalid.len();
        for master in links {
            if master
                .bias
                .iter()
                .chain(master.dark.iter())
                .any(|dependency| invalid.contains(dependency))
            {
                invalid.insert(master.uuid.clone());
            }
        }
        if invalid.len() == before {
            return invalid;
        }
    }
}

/// One row of a twin group.
#[derive(Debug, Clone, Serialize)]
pub struct TwinMember {
    pub frame_uuid: String,
    pub source_path: String,
    /// Whether the row's file is on disk.
    pub exists: bool,
    /// Whether this is the row the repair keeps.
    pub keep: bool,
}

/// Rows that record one exposure, and what the repair makes of them.
#[derive(Debug, Clone, Serialize)]
pub struct TwinGroup {
    pub kind: CalibrationKind,
    pub captured_at: Option<i64>,
    pub file_name: String,
    pub rig_uuid: String,
    pub rig_name: Option<String>,
    /// The row kept: the oldest.
    pub keep: String,
    /// Its file after the repair: its own, or a live twin's when its own is
    /// gone.
    pub path: String,
    /// Whether the kept row moves to a twin's file.
    pub repointed: bool,
    /// Whether more than one of the files is on disk.
    pub physical: bool,
    /// The oldest first.
    pub members: Vec<TwinMember>,
    /// The twin whose file, and header, the kept row takes.
    #[serde(skip)]
    repoint: Option<String>,
    #[serde(skip)]
    twins: Vec<(String, String)>,
    #[serde(skip)]
    validity: Option<String>,
}

/// A generated master the repair removes, so it is built again.
#[derive(Debug, Clone, Serialize)]
pub struct InvalidatedMaster {
    pub master_uuid: String,
    pub kind: String,
    pub source_count: usize,
    pub created_at: i64,
    /// Whether it cites a row the repair removes or moves to another file
    /// itself, rather than being built on a master that does.
    pub direct: bool,
    /// Whether it took more than one copy of an exposure: it averaged that
    /// exposure twice.
    pub doubled: bool,
}

/// What [`plan_repair`] found, and what applying it would do.
#[derive(Debug, Clone, Serialize)]
pub struct DuplicateReport {
    pub groups: Vec<TwinGroup>,
    /// Rows deleted, merged into the row kept.
    pub rows_removed: usize,
    /// Rows whose file is gone that the repair settles: merged into a live
    /// twin, or kept and moved to its file.
    pub missing_rows: usize,
    /// Extra rows whose file is a second copy on disk.
    pub physical_rows: usize,
    /// Kept rows that move to a twin's file.
    pub repointed: usize,
    pub masters: Vec<InvalidatedMaster>,
    /// Hands the plan to [`apply_repair`]. A change to the rows that could
    /// be duplicates, or to which of their files are there, gives another;
    /// a change elsewhere in the catalog does not.
    pub token: String,
    #[serde(skip)]
    snapshot_digest: String,
}

impl DuplicateReport {
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// An empty plan, for a test to fill in.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            groups: Vec::new(),
            rows_removed: 0,
            missing_rows: 0,
            physical_rows: 0,
            repointed: 0,
            masters: Vec::new(),
            token: String::new(),
            snapshot_digest: String::new(),
        }
    }
}

impl TwinGroup {
    /// A group with nothing in it, for a test to fill in.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            kind: CalibrationKind::Dark,
            captured_at: None,
            file_name: String::new(),
            rig_uuid: String::new(),
            rig_name: None,
            keep: String::new(),
            path: String::new(),
            repointed: false,
            physical: false,
            members: Vec::new(),
            repoint: None,
            twins: Vec::new(),
            validity: None,
        }
    }
}

/// Find every exposure recorded more than once.
///
/// Rows that agree on [`ExposureKey`] are compared: live files by size and
/// content, each cluster of identical files one exposure. A row whose file
/// is gone joins the one cluster whose size it recorded; with none, or more
/// than one, it is left alone. A group of rows whose files are all gone is
/// left too: it may be a disk that is not mounted. The oldest row of each
/// group is kept.
pub fn plan_repair(snapshot: &DuplicateSnapshot, files: &FileAccess) -> DuplicateReport {
    let by_key = candidates(&snapshot.rows);
    let mut groups = Vec::new();
    for (key, rows) in &by_key {
        let (live, dead): (Vec<&RecordedRow>, Vec<&RecordedRow>) = rows
            .iter()
            .partition(|row| files.exists(Path::new(&row.source_path)));
        let mut clusters: Vec<Vec<&RecordedRow>> = Vec::new();
        for row in live {
            match clusters.iter_mut().find(|cluster| {
                same_exposure(&row_copy(key, cluster[0]), &row_copy(key, row), files)
            }) {
                Some(cluster) => cluster.push(row),
                None => clusters.push(vec![row]),
            }
        }
        for row in dead {
            let fits = clusters
                .iter()
                .enumerate()
                .filter(|(_, cluster)| {
                    same_exposure(&row_copy(key, cluster[0]), &row_copy(key, row), files)
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if let [only] = fits[..] {
                clusters[only].push(row);
            }
        }
        for mut members in clusters.into_iter().filter(|cluster| cluster.len() > 1) {
            members.sort_by_key(|row| row.id);
            groups.push(twin_group(snapshot, members, files));
        }
    }
    groups.sort_by(|left, right| {
        right
            .captured_at
            .cmp(&left.captured_at)
            .then_with(|| left.file_name.cmp(&right.file_name))
    });

    let removed = groups
        .iter()
        .flat_map(|group| group.members.iter().filter(|member| !member.keep))
        .map(|member| member.frame_uuid.as_str())
        .collect::<HashSet<_>>();
    // A kept row that moves to a twin's file has that file's fingerprint
    // from then on, so a master built from it would never be found again.
    let changed = removed
        .iter()
        .copied()
        .chain(
            groups
                .iter()
                .filter(|group| group.repointed)
                .map(|group| group.keep.as_str()),
        )
        .collect::<HashSet<_>>();
    let invalid = citing(&snapshot.masters, &changed);
    let group_of = groups
        .iter()
        .enumerate()
        .flat_map(|(index, group)| {
            group
                .members
                .iter()
                .map(move |member| (member.frame_uuid.as_str(), index))
        })
        .collect::<HashMap<_, _>>();
    let mut masters = snapshot
        .masters
        .iter()
        .filter(|master| invalid.contains(&master.uuid))
        .map(|master| InvalidatedMaster {
            master_uuid: master.uuid.clone(),
            kind: master.kind.clone(),
            source_count: master.sources.len(),
            created_at: master.created_at,
            direct: master
                .sources
                .iter()
                .any(|source| changed.contains(source.as_str())),
            doubled: {
                let mut seen = HashSet::new();
                master
                    .sources
                    .iter()
                    .filter_map(|source| group_of.get(source.as_str()))
                    .any(|group| !seen.insert(*group))
            },
        })
        .collect::<Vec<_>>();
    masters.sort_by(|left, right| {
        right
            .direct
            .cmp(&left.direct)
            .then_with(|| right.created_at.cmp(&left.created_at))
            .then_with(|| left.master_uuid.cmp(&right.master_uuid))
    });

    let rows_removed = removed.len();
    let missing_rows = groups
        .iter()
        .flat_map(|group| &group.members)
        .filter(|member| !member.exists)
        .count();
    let physical_rows = groups
        .iter()
        .map(|group| {
            group
                .members
                .iter()
                .filter(|member| member.exists)
                .count()
                .saturating_sub(1)
        })
        .sum();
    let repointed = groups.iter().filter(|group| group.repointed).count();

    let mut plan = Sha256::new();
    plan.update(snapshot.digest.as_bytes());
    for group in &groups {
        plan.update(
            format!(
                "G{}\u{1f}{}\u{1f}{:?}",
                group.keep, group.path, group.validity
            )
            .as_bytes(),
        );
        for member in &group.members {
            plan.update(
                format!(
                    "\u{1f}{}:{}:{}",
                    member.frame_uuid, member.exists, member.keep
                )
                .as_bytes(),
            );
        }
        for (path, fingerprint) in &group.twins {
            plan.update(format!("\u{1f}T{path}:{fingerprint}").as_bytes());
        }
    }
    // The masters are not part of the token: a master built since the
    // preview from one of these rows goes too when the repair applies,
    // and one built from other rows is no business of the repair's.
    let token = plan
        .finalize()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();

    DuplicateReport {
        groups,
        rows_removed,
        missing_rows,
        physical_rows,
        repointed,
        masters,
        token,
        snapshot_digest: snapshot.digest.clone(),
    }
}

/// What a file check is doing, for a job's progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStage {
    /// Whether each file the rows record is there, and its size.
    Files,
    /// The two ends of copies that could hold one exposure.
    Copies,
}

/// How a plan checks its files: on `workers` threads, telling `progress`
/// of each file done, and stopping between files once `stop` is set.
pub struct Checking<'a> {
    pub workers: usize,
    pub progress: &'a (dyn Fn(CheckStage, usize, usize) + Sync),
    pub stop: &'a AtomicBool,
}

/// A check stopped when asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped;

/// [`plan_repair`], with the files checked first in parallel: each file
/// the rows that could be duplicates record is looked at once, and copies
/// are read only where two on disk share a size. Over a network share
/// this is most of a plan's time.
pub fn plan_repair_checked(
    snapshot: &DuplicateSnapshot,
    files: &FileAccess,
    checking: &Checking<'_>,
) -> std::result::Result<DuplicateReport, Stopped> {
    let checked = check_files(snapshot, files, checking)?;
    Ok(plan_repair(snapshot, &checked))
}

/// Run `work` for each of `len` items on the checking threads, in order.
fn each_checked<T: Send>(
    len: usize,
    stage: CheckStage,
    checking: &Checking<'_>,
    work: impl Fn(usize) -> T + Sync,
) -> std::result::Result<Vec<T>, Stopped> {
    let results = (0..len).map(|_| Mutex::new(None)).collect::<Vec<_>>();
    let done = AtomicUsize::new(0);
    (checking.progress)(stage, 0, len);
    crate::concurrency::parallel_index(len, checking.workers, |index| {
        if checking.stop.load(Ordering::Relaxed) {
            return;
        }
        let value = work(index);
        *results[index]
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(value);
        let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
        (checking.progress)(stage, finished, len);
    });
    if checking.stop.load(Ordering::Relaxed) {
        return Err(Stopped);
    }
    Ok(results
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .expect("every item was checked")
        })
        .collect())
}

fn check_files(
    snapshot: &DuplicateSnapshot,
    files: &FileAccess,
    checking: &Checking<'_>,
) -> std::result::Result<FileAccess, Stopped> {
    let by_key = candidates(&snapshot.rows);
    let paths = by_key
        .values()
        .flatten()
        .map(|row| PathBuf::from(&row.source_path))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let sizes = each_checked(paths.len(), CheckStage::Files, checking, |index| {
        files.size(&paths[index])
    })?;
    let size_of = paths
        .iter()
        .cloned()
        .zip(sizes)
        .collect::<HashMap<PathBuf, Option<u64>>>();
    // Only copies on disk that share a size with another copy of the same
    // exposure key are read, byte for byte at both ends.
    let mut to_sample = BTreeSet::new();
    for rows in by_key.values() {
        let mut by_size: HashMap<u64, Vec<&Path>> = HashMap::new();
        for row in rows {
            let path = Path::new(&row.source_path);
            if let Some(Some(size)) = size_of.get(path) {
                by_size.entry(*size).or_default().push(path);
            }
        }
        for same in by_size.into_values().filter(|same| same.len() > 1) {
            to_sample.extend(same.into_iter().map(Path::to_path_buf));
        }
    }
    let to_sample = to_sample.into_iter().collect::<Vec<_>>();
    let samples = each_checked(to_sample.len(), CheckStage::Copies, checking, |index| {
        files.sample(&to_sample[index])
    })?;
    let mut known = size_of
        .into_iter()
        .map(|(path, size)| (path, KnownFile { size, sample: None }))
        .collect::<HashMap<_, _>>();
    for (path, sample) in to_sample.into_iter().zip(samples) {
        if let Some(entry) = known.get_mut(&path) {
            entry.sample = Some(sample);
        }
    }
    Ok(FileAccess {
        resolve: Arc::clone(&files.resolve),
        known: Some(Arc::new(known)),
    })
}

fn row_copy<'a>(key: &'a ExposureKey, row: &'a RecordedRow) -> ExposureCopy<'a> {
    ExposureCopy {
        key,
        settings: &row.settings,
        path: Path::new(&row.source_path),
        recorded_size: row.file_size,
    }
}

fn twin_group(
    snapshot: &DuplicateSnapshot,
    members: Vec<&RecordedRow>,
    files: &FileAccess,
) -> TwinGroup {
    let keep = members[0];
    let exists = members
        .iter()
        .map(|row| files.exists(Path::new(&row.source_path)))
        .collect::<Vec<_>>();
    // The kept row's own file if it is there; otherwise the oldest live
    // twin's, whose row then goes.
    let target = if exists[0] {
        0
    } else {
        exists.iter().position(|live| *live).unwrap_or(0)
    };
    let repoint = (target != 0).then(|| members[target].frame_uuid.clone());
    let twins = members
        .iter()
        .zip(&exists)
        .enumerate()
        .filter(|(index, (_, live))| **live && *index != 0 && *index != target)
        .map(|(_, (row, _))| (row.source_path.clone(), row.source_fingerprint.clone()))
        .collect();
    let validity = if keep.valid_direction.is_some() {
        None
    } else {
        members.iter().find_map(|row| row.valid_direction.clone())
    };
    let file_name = Path::new(&members[target].source_path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    TwinGroup {
        kind: keep.kind,
        captured_at: keep.captured_at,
        file_name,
        rig_uuid: keep.rig_uuid.clone(),
        rig_name: snapshot.rig_names.get(&keep.rig_uuid).cloned(),
        keep: keep.frame_uuid.clone(),
        path: members[target].source_path.clone(),
        repointed: target != 0,
        physical: exists.iter().filter(|live| **live).count() > 1,
        members: members
            .iter()
            .zip(&exists)
            .enumerate()
            .map(|(index, (row, live))| TwinMember {
                frame_uuid: row.frame_uuid.clone(),
                source_path: row.source_path.clone(),
                exists: *live,
                keep: index == 0,
            })
            .collect(),
        repoint,
        twins,
        validity,
    }
}

/// What a repair did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RepairOutcome {
    pub groups: usize,
    pub rows_removed: usize,
    pub repointed: usize,
    pub masters_removed: usize,
}

/// Why a repair was not applied.
#[derive(Debug)]
pub enum RepairError {
    /// The rows the plan was made from changed after it was made.
    Stale,
    /// Stop was asked for: nothing was written.
    Stopped,
    Failed(anyhow::Error),
}

impl std::fmt::Display for RepairError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale => formatter.write_str(
                "the calibration library changed since the preview; preview the repair again",
            ),
            Self::Stopped => formatter.write_str("the repair was stopped; nothing was changed"),
            Self::Failed(error) => write!(formatter, "{error:#}"),
        }
    }
}

impl std::error::Error for RepairError {}

impl From<anyhow::Error> for RepairError {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed(error)
    }
}

impl From<rusqlite::Error> for RepairError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Failed(error.into())
    }
}

/// Apply a plan in one transaction, if the catalog still holds what it was
/// planned from. For each group the oldest row stays, with its uuid, and
/// moves to a live twin's file when its own is gone; the others are merged
/// into it and deleted. Grades, a person's keep-out or use choice, the
/// validity mark and a remote upload's record carry over; second copies
/// still on disk are remembered as twins so a scan skips them; every master
/// built from a deleted row, and every master built on one of those, goes so
/// it is built again. No file is moved or deleted.
pub fn apply_repair(
    conn: &mut Connection,
    report: &DuplicateReport,
) -> std::result::Result<RepairOutcome, RepairError> {
    apply_repair_with(conn, report, &|_, _| {}, &AtomicBool::new(false))
}

/// [`apply_repair`], telling `progress` of each exposure merged, and
/// stopping between exposures once `stop` is set. A stopped repair rolls
/// back whole: nothing is written.
///
/// The rows that could be duplicates must be as they were planned from,
/// or [`RepairError::Stale`]. The masters are read again here, so one built
/// since the preview from a row that goes, or on such a master, goes too.
pub fn apply_repair_with(
    conn: &mut Connection,
    report: &DuplicateReport,
    progress: &dyn Fn(usize, usize),
    stop: &AtomicBool,
) -> std::result::Result<RepairOutcome, RepairError> {
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = duplicate_snapshot(&transaction)?;
    if current.digest != report.snapshot_digest {
        return Err(RepairError::Stale);
    }
    if report.is_empty() {
        return Ok(RepairOutcome::default());
    }
    super::ensure_schema(&transaction)?;
    let now = chrono::Utc::now().timestamp();
    let remote_table = table_exists(&transaction, "psf_guard_remote_calibration_file");

    // Every row that goes, and every kept row that moves to a twin's file.
    let changed = report
        .groups
        .iter()
        .flat_map(|group| {
            group
                .members
                .iter()
                .filter(|member| !member.keep)
                .map(|member| member.frame_uuid.as_str())
                .chain(group.repointed.then_some(group.keep.as_str()))
        })
        .collect::<HashSet<_>>();
    let invalid = citing(&current.masters, &changed);
    let mut invalid_masters = current
        .masters
        .iter()
        .filter(|master| invalid.contains(&master.uuid))
        .collect::<Vec<_>>();
    invalid_masters.sort_by(|left, right| left.uuid.cmp(&right.uuid));
    {
        let mut statement = transaction
            .prepare("DELETE FROM psf_guard_calibration_master WHERE master_uuid = ?1")?;
        for master in &invalid_masters {
            statement.execute([&master.uuid])?;
        }
    }
    let mut outcome = RepairOutcome {
        groups: report.groups.len(),
        masters_removed: invalid_masters.len(),
        ..Default::default()
    };
    let total = report.groups.len();
    progress(0, total);
    for (index, group) in report.groups.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return Err(RepairError::Stopped);
        }
        let removed = group
            .members
            .iter()
            .filter(|member| !member.keep)
            .map(|member| member.frame_uuid.clone())
            .collect::<Vec<_>>();
        outcome.rows_removed += merge_rows(
            &transaction,
            &Merge {
                keep: &group.keep,
                removed: &removed,
                repoint_from: group.repoint.as_deref(),
                path: &group.path,
                twins: &group.twins,
                validity: group.validity.as_deref(),
            },
            remote_table,
            now,
        )?;
        if group.repointed {
            outcome.repointed += 1;
        }
        tracing::info!(
            "calibration repair: kept {} {} at {} ({}); merged {} twin row(s): {}",
            group.kind.as_str(),
            group.keep,
            group.path,
            if group.repointed {
                "moved to a live twin"
            } else {
                "its own file"
            },
            removed.len(),
            group
                .members
                .iter()
                .filter(|member| !member.keep)
                .map(|member| format!(
                    "{} {}{}",
                    member.frame_uuid,
                    member.source_path,
                    if member.exists { "" } else { " (missing)" }
                ))
                .collect::<Vec<_>>()
                .join("; ")
        );
        progress(index + 1, total);
    }
    if stop.load(Ordering::Relaxed) {
        return Err(RepairError::Stopped);
    }
    for master in &invalid_masters {
        tracing::info!(
            "calibration repair: {} master {} will be built again",
            master.kind,
            master.uuid
        );
    }
    prune_twins(&transaction)?;
    transaction.commit()?;
    tracing::info!(
        "calibration repair: {} group(s), {} row(s) merged, {} re-pointed, {} master(s) invalidated",
        outcome.groups,
        outcome.rows_removed,
        outcome.repointed,
        outcome.masters_removed
    );
    Ok(outcome)
}

/// One exposure's rows folding into the row kept.
struct Merge<'a> {
    keep: &'a str,
    removed: &'a [String],
    /// The twin whose file the kept row moves to, its own being gone.
    repoint_from: Option<&'a str>,
    /// The kept row's file afterwards.
    path: &'a str,
    /// Second copies still on disk, by path and fingerprint.
    twins: &'a [(String, String)],
    /// A validity mark the kept row takes when it has none.
    validity: Option<&'a str>,
}

/// Fold `merge.removed` into `merge.keep`: the kept row takes a twin's file
/// and header when it moves, the grade rows and a person's choice merge, a
/// remote upload's record follows, the other rows go, their second copies
/// are remembered, and each merge is recorded so a sync honours it. Masters
/// are the caller's. Returns the rows deleted.
fn merge_rows(conn: &Connection, merge: &Merge<'_>, remote_table: bool, now: i64) -> Result<usize> {
    if let Some(target) = merge.repoint_from {
        let present = super::table_column_names(conn, "psf_guard_calibration_frame")?
            .into_iter()
            .map(|name| name.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        let columns = FILE_COLUMNS
            .iter()
            .filter(|column| present.contains(**column))
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        conn.execute(
            &format!(
                "UPDATE psf_guard_calibration_frame
                 SET ({columns}) = (SELECT {columns} FROM psf_guard_calibration_frame
                                    WHERE frame_uuid = ?1),
                     updated_at = ?2
                 WHERE frame_uuid = ?3"
            ),
            params![target, now, merge.keep],
        )?;
    }
    let fingerprint: String = conn.query_row(
        "SELECT source_fingerprint FROM psf_guard_calibration_frame WHERE frame_uuid = ?1",
        [merge.keep],
        |row| row.get(0),
    )?;
    let members = std::iter::once(merge.keep)
        .chain(merge.removed.iter().map(String::as_str))
        .collect::<Vec<_>>();
    merge_quality(conn, merge.keep, &members, &fingerprint, now)?;
    if remote_table {
        merge_remote_record(conn, merge.keep, &members)?;
    }
    // A flat response built from either row is built from rows that no
    // longer stand as they were: it is measured again.
    super::flat_groups::store::forget_frames(
        conn,
        &members
            .iter()
            .map(|uuid| uuid.to_string())
            .collect::<Vec<_>>(),
    )?;
    let mut removed = 0;
    {
        let mut statement =
            conn.prepare("DELETE FROM psf_guard_calibration_frame WHERE frame_uuid = ?1")?;
        for frame_uuid in merge.removed {
            removed += statement.execute([frame_uuid])?;
        }
    }
    if merge.repoint_from.is_some() {
        conn.execute(
            "UPDATE psf_guard_calibration_frame SET source_path = ?1 WHERE frame_uuid = ?2",
            params![merge.path, merge.keep],
        )?;
    }
    if let Some(direction) = merge.validity {
        conn.execute(
            "UPDATE psf_guard_calibration_frame SET valid_direction = ?1, updated_at = ?2
             WHERE frame_uuid = ?3 AND valid_direction IS NULL",
            params![direction, now, merge.keep],
        )?;
    }
    {
        let mut twins = conn.prepare(
            "UPDATE psf_guard_calibration_twin SET frame_uuid = ?1 WHERE frame_uuid = ?2",
        )?;
        let mut recorded = conn.prepare(
            "INSERT INTO psf_guard_calibration_merged (merged_uuid, keep_uuid, merged_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(merged_uuid) DO UPDATE SET
                keep_uuid = excluded.keep_uuid, merged_at = excluded.merged_at",
        )?;
        let mut chained = conn.prepare(
            "UPDATE psf_guard_calibration_merged SET keep_uuid = ?1 WHERE keep_uuid = ?2",
        )?;
        for frame_uuid in merge.removed {
            twins.execute(params![merge.keep, frame_uuid])?;
            chained.execute(params![merge.keep, frame_uuid])?;
            recorded.execute(params![frame_uuid, merge.keep, now])?;
        }
    }
    // A person's split, join or group choice named a merged row's flat:
    // it names the kept row's now.
    super::flat_groups::override_store::repoint_frames(conn, merge.removed, merge.keep)?;
    forget_twin(conn, merge.path)?;
    for (path, fingerprint) in merge.twins {
        record_twin(conn, path, merge.keep, fingerprint, now)?;
    }
    Ok(removed)
}

/// One grade row for the kept frame: the measurement that matches its
/// file, else its own, else the newest; and a person's most recent choice
/// among all of them — a cleared one too, which is a choice.
fn merge_quality(
    conn: &Connection,
    keep: &str,
    members: &[&str],
    keep_fingerprint: &str,
    now: i64,
) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT frame_uuid, source_fingerprint, measured_at, override_choice, override_at
         FROM psf_guard_calibration_frame_quality WHERE frame_uuid = ?1",
    )?;
    let mut rows = Vec::new();
    for member in members {
        if let Some(row) = statement
            .query_row([member], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            })
            .optional()?
        {
            rows.push(row);
        }
    }
    if rows.is_empty() {
        return Ok(());
    }
    let measured_here = |row: &&(
        String,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<i64>,
    )| { row.2.is_some() && row.1.as_deref() == Some(keep_fingerprint) };
    let base = rows
        .iter()
        .filter(measured_here)
        .find(|row| row.0 == keep)
        .or_else(|| rows.iter().find(measured_here))
        .or_else(|| rows.iter().find(|row| row.0 == keep))
        .or_else(|| rows.iter().max_by_key(|row| row.2))
        .map(|row| row.0.clone())
        .expect("rows is not empty");
    let choice = rows
        .iter()
        .filter(|row| row.4.is_some())
        .max_by_key(|row| row.4)
        .map(|row| (row.3.clone(), row.4));
    for (frame_uuid, ..) in &rows {
        if *frame_uuid != base {
            conn.execute(
                "DELETE FROM psf_guard_calibration_frame_quality WHERE frame_uuid = ?1",
                [frame_uuid],
            )?;
        }
    }
    let (choice, choice_at) = choice.unwrap_or((None, None));
    conn.execute(
        "UPDATE psf_guard_calibration_frame_quality
         SET frame_uuid = ?1, override_choice = ?2, override_at = ?3, updated_at = ?4
         WHERE frame_uuid = ?5",
        params![keep, choice, choice_at, now, base],
    )?;
    Ok(())
}

/// A remote upload's record of a frame follows the kept row.
fn merge_remote_record(conn: &Connection, keep: &str, members: &[&str]) -> Result<()> {
    let kept_has = conn
        .query_row(
            "SELECT 1 FROM psf_guard_remote_calibration_file WHERE frame_uuid = ?1",
            [keep],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    let mut adopted = kept_has;
    for member in members.iter().filter(|member| **member != keep) {
        if adopted {
            conn.execute(
                "DELETE FROM psf_guard_remote_calibration_file WHERE frame_uuid = ?1",
                [member],
            )?;
        } else {
            adopted = conn.execute(
                "UPDATE psf_guard_remote_calibration_file SET frame_uuid = ?1 WHERE frame_uuid = ?2",
                params![keep, member],
            )? > 0;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Sync
// ---------------------------------------------------------------------------

/// Copy the source's merge records that this catalog lacks. Returns how
/// many were new.
pub(crate) fn sync_merges(source: &Connection, destination: &Connection) -> Result<usize> {
    if !table_exists(source, MERGED_TABLE) {
        return Ok(0);
    }
    let mut statement = source
        .prepare("SELECT merged_uuid, keep_uuid, merged_at FROM psf_guard_calibration_merged")?;
    let records = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut insert = destination.prepare(
        "INSERT INTO psf_guard_calibration_merged (merged_uuid, keep_uuid, merged_at)
         VALUES (?1, ?2, ?3) ON CONFLICT(merged_uuid) DO NOTHING",
    )?;
    let mut added = 0;
    for (merged, keep, at) in records {
        added += insert.execute(params![merged, keep, at])?;
    }
    Ok(added)
}

/// Every merged uuid this catalog records, and the row it lives on in now,
/// following a merge into a row that was later merged itself.
pub(crate) fn merge_map(conn: &Connection) -> Result<HashMap<String, String>> {
    if !table_exists(conn, MERGED_TABLE) {
        return Ok(HashMap::new());
    }
    let mut statement =
        conn.prepare("SELECT merged_uuid, keep_uuid FROM psf_guard_calibration_merged")?;
    let direct = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    Ok(direct
        .keys()
        .map(|merged| {
            let mut keep = &direct[merged];
            for _ in 0..direct.len() {
                match direct.get(keep) {
                    Some(next) if next != merged => keep = next,
                    _ => break,
                }
            }
            (merged.clone(), keep.clone())
        })
        .collect())
}

/// What folding a source's merges into this catalog did.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FoldOutcome {
    pub rows: usize,
    pub masters: usize,
}

/// Merge each row this catalog still holds that `map` says was merged into
/// another row it also holds, as the repair that recorded the merge did:
/// the kept row moves to the merged row's file when its own is gone here,
/// the grades and choices merge, and the masters built from either go.
pub(crate) fn fold_merged(conn: &Connection, map: &HashMap<String, String>) -> Result<FoldOutcome> {
    let mut outcome = FoldOutcome::default();
    if map.is_empty() {
        return Ok(outcome);
    }
    let now = chrono::Utc::now().timestamp();
    let remote_table = table_exists(conn, "psf_guard_remote_calibration_file");
    let row = |uuid: &str| {
        conn.query_row(
            "SELECT source_path, source_fingerprint, valid_direction
             FROM psf_guard_calibration_frame WHERE frame_uuid = ?1",
            [uuid],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
    };
    let mut pairs = map.iter().collect::<Vec<_>>();
    pairs.sort();
    for (merged, keep) in pairs {
        let (Some(gone), Some(kept)) = (row(merged)?, row(keep)?) else {
            continue;
        };
        let moves = !Path::new(&kept.0).is_file() && Path::new(&gone.0).is_file();
        let mut changed = HashSet::from([merged.as_str()]);
        if moves {
            changed.insert(keep.as_str());
        }
        let invalid = masters_citing(conn, &changed)?;
        for master in &invalid {
            conn.execute(
                "DELETE FROM psf_guard_calibration_master WHERE master_uuid = ?1",
                [master],
            )?;
        }
        outcome.masters += invalid.len();
        let twins = if !moves && Path::new(&gone.0).is_file() {
            vec![(gone.0.clone(), gone.1.clone())]
        } else {
            Vec::new()
        };
        let path = if moves {
            gone.0.clone()
        } else {
            kept.0.clone()
        };
        outcome.rows += merge_rows(
            conn,
            &Merge {
                keep,
                removed: std::slice::from_ref(merged),
                repoint_from: moves.then_some(merged.as_str()),
                path: &path,
                twins: &twins,
                validity: if kept.2.is_none() {
                    gone.2.as_deref()
                } else {
                    None
                },
            },
            remote_table,
            now,
        )?;
        tracing::info!("calibration sync: merged {merged} into {keep}, as the source's repair did");
    }
    Ok(outcome)
}

/// Held while a repair applies, so the command line and a server never
/// repair one catalog at once. It is a lock on a file beside the catalog,
/// which the operating system lets go of when the process ends, however it
/// ends. The file stays: removing it could hand the lock to two at once.
#[derive(Debug)]
pub struct RepairLock {
    _file: Option<std::fs::File>,
}

/// Why a repair could not take [`RepairLock`].
#[derive(Debug)]
pub enum RepairLockError {
    /// Another process, or another repair here, holds it.
    Busy,
    Failed(std::io::Error),
}

impl std::fmt::Display for RepairLockError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => formatter.write_str(
                "another repair of this catalog is running, from the command line or a server; try again when it has finished",
            ),
            Self::Failed(error) => write!(formatter, "could not lock the catalog for a repair: {error}"),
        }
    }
}

impl std::error::Error for RepairLockError {}

/// The lock file of `catalog`'s repairs.
pub fn repair_lock_path(catalog: &Path) -> PathBuf {
    let mut name = catalog.as_os_str().to_owned();
    name.push(".psf-guard-repair.lock");
    PathBuf::from(name)
}

/// Take `catalog`'s repair lock, without waiting. Only another holder
/// refuses the repair. A file system that cannot lock files (ENOLCK, or
/// EOPNOTSUPP, which arrives uncategorised) lets it go ahead unlocked, with
/// a warning: a guard rail must not make the repair impossible. A lock
/// file that cannot be opened at all is an error.
pub fn repair_lock(catalog: &Path) -> std::result::Result<RepairLock, RepairLockError> {
    let path = repair_lock_path(catalog);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(RepairLockError::Failed)?;
    match file.try_lock() {
        Ok(()) => Ok(RepairLock { _file: Some(file) }),
        Err(std::fs::TryLockError::WouldBlock) => Err(RepairLockError::Busy),
        Err(std::fs::TryLockError::Error(error)) => {
            tracing::warn!(
                "{} cannot be locked ({error}); repairing without the lock",
                path.display()
            );
            Ok(RepairLock { _file: None })
        }
    }
}

#[cfg(test)]
mod tests;
