//! Light headers kept in the catalog.
//!
//! Preparing a stack, judging calibration health and finding the lights the
//! library cannot calibrate all start from each light's FITS header. Read
//! from a network share a thousand at a time, those reads made one build
//! take seconds. So each header read is kept in `psf_guard_light_header`,
//! one row per light, with the path, size and modification time of the file
//! it came from. A row stands while the file at that path still has that size
//! and time and the row was written by this reader ([`FRAME_META_VERSION`]);
//! otherwise the header is read again.
//!
//! Import writes a light's row as it adds the light, the calibration health
//! pass writes the ones it reads, and stack preparation hands the ones it
//! had to read to a writer when it is done. A stack also notes that its own
//! file lookup found the light at the row's path (`located`), so the next one
//! goes straight there.
//!
//! The rows hold this machine's paths and file times. Sync and transfers
//! never carry them.

use crate::commands::import::headers::{FrameMeta, FRAME_META_VERSION};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// One row per light, by `acquiredimage.Id`. `mtime_ns` is nanoseconds since
/// 1970; `written_ns` is when the row last changed, so a view worked out
/// from a project's headers can tell when to work it out again.
pub const TABLE_SQL: &str = r#"
    CREATE TABLE IF NOT EXISTS psf_guard_light_header (
        image_id       INTEGER PRIMARY KEY,
        path           TEXT NOT NULL,
        size           INTEGER NOT NULL,
        mtime_ns       INTEGER NOT NULL,
        reader_version INTEGER NOT NULL,
        located        INTEGER NOT NULL DEFAULT 0,
        meta_json      TEXT NOT NULL,
        written_ns     INTEGER NOT NULL
    );
"#;

/// A file's size and modification time: what tells a kept header, or a
/// checked calibration file, from one that changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileStamp {
    pub size: u64,
    /// Nanoseconds since 1970.
    pub modified_ns: u64,
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
        })
    }
}

/// A light's header as it was read, and the file it was read from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeptHeader {
    pub path: PathBuf,
    pub size: u64,
    /// Nanoseconds since 1970.
    pub modified_ns: u64,
    pub meta: FrameMeta,
    /// A stack's own file lookup found the light at `path`, so a stack may
    /// go straight there while the file stands as it was.
    #[serde(default)]
    pub located: bool,
}

impl KeptHeader {
    pub fn new(path: &Path, stamp: FileStamp, meta: FrameMeta) -> Self {
        Self {
            path: path.to_path_buf(),
            size: stamp.size,
            modified_ns: stamp.modified_ns,
            meta,
            located: false,
        }
    }

    pub fn stamp(&self) -> FileStamp {
        FileStamp {
            size: self.size,
            modified_ns: self.modified_ns,
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

/// The kept headers of a project's lights, by light id. Headers another
/// reader version wrote are left out: they are read again.
pub fn project_headers(conn: &Connection, project_id: i32) -> Result<BTreeMap<i32, KeptHeader>> {
    if !table_exists(conn) {
        return Ok(BTreeMap::new());
    }
    let mut statement = conn.prepare(
        "SELECT h.image_id, h.path, h.size, h.mtime_ns, h.located, h.meta_json
         FROM psf_guard_light_header h
         JOIN acquiredimage a ON a.Id = h.image_id
         WHERE a.projectId = ?1 AND h.reader_version = ?2",
    )?;
    let rows = statement.query_map(params![project_id, FRAME_META_VERSION], |row| {
        Ok((
            row.get::<_, i32>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, bool>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    let mut headers = BTreeMap::new();
    for row in rows {
        let (image_id, path, size, modified_ns, located, meta_json) = row?;
        // A row that does not parse is read again, as a missing one is.
        let Ok(mut meta) = serde_json::from_str::<FrameMeta>(&meta_json) else {
            continue;
        };
        meta.path = PathBuf::from(&path);
        let (Ok(size), Ok(modified_ns)) = (u64::try_from(size), u64::try_from(modified_ns)) else {
            continue;
        };
        headers.insert(
            image_id,
            KeptHeader {
                path: PathBuf::from(path),
                size,
                modified_ns,
                meta,
                located,
            },
        );
    }
    Ok(headers)
}

/// When a project's kept headers last changed. A header written again as it
/// was leaves it, so what is worked out from the headers is not worked out
/// again for nothing.
pub fn project_stamp(conn: &Connection, project_id: i32) -> u128 {
    if !table_exists(conn) {
        return 0;
    }
    conn.query_row(
        "SELECT count(*), coalesce(max(h.written_ns), 0)
         FROM psf_guard_light_header h
         JOIN acquiredimage a ON a.Id = h.image_id
         WHERE a.projectId = ?1",
        [project_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )
    .map(|(count, written)| ((count as u128) << 64) | written as u128)
    .unwrap_or(0)
}

/// Keep `headers`, by light id, in one transaction unless the caller holds
/// one. A header written again as it was is left alone, and a row a stack
/// located keeps that mark while its path stays. Returns the rows written.
pub fn store<'a>(
    conn: &Connection,
    headers: impl IntoIterator<Item = (i32, &'a KeptHeader)>,
) -> Result<usize> {
    ensure_table(conn)?;
    let transaction = if conn.is_autocommit() {
        Some(conn.unchecked_transaction()?)
    } else {
        None
    };
    let now = now_ns();
    let mut written = 0;
    {
        let mut statement = conn.prepare(
            "INSERT INTO psf_guard_light_header
                (image_id, path, size, mtime_ns, reader_version, located, meta_json, written_ns)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(image_id) DO UPDATE SET
                located = CASE WHEN path = excluded.path
                               THEN max(located, excluded.located)
                               ELSE excluded.located END,
                path = excluded.path,
                size = excluded.size,
                mtime_ns = excluded.mtime_ns,
                reader_version = excluded.reader_version,
                meta_json = excluded.meta_json,
                written_ns = excluded.written_ns
             WHERE path <> excluded.path
                OR size <> excluded.size
                OR mtime_ns <> excluded.mtime_ns
                OR reader_version <> excluded.reader_version
                OR located < excluded.located",
        )?;
        for (image_id, header) in headers {
            let (Ok(size), Ok(modified_ns)) = (
                i64::try_from(header.size),
                i64::try_from(header.modified_ns),
            ) else {
                continue;
            };
            written += statement.execute(params![
                image_id,
                header.path.to_string_lossy(),
                size,
                modified_ns,
                FRAME_META_VERSION,
                header.located,
                meta_json(&header.meta)?,
                now,
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

/// Forget the headers of lights the catalog no longer holds.
pub fn prune(conn: &Connection) -> Result<usize> {
    if !table_exists(conn) {
        return Ok(0);
    }
    Ok(conn.execute(
        "DELETE FROM psf_guard_light_header
         WHERE image_id NOT IN (SELECT Id FROM acquiredimage)",
        [],
    )?)
}

/// Keep the header an import read for the light it just added as `image_id`.
pub fn record_imported(conn: &Connection, image_id: i64, meta: &FrameMeta) -> Result<()> {
    if !meta.readable {
        return Ok(());
    }
    let (Ok(image_id), Some(stamp)) = (i32::try_from(image_id), FileStamp::of(&meta.path)) else {
        return Ok(());
    };
    store(
        conn,
        [(image_id, &KeptHeader::new(&meta.path, stamp, meta.clone()))],
    )?;
    Ok(())
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
        {
            self.fresh.push((
                image_id,
                KeptHeader {
                    located,
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
                     gradingStatus, metadata)
                 VALUES (?1, ?2, 1, 0, 'Ha', 0, '{}')",
                params![id, project],
            )
            .unwrap();
        }
        conn
    }

    /// A tiny light naming `object`; two objects of one length make files
    /// of one size.
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
        store(&conn, fresh.iter().map(|(id, header)| (*id, header))).unwrap();

        // The file still as it was: the kept header serves, not the file.
        // Swap in another header of the same length and time to prove it.
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let other = light(temp.path(), "b.fits", "M33");
        std::fs::copy(&other, &path).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let mut second = LightHeaders::load(&conn, 1);
        assert_eq!(second.located(1), Some(path.as_path()));
        let stamp = FileStamp::of(&path);
        assert!(second.holds(1, &path, stamp));
        assert_eq!(
            second.meta(1, &path, stamp, true).object.as_deref(),
            Some("M31")
        );
        assert_eq!(second.read(), 0);
        assert!(second.take_fresh().is_empty(), "nothing new to keep");

        // Written again: read again, and kept again.
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified + std::time::Duration::from_secs(5))
            .unwrap();
        let stamp = FileStamp::of(&path);
        assert!(!second.holds(1, &path, stamp));
        assert_eq!(
            second.meta(1, &path, stamp, true).object.as_deref(),
            Some("M33")
        );
        assert_eq!(second.read(), 1);
        let fresh = second.take_fresh();
        assert_eq!(fresh.len(), 1);
        store(&conn, fresh.iter().map(|(id, header)| (*id, header))).unwrap();
        let kept = project_headers(&conn, 1).unwrap();
        assert_eq!(kept[&1].meta.object.as_deref(), Some("M33"));
        assert_eq!(kept[&1].stamp(), stamp.unwrap());

        // Found somewhere else: read again.
        let moved = temp.path().join("moved.fits");
        std::fs::copy(&path, &moved).unwrap();
        assert!(!second.holds(1, &moved, FileStamp::of(&moved)));
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
    fn a_header_from_another_reader_is_read_again() {
        let temp = tempfile::tempdir().unwrap();
        let conn = catalog();
        let path = light(temp.path(), "a.fits", "M31");
        let header = KeptHeader::new(
            &path,
            FileStamp::of(&path).unwrap(),
            crate::commands::import::headers::read_frame_meta(&path),
        );
        store(&conn, [(1, &header)]).unwrap();
        assert_eq!(project_headers(&conn, 1).unwrap().len(), 1);
        conn.execute(
            "UPDATE psf_guard_light_header SET reader_version = reader_version - 1",
            [],
        )
        .unwrap();
        assert!(project_headers(&conn, 1).unwrap().is_empty());
    }

    #[test]
    fn writing_unchanged_headers_keeps_their_stamp() {
        let temp = tempfile::tempdir().unwrap();
        let conn = catalog();
        assert_eq!(project_stamp(&conn, 1), 0, "no table yet");
        let path = light(temp.path(), "a.fits", "M31");
        let header = KeptHeader::new(
            &path,
            FileStamp::of(&path).unwrap(),
            crate::commands::import::headers::read_frame_meta(&path),
        );
        assert_eq!(store(&conn, [(1, &header)]).unwrap(), 1);
        let stamp = project_stamp(&conn, 1);
        assert_ne!(stamp, 0);
        assert_eq!(project_stamp(&conn, 2), 0, "another project's lights");

        // The health pass judges the project again and reads the same.
        assert_eq!(store(&conn, [(1, &header)]).unwrap(), 0);
        assert_eq!(project_stamp(&conn, 1), stamp);

        // A stack locating the light marks it; the pass does not unmark it.
        let located = KeptHeader {
            located: true,
            ..header.clone()
        };
        assert_eq!(store(&conn, [(1, &located)]).unwrap(), 1);
        assert_eq!(store(&conn, [(1, &header)]).unwrap(), 0);
        assert!(project_headers(&conn, 1).unwrap()[&1].located);

        // Another light of the project moves it.
        let changed = project_stamp(&conn, 1);
        assert_ne!(changed, stamp);
        store(&conn, [(2, &header)]).unwrap();
        assert_ne!(project_stamp(&conn, 1), changed);

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
        let header = KeptHeader::new(
            &path,
            FileStamp::of(&path).unwrap(),
            crate::commands::import::headers::read_frame_meta(&path),
        );
        store(&conn, [(1, &header), (3, &header)]).unwrap();
        conn.execute("DELETE FROM acquiredimage WHERE Id = 3", [])
            .unwrap();
        assert_eq!(prune(&conn).unwrap(), 1);
        assert_eq!(project_headers(&conn, 1).unwrap().len(), 1);
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
