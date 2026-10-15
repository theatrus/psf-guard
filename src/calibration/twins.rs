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
//! repair asks it to find twins already recorded ([`plan_repair`]). Nothing
//! here moves or deletes a file.

use super::{canonical_text, hex_digest, CalibrationKind};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::UNIX_EPOCH;

/// Copies of a frame that the catalog knows are not its recorded file. A
/// scan skips an unchanged copy here without reading its header, as long as
/// the recorded file is still there; once it is gone, the next scan
/// re-points the row to the copy. Paths are this host's, so the table is
/// never synced.
pub(crate) const TWIN_TABLE_SQL: &str = r#"
    CREATE TABLE IF NOT EXISTS psf_guard_calibration_twin (
        source_path        TEXT PRIMARY KEY,
        frame_uuid         TEXT NOT NULL,
        source_fingerprint TEXT NOT NULL,
        updated_at         INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_psf_guard_calibration_twin_frame
        ON psf_guard_calibration_twin(frame_uuid);
"#;

const TWIN_TABLE: &str = "psf_guard_calibration_twin";

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

/// One copy: a recorded row or a scanned file.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ExposureCopy<'a> {
    pub key: &'a ExposureKey,
    pub path: &'a Path,
    /// The size the catalog recorded, for a copy whose file is gone.
    pub recorded_size: Option<i64>,
}

/// How a recorded path is read on this machine. The server reads only
/// inside a database's folders, as the calibration browser does; a dry run
/// against a copied catalog can map the recording host's mount onto this
/// one's.
/// Where a recorded path is read on this machine, or `None` for nowhere.
type Resolve = dyn Fn(&Path) -> Option<PathBuf> + Send + Sync;

#[derive(Clone)]
pub struct FileAccess {
    resolve: Arc<Resolve>,
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

    pub fn exists(&self, path: &Path) -> bool {
        self.local(path).is_some()
    }

    fn size(&self, path: &Path) -> Option<u64> {
        let local = self.local(path)?;
        std::fs::metadata(local).ok().map(|metadata| metadata.len())
    }

    fn sample(&self, path: &Path) -> Option<String> {
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
/// rig — and on file size, read from disk or, for a file that is gone, from
/// the catalog. When both files are there, their headers and the ends of
/// their pixels must also be byte for byte the same, so a different frame
/// that happens to share a name and second is never taken for a copy. When
/// one is gone there is nothing left to compare, and the catalog's record
/// decides.
pub(crate) fn same_exposure(
    left: &ExposureCopy<'_>,
    right: &ExposureCopy<'_>,
    files: &FileAccess,
) -> bool {
    if left.key != right.key {
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
        _ => true,
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
    path: &Path,
    files: &FileAccess,
) -> Result<Option<RecordedCopy>> {
    let mut statement = conn.prepare(
        "SELECT id, frame_uuid, source_path, file_size FROM psf_guard_calibration_frame
         WHERE kind = ?1 AND captured_at IS ?2 AND rig_uuid = ?3 AND source_path <> ?4
         ORDER BY id",
    )?;
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
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let scanned = ExposureCopy {
        key,
        path,
        recorded_size: None,
    };
    let mut moved = None;
    for (id, frame_uuid, recorded, recorded_size) in rows {
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

fn twin_table_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [TWIN_TABLE],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
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
    file_mtime_ns: Option<String>,
    valid_direction: Option<String>,
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

/// The rows and masters a repair is planned from, and a digest of them that
/// says whether the catalog has changed since.
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
                captured_at, file_size, file_mtime_ns, {validity}
         FROM psf_guard_calibration_frame ORDER BY id"
    ))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter_map(
            |(id, uuid, rig, kind, path, fingerprint, captured, size, mtime, validity)| {
                Some(RecordedRow {
                    id,
                    frame_uuid: uuid,
                    rig_uuid: rig,
                    kind: CalibrationKind::from_db(&kind)?,
                    source_path: path,
                    source_fingerprint: fingerprint,
                    captured_at: captured,
                    file_size: size,
                    file_mtime_ns: mtime,
                    valid_direction: validity,
                })
            },
        )
        .collect::<Vec<_>>();
    let masters = master_links(conn)?;
    let mut statement = conn.prepare("SELECT rig_uuid, name FROM psf_guard_rig")?;
    let rig_names = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<HashMap<String, String>>>()?;

    let mut hasher = Sha256::new();
    for row in &rows {
        hasher.update(
            format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1e}",
                row.id,
                row.frame_uuid,
                row.rig_uuid,
                row.kind.as_str(),
                row.source_path,
                row.source_fingerprint,
                row.captured_at,
                row.file_size,
                row.valid_direction,
            )
            .as_bytes(),
        );
    }
    let mut sorted = masters.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.uuid.cmp(&right.uuid));
    for master in sorted {
        hasher.update(
            format!(
                "M{}\u{1f}{}\u{1f}{:?}\u{1f}{:?}\u{1e}",
                master.uuid,
                master.sources.join(","),
                master.bias,
                master.dark
            )
            .as_bytes(),
        );
    }
    let digest = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(DuplicateSnapshot {
        rows,
        masters,
        rig_names,
        digest,
    })
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
    #[serde(skip)]
    repoint: Option<Repoint>,
    #[serde(skip)]
    twins: Vec<(String, String)>,
    #[serde(skip)]
    validity: Option<String>,
}

#[derive(Debug, Clone)]
struct Repoint {
    fingerprint: String,
    size: Option<i64>,
    mtime_ns: Option<String>,
}

/// A generated master the repair removes, so it is built again.
#[derive(Debug, Clone, Serialize)]
pub struct InvalidatedMaster {
    pub master_uuid: String,
    pub kind: String,
    pub source_count: usize,
    pub created_at: i64,
    /// Whether it cites a row the repair removes itself, rather than being
    /// built on a master that does.
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
    /// Hands the plan to [`apply_repair`]; any change to the catalog or to
    /// which files are there gives another.
    pub token: String,
    #[serde(skip)]
    snapshot_digest: String,
}

impl DuplicateReport {
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
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
    let mut by_key: BTreeMap<ExposureKey, Vec<&RecordedRow>> = BTreeMap::new();
    for row in &snapshot.rows {
        if let Some(key) = ExposureKey::new(
            row.kind,
            row.captured_at,
            Path::new(&row.source_path),
            &row.rig_uuid,
        ) {
            by_key.entry(key).or_default().push(row);
        }
    }

    let mut groups = Vec::new();
    for (key, rows) in &by_key {
        if rows.len() < 2 {
            continue;
        }
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
    let invalid = citing(&snapshot.masters, &removed);
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
                .any(|source| removed.contains(source.as_str())),
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
    for master in &masters {
        plan.update(format!("M{}", master.master_uuid).as_bytes());
    }
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

fn row_copy<'a>(key: &'a ExposureKey, row: &'a RecordedRow) -> ExposureCopy<'a> {
    ExposureCopy {
        key,
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
    let repoint = (target != 0).then(|| Repoint {
        fingerprint: members[target].source_fingerprint.clone(),
        size: members[target].file_size,
        mtime_ns: members[target].file_mtime_ns.clone(),
    });
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
    /// The catalog changed after the plan was made.
    Stale,
    Failed(anyhow::Error),
}

impl std::fmt::Display for RepairError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale => formatter.write_str(
                "the calibration library changed since the preview; preview the repair again",
            ),
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
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if duplicate_snapshot(&transaction)?.digest != report.snapshot_digest {
        return Err(RepairError::Stale);
    }
    if report.is_empty() {
        return Ok(RepairOutcome::default());
    }
    super::ensure_schema(&transaction)?;
    let now = chrono::Utc::now().timestamp();
    let remote_table = super::table_column_names(&transaction, "psf_guard_remote_calibration_file")
        .map(|columns| !columns.is_empty())
        .unwrap_or(false);

    for master in &report.masters {
        transaction.execute(
            "DELETE FROM psf_guard_calibration_master WHERE master_uuid = ?1",
            [&master.master_uuid],
        )?;
    }
    let mut outcome = RepairOutcome {
        groups: report.groups.len(),
        masters_removed: report.masters.len(),
        ..Default::default()
    };
    for group in &report.groups {
        let removed = group
            .members
            .iter()
            .filter(|member| !member.keep)
            .map(|member| member.frame_uuid.clone())
            .collect::<Vec<_>>();
        let members = group
            .members
            .iter()
            .map(|member| member.frame_uuid.as_str())
            .collect::<Vec<_>>();
        let fingerprint = group
            .repoint
            .as_ref()
            .map(|repoint| repoint.fingerprint.clone())
            .map(Ok)
            .unwrap_or_else(|| {
                transaction.query_row(
                    "SELECT source_fingerprint FROM psf_guard_calibration_frame WHERE frame_uuid = ?1",
                    [&group.keep],
                    |row| row.get::<_, String>(0),
                )
            })?;
        merge_quality(&transaction, &group.keep, &members, &fingerprint, now)?;
        if remote_table {
            merge_remote_record(&transaction, &group.keep, &members)?;
        }
        {
            let mut statement = transaction
                .prepare("DELETE FROM psf_guard_calibration_frame WHERE frame_uuid = ?1")?;
            for frame_uuid in &removed {
                outcome.rows_removed += statement.execute([frame_uuid])?;
            }
        }
        if let Some(repoint) = &group.repoint {
            transaction.execute(
                "UPDATE psf_guard_calibration_frame
                 SET source_path = ?1, source_fingerprint = ?2, file_size = ?3,
                     file_mtime_ns = ?4, updated_at = ?5
                 WHERE frame_uuid = ?6",
                params![
                    group.path,
                    repoint.fingerprint,
                    repoint.size,
                    repoint.mtime_ns,
                    now,
                    group.keep
                ],
            )?;
            outcome.repointed += 1;
        }
        if let Some(direction) = &group.validity {
            transaction.execute(
                "UPDATE psf_guard_calibration_frame SET valid_direction = ?1, updated_at = ?2
                 WHERE frame_uuid = ?3 AND valid_direction IS NULL",
                params![direction, now, group.keep],
            )?;
        }
        {
            let mut statement = transaction.prepare(
                "UPDATE psf_guard_calibration_twin SET frame_uuid = ?1 WHERE frame_uuid = ?2",
            )?;
            for frame_uuid in &removed {
                statement.execute(params![group.keep, frame_uuid])?;
            }
        }
        forget_twin(&transaction, &group.path)?;
        for (path, fingerprint) in &group.twins {
            record_twin(&transaction, path, &group.keep, fingerprint, now)?;
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
    }
    for master in &report.masters {
        tracing::info!(
            "calibration repair: {} master {} will be built again",
            master.kind,
            master.master_uuid
        );
    }
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

/// One grade row for the kept frame: the measurement that matches its
/// file, else its own, else the newest; and a person's most recent choice
/// among all of them.
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
        .filter(|row| row.3.is_some())
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

#[cfg(test)]
mod tests;
