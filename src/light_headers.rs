//! Light headers kept in the catalog.
//!
//! Preparing a stack, judging calibration health and finding the lights the
//! library cannot calibrate all start from each light's FITS header. Read
//! from a network share a thousand at a time, those reads made one build
//! take seconds. So each header read is kept in `psf_guard_light_header`,
//! one row per light. A row stands while:
//!
//! - the light is the one the row was written for: its `acquiredimage` row
//!   has the same guid, or, in a catalog without guids, the same capture
//!   time and file name. An id SQLite hands out again after a delete names
//!   another light;
//! - the file at the row's path has the size, modification time and change
//!   time it had then. The change time cannot be set back, so a header edited
//!   in place is read again even when its size and modification time are
//!   kept;
//! - this release wrote it ([`reader`]): a release may read a header another
//!   way.
//!
//! Otherwise the header is read again.
//!
//! Import writes a light's row as it adds the light, the calibration health
//! pass writes the ones it reads, and stack preparation hands the ones it
//! had to read to [`crate::server::light_header_writes`]. A stack also notes
//! that its own file lookup found the light at the row's path (`located`), so
//! the next one goes straight there.
//!
//! The rows hold this machine's paths and file times. Sync and transfers
//! never carry them. Deleting them costs only reading the headers again.

use crate::commands::import::headers::{FrameMeta, FRAME_META_VERSION};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// One row per light, by `acquiredimage.Id`, for the light `image_key` names
/// ([`image_key_sql`]). Times are nanoseconds since 1970; `written_ns` is
/// when what the row says of the file last changed, so a view worked out
/// from a project's headers can tell when to work it out again.
pub const TABLE_SQL: &str = r#"
    CREATE TABLE IF NOT EXISTS psf_guard_light_header (
        image_id   INTEGER PRIMARY KEY,
        image_key  TEXT NOT NULL,
        path       TEXT NOT NULL,
        size       INTEGER NOT NULL,
        mtime_ns   INTEGER NOT NULL,
        ctime_ns   INTEGER NOT NULL,
        reader     TEXT NOT NULL,
        located    INTEGER NOT NULL DEFAULT 0,
        meta_json  TEXT NOT NULL,
        written_ns INTEGER NOT NULL
    );
"#;

/// Which reader wrote a header: this release, and [`FRAME_META_VERSION`]
/// for changes between releases. Headers another reader wrote are read
/// again, once a release, as the old per-release store did.
pub fn reader() -> String {
    format!("{}+{FRAME_META_VERSION}", env!("CARGO_PKG_VERSION"))
}

/// A file's size, modification time and change time: what tells a kept
/// header, or a checked calibration file, from one that changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileStamp {
    pub size: u64,
    /// Nanoseconds since 1970.
    pub modified_ns: u64,
    /// When the file's contents or attributes last changed, in nanoseconds
    /// since 1970. A tool can set the modification time back, not this. 0
    /// where the platform does not say.
    pub changed_ns: u64,
}

impl FileStamp {
    /// The file at `path` as it stands, or `None` when no file is there.
    pub fn of(path: &Path) -> Option<Self> {
        Self::from_metadata(&std::fs::metadata(path).ok()?)
    }

    pub fn from_metadata(metadata: &std::fs::Metadata) -> Option<Self> {
        if !metadata.is_file() {
            return None;
        }
        let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        Some(Self {
            size: metadata.len(),
            modified_ns: u64::try_from(modified.as_nanos()).ok()?,
            changed_ns: changed_ns(metadata)?,
        })
    }
}

#[cfg(unix)]
fn changed_ns(metadata: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    u64::try_from(metadata.ctime())
        .ok()?
        .checked_mul(1_000_000_000)?
        .checked_add(u64::try_from(metadata.ctime_nsec()).ok()?)
}

#[cfg(not(unix))]
fn changed_ns(_metadata: &std::fs::Metadata) -> Option<u64> {
    Some(0)
}

/// A light's header as it was read, and the file it was read from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeptHeader {
    pub path: PathBuf,
    pub size: u64,
    /// Nanoseconds since 1970.
    pub modified_ns: u64,
    /// See [`FileStamp::changed_ns`].
    #[serde(default)]
    pub changed_ns: u64,
    pub meta: FrameMeta,
    /// A stack's own file lookup found the light at `path`, so a stack may
    /// go straight there while the file stands as it was.
    #[serde(default)]
    pub located: bool,
    /// The light this header is of ([`image_key_sql`]) when the reader knew
    /// it; written only while the light's row still has it. `None` takes the
    /// row as it is when the header is kept.
    #[serde(default)]
    pub image_key: Option<String>,
}

impl KeptHeader {
    pub fn new(path: &Path, stamp: FileStamp, meta: FrameMeta) -> Self {
        Self {
            path: path.to_path_buf(),
            size: stamp.size,
            modified_ns: stamp.modified_ns,
            changed_ns: stamp.changed_ns,
            meta,
            located: false,
            image_key: None,
        }
    }

    pub fn stamp(&self) -> FileStamp {
        FileStamp {
            size: self.size,
            modified_ns: self.modified_ns,
            changed_ns: self.changed_ns,
        }
    }

    /// Whether this is the header of the file at `path`, which `stamp`
    /// found as it stands.
    pub fn holds(&self, path: &Path, stamp: Option<FileStamp>) -> bool {
        self.path == path && stamp == Some(self.stamp())
    }
}

pub fn ensure_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(TABLE_SQL)
        .context("creating the PSF Guard light header table")
}

/// Whether the catalog has the table. One that never had a header kept has
/// none, and every header is read from its file.
pub fn table_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'psf_guard_light_header'",
        [],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .unwrap_or(false)
}

/// The SQL naming the light `acquiredimage a` is: its guid, or, in a
/// catalog without guids or for a row without one, its capture time and
/// file name. Never NULL.
fn image_key_sql(conn: &Connection) -> String {
    let fallback = "COALESCE(CAST(a.acquireddate AS TEXT), '') || '|' ||                     COALESCE(CASE WHEN json_valid(a.metadata)                              THEN json_extract(a.metadata, '$.FileName') END, '')";
    if crate::frame_derivatives::catalog_has_guids(conn) {
        format!("COALESCE(NULLIF(a.guid, ''), {fallback})")
    } else {
        fallback.to_string()
    }
}

/// The kept headers of a project's lights, by light id: those of the
/// lights the rows were written for, by this reader.
pub fn project_headers(conn: &Connection, project_id: i32) -> Result<BTreeMap<i32, KeptHeader>> {
    if !table_exists(conn) {
        return Ok(BTreeMap::new());
    }
    let mut statement = conn.prepare(&format!(
        "SELECT h.image_id, h.path, h.size, h.mtime_ns, h.ctime_ns, h.located, h.meta_json,
                h.image_key
         FROM psf_guard_light_header h
         JOIN acquiredimage a ON a.Id = h.image_id
         WHERE a.projectId = ?1 AND h.reader = ?2 AND h.image_key = {}",
        image_key_sql(conn)
    ))?;
    let rows = statement.query_map(params![project_id, reader()], |row| {
        Ok((
            row.get::<_, i32>(0)?,
            row.get::<_, String>(1)?,
            [
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ],
            row.get::<_, bool>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
        ))
    })?;
    let mut headers = BTreeMap::new();
    for row in rows {
        let (image_id, path, stamp, located, meta_json, image_key) = row?;
        // A row that does not parse is read again, as a missing one is.
        let Ok(mut meta) = serde_json::from_str::<FrameMeta>(&meta_json) else {
            continue;
        };
        meta.path = PathBuf::from(&path);
        let [Ok(size), Ok(modified_ns), Ok(changed_ns)] = stamp.map(u64::try_from) else {
            continue;
        };
        headers.insert(
            image_id,
            KeptHeader {
                path: PathBuf::from(path),
                size,
                modified_ns,
                changed_ns,
                meta,
                located,
                image_key: Some(image_key),
            },
        );
    }
    Ok(headers)
}

/// What names each of a project's lights ([`image_key_sql`]), by light id.
fn project_keys(conn: &Connection, project_id: i32) -> Result<HashMap<i32, String>> {
    let mut statement = conn.prepare(&format!(
        "SELECT a.Id, {} FROM acquiredimage a WHERE a.projectId = ?1",
        image_key_sql(conn)
    ))?;
    Ok(statement
        .query_map([project_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

/// When a project's kept headers last changed. A header written again as it
/// was, or only marked located, leaves it, so what is worked out from the
/// headers is not worked out again for nothing.
pub fn project_stamp(conn: &Connection, project_id: i32) -> u128 {
    if !table_exists(conn) {
        return 0;
    }
    conn.query_row(
        &format!(
            "SELECT count(*), coalesce(max(h.written_ns), 0)
             FROM psf_guard_light_header h
             JOIN acquiredimage a ON a.Id = h.image_id
             WHERE a.projectId = ?1 AND h.reader = ?2 AND h.image_key = {}",
            image_key_sql(conn)
        ),
        params![project_id, reader()],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )
    .map(|(count, written)| ((count as u128) << 64) | written as u128)
    .unwrap_or(0)
}

/// Whether this build may write the catalog's PSF Guard tables: not one a
/// newer build has upgraded, whose shapes it does not know.
fn writable(conn: &Connection) -> bool {
    if crate::calibration::schema_is_newer(conn) {
        tracing::debug!("not keeping light headers in a catalog a newer build upgraded");
        return false;
    }
    true
}

/// Keep `headers`, by light id, in one transaction unless the caller holds
/// one. A header is kept only for a light the catalog still holds, and one
/// that names its light ([`KeptHeader::image_key`]) only while the light's row
/// still does. A header written again as it was is left alone, and a row a
/// stack located keeps that mark while its path stays. Returns the rows
/// written.
pub fn store<'a>(
    conn: &Connection,
    headers: impl IntoIterator<Item = (i32, &'a KeptHeader)>,
) -> Result<usize> {
    if !writable(conn) {
        return Ok(0);
    }
    ensure_table(conn)?;
    let transaction = if conn.is_autocommit() {
        Some(conn.unchecked_transaction()?)
    } else {
        None
    };
    let now = now_ns();
    let reader = reader();
    let key = image_key_sql(conn);
    let same = "path = excluded.path AND image_key = excluded.image_key                 AND size = excluded.size AND mtime_ns = excluded.mtime_ns                 AND ctime_ns = excluded.ctime_ns AND reader = excluded.reader";
    let mut written = 0;
    {
        let mut statement = conn.prepare(&format!(
            "INSERT INTO psf_guard_light_header
                (image_id, image_key, path, size, mtime_ns, ctime_ns, reader, located,
                 meta_json, written_ns)
             SELECT a.Id, {key}, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
             FROM acquiredimage a
             WHERE a.Id = ?1 AND (?10 IS NULL OR {key} = ?10)
             ON CONFLICT(image_id) DO UPDATE SET
                located = CASE WHEN path = excluded.path AND image_key = excluded.image_key
                               THEN max(located, excluded.located)
                               ELSE excluded.located END,
                written_ns = CASE WHEN {same} THEN written_ns ELSE excluded.written_ns END,
                image_key = excluded.image_key,
                path = excluded.path,
                size = excluded.size,
                mtime_ns = excluded.mtime_ns,
                ctime_ns = excluded.ctime_ns,
                reader = excluded.reader,
                meta_json = excluded.meta_json
             WHERE NOT ({same}) OR located < excluded.located"
        ))?;
        for (image_id, header) in headers {
            let (Ok(size), Ok(modified_ns), Ok(changed_ns)) = (
                i64::try_from(header.size),
                i64::try_from(header.modified_ns),
                i64::try_from(header.changed_ns),
            ) else {
                continue;
            };
            written += statement.execute(params![
                image_id,
                header.path.to_string_lossy(),
                size,
                modified_ns,
                changed_ns,
                reader,
                header.located,
                meta_json(&header.meta)?,
                now,
                header.image_key,
            ])?;
        }
    }
    if let Some(transaction) = transaction {
        transaction.commit()?;
    }
    Ok(written)
}

/// A header as the catalog keeps it: without what it does not record, and
/// without its path, which the row holds. About a kilobyte a light.
fn meta_json(meta: &FrameMeta) -> Result<String> {
    let mut value = serde_json::to_value(meta)?;
    if let Some(fields) = value.as_object_mut() {
        fields.retain(|name, field| name != "path" && !field.is_null());
    }
    Ok(serde_json::to_string(&value)?)
}

/// The rows that name no light the catalog holds: its row deleted, or its
/// id given to another light since.
fn orphans_sql(conn: &Connection) -> String {
    format!(
        "NOT EXISTS (SELECT 1 FROM acquiredimage a
                     WHERE a.Id = psf_guard_light_header.image_id
                       AND {} = psf_guard_light_header.image_key)",
        image_key_sql(conn)
    )
}

/// Whether any kept header names a light the catalog no longer holds.
pub fn has_orphans(conn: &Connection) -> bool {
    table_exists(conn)
        && conn
            .query_row(
                &format!(
                    "SELECT EXISTS (SELECT 1 FROM psf_guard_light_header WHERE {})",
                    orphans_sql(conn)
                ),
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap_or(false)
}

/// Forget the headers of lights the catalog no longer holds.
pub fn prune(conn: &Connection) -> Result<usize> {
    if !table_exists(conn) || !writable(conn) {
        return Ok(0);
    }
    Ok(conn.execute(
        &format!(
            "DELETE FROM psf_guard_light_header WHERE {}",
            orphans_sql(conn)
        ),
        [],
    )?)
}

/// Keep the header an import read for the light it just added as
/// `image_id`, with the file as it stood before the read
/// ([`FrameMeta::file_stamp`]). A header that cannot be kept is read again
/// later; the import goes on.
pub fn record_imported(conn: &Connection, image_id: i64, meta: &FrameMeta) {
    if !meta.readable {
        return;
    }
    let (Ok(image_id), Some(stamp)) = (i32::try_from(image_id), meta.file_stamp) else {
        return;
    };
    let header = KeptHeader::new(&meta.path, stamp, meta.clone());
    if let Err(error) = store(conn, [(image_id, &header)]) {
        tracing::debug!(
            "Could not keep the header of {}: {error:#}",
            meta.path.display()
        );
    }
}

fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|now| i64::try_from(now.as_nanos()).ok())
        .unwrap_or(0)
}

/// The kept headers of the lights one request reads, and the headers it had
/// to read from their files, to keep for the next.
#[derive(Debug, Default)]
pub struct LightHeaders {
    kept: HashMap<i32, KeptHeader>,
    /// What names each light as the request found it, so a header is kept
    /// for that light only.
    keys: HashMap<i32, String>,
    fresh: Vec<(i32, KeptHeader)>,
    read: usize,
}

impl LightHeaders {
    /// The kept headers of a project's lights. A catalog that cannot give
    /// them leaves every header to be read.
    pub fn load(conn: &Connection, project_id: i32) -> Self {
        let kept = project_headers(conn, project_id).unwrap_or_else(|error| {
            tracing::debug!("light headers of project {project_id} unavailable: {error:#}");
            BTreeMap::new()
        });
        let keys = project_keys(conn, project_id).unwrap_or_else(|error| {
            tracing::debug!("light names of project {project_id} unavailable: {error:#}");
            HashMap::new()
        });
        Self {
            kept: kept.into_iter().collect(),
            keys,
            ..Self::default()
        }
    }

    /// Headers as if the catalog kept these, for tests elsewhere.
    #[cfg(test)]
    pub fn with_kept(kept: impl IntoIterator<Item = (i32, KeptHeader)>) -> Self {
        Self {
            kept: kept.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Where a stack's own lookup last found this light. The caller checks
    /// the file still stands as it was with [`Self::holds`].
    pub fn located(&self, image_id: i32) -> Option<&Path> {
        self.kept
            .get(&image_id)
            .filter(|kept| kept.located)
            .map(|kept| kept.path.as_path())
    }

    /// Whether the kept header of this light is that of the file at `path`
    /// as `stamp` finds it.
    pub fn holds(&self, image_id: i32, path: &Path, stamp: Option<FileStamp>) -> bool {
        self.kept
            .get(&image_id)
            .is_some_and(|kept| kept.holds(path, stamp))
    }

    /// The light's header: the kept one while it holds the file at `path`,
    /// else read from the file and kept. `located` says a stack's own
    /// lookup found the file at `path`.
    pub fn meta(
        &mut self,
        image_id: i32,
        path: &Path,
        stamp: Option<FileStamp>,
        located: bool,
    ) -> FrameMeta {
        if let Some(kept) = self.kept.get(&image_id)
            && kept.holds(path, stamp)
        {
            if located && !kept.located {
                self.fresh.push((
                    image_id,
                    KeptHeader {
                        located,
                        ..kept.clone()
                    },
                ));
            }
            return kept.meta.clone();
        }
        let meta = crate::commands::import::headers::read_frame_meta(path);
        self.read += 1;
        if meta.readable
            && let Some(stamp) = stamp
            && let Some(key) = self.keys.get(&image_id)
        {
            self.fresh.push((
                image_id,
                KeptHeader {
                    located,
                    image_key: Some(key.clone()),
                    ..KeptHeader::new(path, stamp, meta.clone())
                },
            ));
        }
        meta
    }

    /// How many headers were read from their files.
    pub fn read(&self) -> usize {
        self.read
    }

    /// The headers to keep: those read from their files, and those a
    /// stack's lookup located for the first time.
    pub fn take_fresh(&mut self) -> Vec<(i32, KeptHeader)> {
        std::mem::take(&mut self.fresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        for (id, project) in [(1, 1), (2, 1), (3, 2)] {
            conn.execute(
                "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
                     gradingStatus, metadata, guid)
                 VALUES (?1, ?2, 1, 0, 'Ha', 0, '{}', ?3)",
                params![id, project, format!("light-{id}")],
            )
            .unwrap();
        }
        conn
    }

    /// A tiny light naming `object`.
    fn light(dir: &Path, name: &str, object: &str) -> PathBuf {
        let mut bytes = Vec::new();
        for card in [
            "SIMPLE  =                    T",
            "BITPIX  =                   16",
            "NAXIS   =                    2",
            "NAXIS1  =                    1",
            "NAXIS2  =                    1",
            "IMAGETYP= 'LIGHT'",
            &format!("OBJECT  = '{object}'"),
            "END",
        ] {
            bytes.extend(format!("{card:<80}").bytes());
        }
        bytes.resize(2880, b' ');
        bytes.resize(2 * 2880, 0);
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn header_of(path: &Path) -> KeptHeader {
        KeptHeader::new(
            path,
            FileStamp::of(path).unwrap(),
            crate::commands::import::headers::read_frame_meta(path),
        )
    }

    fn set_modified(path: &Path, modified: std::time::SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    #[test]
    fn a_kept_header_stands_while_its_file_does() {
        let temp = tempfile::tempdir().unwrap();
        let conn = catalog();
        let path = light(temp.path(), "a.fits", "M31");
        let mut first = LightHeaders::load(&conn, 1);
        let stamp = FileStamp::of(&path);
        assert_eq!(
            first.meta(1, &path, stamp, true).object.as_deref(),
            Some("M31")
        );
        assert_eq!(first.read(), 1);
        let fresh = first.take_fresh();
        assert_eq!(fresh[0].1.image_key.as_deref(), Some("light-1"));
        store(&conn, fresh.iter().map(|(id, header)| (*id, header))).unwrap();

        // The file still as it was: the kept header serves, not the file.
        // One that names another object proves it.
        conn.execute(
            "UPDATE psf_guard_light_header SET meta_json = json_set(meta_json, '$.object', 'kept')",
            [],
        )
        .unwrap();
        let mut second = LightHeaders::load(&conn, 1);
        assert_eq!(second.located(1), Some(path.as_path()));
        let stamp = FileStamp::of(&path);
        assert!(second.holds(1, &path, stamp));
        assert_eq!(
            second.meta(1, &path, stamp, true).object.as_deref(),
            Some("kept")
        );
        assert_eq!(second.read(), 0);
        assert!(second.take_fresh().is_empty(), "nothing new to keep");

        // Written again: read again, and kept again.
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        set_modified(&path, modified + std::time::Duration::from_secs(5));
        let stamp = FileStamp::of(&path);
        assert!(!second.holds(1, &path, stamp));
        assert_eq!(
            second.meta(1, &path, stamp, true).object.as_deref(),
            Some("M31")
        );
        assert_eq!(second.read(), 1);
        let fresh = second.take_fresh();
        assert_eq!(fresh.len(), 1);
        store(&conn, fresh.iter().map(|(id, header)| (*id, header))).unwrap();
        let kept = project_headers(&conn, 1).unwrap();
        assert_eq!(kept[&1].meta.object.as_deref(), Some("M31"));
        assert_eq!(kept[&1].stamp(), stamp.unwrap());

        // Found somewhere else: read again.
        let moved = temp.path().join("moved.fits");
        std::fs::copy(&path, &moved).unwrap();
        assert!(!second.holds(1, &moved, FileStamp::of(&moved)));
    }

    /// A header edited in place, its size and modification time put back,
    /// is read again: the change time moved.
    #[cfg(unix)]
    #[test]
    fn a_header_edited_in_place_with_its_time_kept_is_read_again() {
        let temp = tempfile::tempdir().unwrap();
        let path = light(temp.path(), "a.fits", "M31");
        let header = header_of(&path);
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let edited = light(temp.path(), "b.fits", "M33");
        std::fs::copy(&edited, &path).unwrap();
        set_modified(&path, modified);
        let stamp = FileStamp::of(&path).unwrap();
        assert_eq!(
            (stamp.size, stamp.modified_ns),
            (header.size, header.modified_ns)
        );
        assert!(!header.holds(&path, Some(stamp)));
    }

    #[test]
    fn a_header_kept_for_a_deleted_light_is_not_another_lights() {
        // SQLite hands the largest id out again after a delete, and N.I.N.A.
        // file names repeat from one target's folder to the next.
        let temp = tempfile::tempdir().unwrap();
        let conn = catalog();
        let path = light(temp.path(), "LIGHT_0001.fits", "M31");
        store(&conn, [(2, &header_of(&path))]).unwrap();
        assert_eq!(project_headers(&conn, 1).unwrap().len(), 1);
        let reuse = |guid: Option<&str>| {
            conn.execute("DELETE FROM acquiredimage WHERE Id = 2", [])
                .unwrap();
            conn.execute(
                "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
                     gradingStatus, metadata, guid)
                 VALUES (2, 1, 1, 0, 'Ha', 0, '{\"FileName\":\"LIGHT_0001.fits\"}', ?1)",
                [guid],
            )
            .unwrap();
        };
        reuse(Some("another-light"));
        assert!(project_headers(&conn, 1).unwrap().is_empty());
        assert_eq!(project_stamp(&conn, 1), 0);
        assert!(has_orphans(&conn));
        assert_eq!(prune(&conn).unwrap(), 1);

        // Without guids, the capture time and file name tell lights apart.
        let mut stored = header_of(&path);
        stored.image_key = None;
        store(&conn, [(2, &stored)]).unwrap();
        conn.execute("UPDATE acquiredimage SET guid = NULL", [])
            .unwrap();
        // The row was kept for the light by its guid; it now has none.
        assert!(project_headers(&conn, 1).unwrap().is_empty());
        prune(&conn).unwrap();
        store(&conn, [(2, &stored)]).unwrap();
        assert_eq!(project_headers(&conn, 1).unwrap().len(), 1);
        conn.execute(
            "UPDATE acquiredimage SET acquireddate = 60 WHERE Id = 2",
            [],
        )
        .unwrap();
        assert!(
            project_headers(&conn, 1).unwrap().is_empty(),
            "another capture under the same id and name"
        );

        // A header read for one light is not kept for the next to take its id.
        let named = KeptHeader {
            image_key: Some("light-1".into()),
            ..header_of(&path)
        };
        assert_eq!(store(&conn, [(2, &named)]).unwrap(), 0);
    }

    #[test]
    fn a_kept_header_reads_back_as_it_was_read() {
        let temp = tempfile::tempdir().unwrap();
        let conn = catalog();
        let path = light(temp.path(), "a.fits", "M31");
        let mut meta = crate::commands::import::headers::read_frame_meta(&path);
        meta.rotator_position = Some(94.7);
        meta.camera_temp = Some(-10.0);
        let header = KeptHeader::new(&path, FileStamp::of(&path).unwrap(), meta);
        store(&conn, [(1, &header)]).unwrap();
        let kept = &project_headers(&conn, 1).unwrap()[&1];
        assert_eq!(
            serde_json::to_value(&kept.meta).unwrap(),
            serde_json::to_value(&header.meta).unwrap()
        );
        let stored: String = conn
            .query_row(
                "SELECT meta_json FROM psf_guard_light_header WHERE image_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !stored.contains("null") && !stored.contains("\"path\""),
            "{stored}"
        );
    }

    #[test]
    fn a_header_from_another_release_is_read_again() {
        let temp = tempfile::tempdir().unwrap();
        let conn = catalog();
        let path = light(temp.path(), "a.fits", "M31");
        store(&conn, [(1, &header_of(&path))]).unwrap();
        assert_eq!(project_headers(&conn, 1).unwrap().len(), 1);
        assert!(reader().starts_with(env!("CARGO_PKG_VERSION")));
        conn.execute("UPDATE psf_guard_light_header SET reader = '0.11.0+1'", [])
            .unwrap();
        assert!(project_headers(&conn, 1).unwrap().is_empty());
    }

    #[test]
    fn writing_unchanged_headers_keeps_their_stamp() {
        let temp = tempfile::tempdir().unwrap();
        let conn = catalog();
        assert_eq!(project_stamp(&conn, 1), 0, "no table yet");
        let path = light(temp.path(), "a.fits", "M31");
        let header = header_of(&path);
        assert_eq!(store(&conn, [(1, &header)]).unwrap(), 1);
        let stamp = project_stamp(&conn, 1);
        assert_ne!(stamp, 0);
        assert_eq!(project_stamp(&conn, 2), 0, "another project's lights");

        // The health pass judges the project again and reads the same.
        assert_eq!(store(&conn, [(1, &header)]).unwrap(), 0);
        assert_eq!(project_stamp(&conn, 1), stamp);

        // A stack locating the light marks it without moving the stamp,
        // and the pass does not unmark it.
        let located = KeptHeader {
            located: true,
            ..header.clone()
        };
        assert_eq!(store(&conn, [(1, &located)]).unwrap(), 1);
        assert_eq!(project_stamp(&conn, 1), stamp);
        assert_eq!(store(&conn, [(1, &header)]).unwrap(), 0);
        assert!(project_headers(&conn, 1).unwrap()[&1].located);

        // Another light of the project moves it.
        store(&conn, [(2, &header)]).unwrap();
        assert_ne!(project_stamp(&conn, 1), stamp);

        // A header found at another path loses the mark.
        let elsewhere = KeptHeader {
            path: temp.path().join("elsewhere.fits"),
            ..header.clone()
        };
        store(&conn, [(1, &elsewhere)]).unwrap();
        assert!(!project_headers(&conn, 1).unwrap()[&1].located);
    }

    #[test]
    fn headers_of_lights_gone_from_the_catalog_are_forgotten() {
        let temp = tempfile::tempdir().unwrap();
        let conn = catalog();
        let path = light(temp.path(), "a.fits", "M31");
        let header = header_of(&path);
        store(&conn, [(1, &header), (3, &header)]).unwrap();
        assert!(!has_orphans(&conn));
        conn.execute("DELETE FROM acquiredimage WHERE Id = 3", [])
            .unwrap();
        assert!(has_orphans(&conn));
        assert_eq!(prune(&conn).unwrap(), 1);
        assert_eq!(project_headers(&conn, 1).unwrap().len(), 1);
        // A header for a light the catalog does not hold is not kept.
        assert_eq!(store(&conn, [(3, &header)]).unwrap(), 0);
    }

    #[test]
    fn a_catalog_a_newer_build_upgraded_is_not_written() {
        let temp = tempfile::tempdir().unwrap();
        let conn = catalog();
        crate::calibration::ensure_schema(&conn).unwrap();
        conn.execute(
            "UPDATE psf_guard_calibration_schema SET version = version + 1",
            [],
        )
        .unwrap();
        let path = light(temp.path(), "a.fits", "M31");
        assert_eq!(store(&conn, [(1, &header_of(&path))]).unwrap(), 0);
        assert!(project_headers(&conn, 1).unwrap().is_empty());
    }

    #[test]
    fn frame_meta_changes_bump_the_reader_version() {
        // A field added to FrameMeta leaves every kept header without it.
        // Bump FRAME_META_VERSION so they are read again, then update this
        // list.
        let fields = serde_json::to_value(FrameMeta::default())
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            (FRAME_META_VERSION, fields.join(",")),
            (
                1,
                "airmass,aperture_mm,bayer_pattern,bayer_x_offset,bayer_y_offset,binning_x,\
                 binning_y,camera,camera_target_temp,camera_temp,channels,class,date_local,\
                 date_obs,date_obs_utc,dec_deg,exposure_s,filter,focal_length_mm,focal_ratio,\
                 focuser_position,focuser_temp,gain,height,image_type,object,offset,path,\
                 pier_side,processed,ra_deg,readable,readout_mode,readout_mode_name,\
                 rotator_position,site_latitude,site_longitude,telescope,timestamp,width"
                    .to_string()
            )
        );
    }
}
