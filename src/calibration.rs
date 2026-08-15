//! PSF Guard-owned calibration library stored beside Target Scheduler data.
//!
//! The scheduler tables remain untouched. We keep paths and provenance here;
//! raw calibration frames and generated masters stay on disk.

use crate::commands::import::headers::FrameMeta;
use anyhow::{Context, Result};
use rusqlite::types::Value;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

pub const CALIBRATION_SCHEMA_VERSION: i64 = 1;
pub const MASTER_CACHE_VERSION: u32 = 1;
const MIN_MASTER_FRAMES: usize = 2;
const MAX_MASTER_FRAMES: usize = 64;
const DARK_TEMPERATURE_TOLERANCE_C: f64 = 3.0;
/// Flats feeding one master must come from one flat session: within this
/// window of the frame that anchors the chosen coherent subset. Dust moves
/// between sessions, so a master must not mix a fresh set with one shot
/// months earlier. Bias, dark, and dark-flat libraries are stable across
/// months and get no time window — only temperature coherence.
const FLAT_SESSION_WINDOW_SECONDS: u64 = 24 * 60 * 60;
/// Sensor-temperature coherence for the frames feeding one master. Matches
/// seiza-stacking's own frame-for-frame CCD-TEMP gate (±1 °C against the
/// first frame): a looser window here would assemble sets seiza refuses,
/// reintroducing the silent build failure this selection exists to avoid.
const MASTER_TEMPERATURE_COHERENCE_C: f64 = 1.0;
const EXPOSURE_TOLERANCE_SECONDS: f64 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationKind {
    Bias,
    Dark,
    DarkFlat,
    Flat,
}

impl CalibrationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bias => "bias",
            Self::Dark => "dark",
            Self::DarkFlat => "dark_flat",
            Self::Flat => "flat",
        }
    }

    fn from_db(value: &str) -> Option<Self> {
        match value {
            "bias" => Some(Self::Bias),
            "dark" => Some(Self::Dark),
            "dark_flat" => Some(Self::DarkFlat),
            "flat" => Some(Self::Flat),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CalibrationFrame {
    pub id: i64,
    pub frame_uuid: String,
    pub rig_uuid: String,
    pub kind: CalibrationKind,
    pub source_path: PathBuf,
    pub source_fingerprint: String,
    pub captured_at: Option<i64>,
    pub telescope: Option<String>,
    pub camera: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub channels: Option<i64>,
    pub binning_x: Option<i64>,
    pub binning_y: Option<i64>,
    pub gain: Option<i64>,
    pub offset: Option<i64>,
    pub readout_mode: Option<i64>,
    pub bayer_pattern: Option<String>,
    pub exposure_s: Option<f64>,
    pub camera_temp: Option<f64>,
    pub filter: Option<String>,
    pub focal_length_mm: Option<f64>,
    /// Set after the current file (or a basename-remapped file) has been
    /// checked against this catalog row's hard settings.
    pub source_verified: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CalibrationSelection {
    pub bias: Vec<CalibrationFrame>,
    pub dark: Vec<CalibrationFrame>,
    pub dark_flat: Vec<CalibrationFrame>,
    pub flat: Vec<CalibrationFrame>,
}

impl CalibrationSelection {
    pub fn is_empty(&self) -> bool {
        self.bias.is_empty()
            && self.dark.is_empty()
            && self.dark_flat.is_empty()
            && self.flat.is_empty()
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CalibrationImportOutcome {
    pub imported: usize,
    pub updated: usize,
    pub skipped_existing: usize,
    pub bias: usize,
    pub dark: usize,
    pub dark_flat: usize,
    pub flat: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationRigSummary {
    pub rig_uuid: String,
    pub name: String,
    pub profile_id: Option<String>,
    pub telescope: Option<String>,
    pub camera: Option<String>,
    pub frame_count: usize,
    pub bias: usize,
    pub dark: usize,
    pub dark_flat: usize,
    pub flat: usize,
    pub oldest_at: Option<i64>,
    pub newest_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationLibrarySummary {
    pub schema_version: i64,
    pub frame_count: usize,
    pub master_count: usize,
    pub rigs: Vec<CalibrationRigSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationFrameSummary {
    pub frame_uuid: String,
    pub rig_uuid: String,
    pub kind: CalibrationKind,
    pub source_path: String,
    pub source_exists: bool,
    pub captured_at: Option<i64>,
    pub camera: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub binning_x: Option<i64>,
    pub binning_y: Option<i64>,
    pub gain: Option<i64>,
    pub offset: Option<i64>,
    pub readout_mode: Option<i64>,
    pub bayer_pattern: Option<String>,
    pub exposure_s: Option<f64>,
    pub camera_temp: Option<f64>,
    pub filter: Option<String>,
    pub focal_length_mm: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationLibraryDetails {
    pub summary: CalibrationLibrarySummary,
    pub frames: Vec<CalibrationFrameSummary>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CalibrationMutationOutcome {
    pub frames_removed: usize,
    pub masters_removed: usize,
}

#[derive(Debug, Clone, Default)]
pub struct CalibrationSyncCounts {
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
}

#[derive(Debug, Clone, Default)]
pub struct CalibrationSyncOutcome {
    pub rigs: CalibrationSyncCounts,
    pub rig_bindings: CalibrationSyncCounts,
    pub frames: CalibrationSyncCounts,
}

/// How a stack asks for calibration. `Auto` applies every master that can be
/// built and refuses combinations that would damage the result (a flat with
/// no bias or dark master). `On` applies whatever can be built, including the
/// combinations `Auto` refuses. `Off` stacks the raw lights.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationMode {
    #[default]
    Auto,
    On,
    Off,
}

impl CalibrationMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct AppliedCalibration {
    /// The mode the resolution ran under. Recorded so consumers that must
    /// reproduce a build — the source-frame search — resolve the same way,
    /// and so the UI can mark a stack stale when the preference changes.
    /// Artifacts recorded before this field existed were built in `Auto`.
    #[serde(default)]
    pub mode: CalibrationMode,
    pub state: String,
    pub bias_frames: usize,
    pub dark_frames: usize,
    pub dark_flat_frames: usize,
    pub flat_frames: usize,
    pub bias_master: Option<String>,
    pub dark_master: Option<String>,
    pub dark_flat_master: Option<String>,
    pub flat_master: Option<String>,
    pub warning: Option<String>,
    pub fingerprint: String,
    /// The masters that actually applied (labels or "none" per kind). The
    /// selection `fingerprint` is computed before any build; this records
    /// the build outcome, so calibration-sensitive consumers can detect a
    /// degraded or recovered build behind an identical selection.
    #[serde(default)]
    pub masters_signature: String,
}

impl Default for AppliedCalibration {
    fn default() -> Self {
        Self {
            mode: CalibrationMode::Auto,
            state: "none".into(),
            bias_frames: 0,
            dark_frames: 0,
            dark_flat_frames: 0,
            flat_frames: 0,
            bias_master: None,
            dark_master: None,
            dark_flat_master: None,
            flat_master: None,
            warning: None,
            fingerprint: "none".into(),
            masters_signature: "bias=none;dark=none;dark_flat=none;flat=none".into(),
        }
    }
}

pub fn kind_from_meta(meta: &FrameMeta) -> Option<CalibrationKind> {
    let value = meta.image_type.as_deref()?.trim().to_ascii_uppercase();
    if value.contains("DARKFLAT") || value.contains("DARK FLAT") || value.contains("FLATDARK") {
        Some(CalibrationKind::DarkFlat)
    } else if value.contains("BIAS") || value.contains("OFFSET") {
        Some(CalibrationKind::Bias)
    } else if value.contains("DARK") {
        Some(CalibrationKind::Dark)
    } else if value.contains("FLAT") {
        Some(CalibrationKind::Flat)
    } else {
        None
    }
}

pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS psf_guard_calibration_schema (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            version   INTEGER NOT NULL
        );
        INSERT INTO psf_guard_calibration_schema (singleton, version)
            VALUES (1, 1)
            ON CONFLICT(singleton) DO NOTHING;

        CREATE TABLE IF NOT EXISTS psf_guard_rig (
            rig_uuid      TEXT PRIMARY KEY,
            signature     TEXT NOT NULL UNIQUE,
            name          TEXT NOT NULL,
            profile_id    TEXT,
            telescope     TEXT,
            camera        TEXT,
            width         INTEGER,
            height        INTEGER,
            binning_x     INTEGER,
            binning_y     INTEGER,
            created_at    INTEGER NOT NULL,
            updated_at    INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS psf_guard_calibration_frame (
            id                 INTEGER PRIMARY KEY AUTOINCREMENT,
            frame_uuid         TEXT NOT NULL UNIQUE,
            rig_uuid           TEXT NOT NULL,
            kind               TEXT NOT NULL CHECK (kind IN ('bias', 'dark', 'dark_flat', 'flat')),
            source_path        TEXT NOT NULL UNIQUE,
            source_fingerprint TEXT NOT NULL,
            captured_at        INTEGER,
            image_type_raw     TEXT,
            telescope          TEXT,
            camera             TEXT,
            width              INTEGER,
            height             INTEGER,
            channels           INTEGER,
            binning_x          INTEGER,
            binning_y          INTEGER,
            gain               INTEGER,
            offset             INTEGER,
            readout_mode       INTEGER,
            bayer_pattern      TEXT,
            bayer_x_offset     INTEGER,
            bayer_y_offset     INTEGER,
            exposure_s         REAL,
            camera_temp        REAL,
            filter_name        TEXT,
            focal_length_mm    REAL,
            file_size          INTEGER,
            file_mtime_ns      TEXT,
            added_at           INTEGER NOT NULL,
            updated_at         INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_psf_guard_calibration_frame_match
            ON psf_guard_calibration_frame
               (kind, camera, width, height, binning_x, binning_y, gain, offset, readout_mode);
        CREATE INDEX IF NOT EXISTS idx_psf_guard_calibration_frame_rig
            ON psf_guard_calibration_frame(rig_uuid, kind);

        CREATE TABLE IF NOT EXISTS psf_guard_rig_binding (
            binding_uuid TEXT PRIMARY KEY,
            rig_uuid     TEXT NOT NULL,
            profile_id   TEXT NOT NULL,
            created_at   INTEGER NOT NULL,
            UNIQUE(rig_uuid, profile_id)
        );

        CREATE TABLE IF NOT EXISTS psf_guard_calibration_master (
            master_uuid         TEXT PRIMARY KEY,
            rig_uuid            TEXT NOT NULL,
            kind                TEXT NOT NULL CHECK (kind IN ('bias', 'dark', 'dark_flat', 'flat')),
            cache_path          TEXT NOT NULL UNIQUE,
            source_set_hash     TEXT NOT NULL,
            source_count        INTEGER NOT NULL,
            source_frame_uuids  TEXT NOT NULL,
            created_at          INTEGER NOT NULL,
            seiza_version       TEXT NOT NULL,
            cache_version       INTEGER NOT NULL,
            exposure_s          REAL,
            filter_name         TEXT,
            camera_temp         REAL,
            bias_master_uuid    TEXT,
            dark_master_uuid    TEXT,
            statistics_json     TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_psf_guard_calibration_master_lookup
            ON psf_guard_calibration_master(rig_uuid, kind, source_set_hash);
        "#,
    )
    .context("creating PSF Guard calibration tables")?;
    let version: i64 = conn.query_row(
        "SELECT version FROM psf_guard_calibration_schema WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    if version != CALIBRATION_SCHEMA_VERSION {
        anyhow::bail!(
            "PSF Guard calibration schema version {version} is not supported by this build \
             (expected {CALIBRATION_SCHEMA_VERSION})"
        );
    }
    Ok(())
}

pub fn schema_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master
         WHERE type = 'table' AND name = 'psf_guard_calibration_frame'",
        [],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .unwrap_or(false)
}

pub fn import_calibration_frames(
    tx: &Transaction<'_>,
    frames: &[FrameMeta],
    profile_id: Option<&str>,
) -> Result<CalibrationImportOutcome> {
    if frames.is_empty() {
        return Ok(CalibrationImportOutcome::default());
    }
    ensure_schema(tx)?;
    let now = chrono::Utc::now().timestamp();
    let mut outcome = CalibrationImportOutcome::default();

    for frame in frames {
        let Some(kind) = kind_from_meta(frame) else {
            continue;
        };
        let source_path = canonical_text(&frame.path);
        let (fingerprint, file_size, file_mtime_ns) = file_fingerprint(&frame.path);
        let signature = rig_signature(profile_id, frame);
        let rig_uuid = ensure_rig(tx, &signature, profile_id, frame, now)?;
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT frame_uuid, source_fingerprint
                 FROM psf_guard_calibration_frame WHERE source_path = ?1",
                [&source_path],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if existing
            .as_ref()
            .is_some_and(|(_, previous)| previous == &fingerprint)
        {
            outcome.skipped_existing += 1;
            count_kind(&mut outcome, kind);
            continue;
        }
        let frame_uuid = existing
            .as_ref()
            .map(|(uuid, _)| uuid.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        tx.execute(
            r#"
            INSERT INTO psf_guard_calibration_frame (
                frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
                captured_at, image_type_raw, telescope, camera, width, height,
                channels, binning_x, binning_y, gain, offset, readout_mode,
                bayer_pattern, bayer_x_offset, bayer_y_offset, exposure_s,
                camera_temp, filter_name, focal_length_mm, file_size,
                file_mtime_ns, added_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24,
                ?25, ?26, ?27, ?27
            )
            ON CONFLICT(source_path) DO UPDATE SET
                rig_uuid=excluded.rig_uuid, kind=excluded.kind,
                source_fingerprint=excluded.source_fingerprint,
                captured_at=excluded.captured_at, image_type_raw=excluded.image_type_raw,
                telescope=excluded.telescope, camera=excluded.camera,
                width=excluded.width, height=excluded.height, channels=excluded.channels,
                binning_x=excluded.binning_x, binning_y=excluded.binning_y,
                gain=excluded.gain, offset=excluded.offset,
                readout_mode=excluded.readout_mode, bayer_pattern=excluded.bayer_pattern,
                bayer_x_offset=excluded.bayer_x_offset,
                bayer_y_offset=excluded.bayer_y_offset,
                exposure_s=excluded.exposure_s, camera_temp=excluded.camera_temp,
                filter_name=excluded.filter_name, focal_length_mm=excluded.focal_length_mm,
                file_size=excluded.file_size, file_mtime_ns=excluded.file_mtime_ns,
                updated_at=excluded.updated_at
            "#,
            params![
                frame_uuid,
                rig_uuid,
                kind.as_str(),
                source_path,
                fingerprint,
                frame.timestamp,
                frame.image_type,
                frame.telescope,
                frame.camera,
                frame.width,
                frame.height,
                frame.channels,
                frame.binning_x,
                frame.binning_y,
                frame.gain,
                frame.offset,
                frame.readout_mode,
                frame.bayer_pattern,
                frame.bayer_x_offset,
                frame.bayer_y_offset,
                frame.exposure_s,
                frame.camera_temp,
                frame.filter,
                frame.focal_length_mm,
                file_size,
                file_mtime_ns,
                now,
            ],
        )?;
        if existing.is_some() {
            outcome.updated += 1;
        } else {
            outcome.imported += 1;
        }
        count_kind(&mut outcome, kind);
    }
    Ok(outcome)
}

pub fn library_summary(conn: &Connection) -> Result<CalibrationLibrarySummary> {
    if !schema_exists(conn) {
        return Ok(CalibrationLibrarySummary {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            frame_count: 0,
            master_count: 0,
            rigs: Vec::new(),
        });
    }
    let schema_version = conn
        .query_row(
            "SELECT version FROM psf_guard_calibration_schema WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or(CALIBRATION_SCHEMA_VERSION);
    let master_count = conn
        .query_row(
            "SELECT COUNT(*) FROM psf_guard_calibration_master",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0) as usize;
    let mut statement = conn.prepare(
        r#"
        SELECT r.rig_uuid, r.name, r.profile_id, r.telescope, r.camera,
               COUNT(f.id),
               SUM(CASE WHEN f.kind = 'bias' THEN 1 ELSE 0 END),
               SUM(CASE WHEN f.kind = 'dark' THEN 1 ELSE 0 END),
               SUM(CASE WHEN f.kind = 'dark_flat' THEN 1 ELSE 0 END),
               SUM(CASE WHEN f.kind = 'flat' THEN 1 ELSE 0 END),
               MIN(f.captured_at), MAX(f.captured_at)
        FROM psf_guard_rig r
        LEFT JOIN psf_guard_calibration_frame f ON f.rig_uuid = r.rig_uuid
        GROUP BY r.rig_uuid
        ORDER BY r.name COLLATE NOCASE, r.rig_uuid
        "#,
    )?;
    let rigs = statement
        .query_map([], |row| {
            Ok(CalibrationRigSummary {
                rig_uuid: row.get(0)?,
                name: row.get(1)?,
                profile_id: row.get(2)?,
                telescope: row.get(3)?,
                camera: row.get(4)?,
                frame_count: row.get::<_, i64>(5)? as usize,
                bias: row.get::<_, i64>(6)? as usize,
                dark: row.get::<_, i64>(7)? as usize,
                dark_flat: row.get::<_, i64>(8)? as usize,
                flat: row.get::<_, i64>(9)? as usize,
                oldest_at: row.get(10)?,
                newest_at: row.get(11)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(CalibrationLibrarySummary {
        schema_version,
        frame_count: rigs.iter().map(|rig| rig.frame_count).sum(),
        master_count,
        rigs,
    })
}

pub fn library_details(conn: &Connection) -> Result<CalibrationLibraryDetails> {
    let summary = library_summary(conn)?;
    if !schema_exists(conn) {
        return Ok(CalibrationLibraryDetails {
            summary,
            frames: Vec::new(),
        });
    }
    let mut statement = conn.prepare(
        r#"
        SELECT id, frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
               captured_at, telescope, camera, width, height, channels,
               binning_x, binning_y, gain, offset, readout_mode, bayer_pattern,
               exposure_s, camera_temp, filter_name, focal_length_mm
        FROM psf_guard_calibration_frame
        ORDER BY kind, captured_at DESC, source_path COLLATE NOCASE
        "#,
    )?;
    let frames = statement
        .query_map([], row_to_frame)?
        .map(|row| {
            row.map(|frame| CalibrationFrameSummary {
                frame_uuid: frame.frame_uuid,
                rig_uuid: frame.rig_uuid,
                kind: frame.kind,
                // The server checks paths after releasing the shared database
                // connection so slow storage cannot block unrelated queries.
                source_exists: false,
                source_path: frame.source_path.to_string_lossy().into_owned(),
                captured_at: frame.captured_at,
                camera: frame.camera,
                width: frame.width,
                height: frame.height,
                binning_x: frame.binning_x,
                binning_y: frame.binning_y,
                gain: frame.gain,
                offset: frame.offset,
                readout_mode: frame.readout_mode,
                bayer_pattern: frame.bayer_pattern,
                exposure_s: frame.exposure_s,
                camera_temp: frame.camera_temp,
                filter: frame.filter,
                focal_length_mm: frame.focal_length_mm,
            })
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(CalibrationLibraryDetails { summary, frames })
}

pub fn forget_frame(conn: &mut Connection, frame_uuid: &str) -> Result<CalibrationMutationOutcome> {
    if !schema_exists(conn) {
        return Ok(CalibrationMutationOutcome::default());
    }
    let transaction = conn.transaction()?;
    let frame_exists = transaction
        .query_row(
            "SELECT 1 FROM psf_guard_calibration_frame WHERE frame_uuid = ?1",
            [frame_uuid],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !frame_exists {
        return Ok(CalibrationMutationOutcome::default());
    }

    #[derive(Debug)]
    struct MasterLink {
        uuid: String,
        sources: Vec<String>,
        bias: Option<String>,
        dark: Option<String>,
    }
    let raw_links = {
        let mut statement = transaction.prepare(
            "SELECT master_uuid, source_frame_uuids, bias_master_uuid, dark_master_uuid
             FROM psf_guard_calibration_master",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let links = raw_links
        .into_iter()
        .map(|(uuid, sources_json, bias, dark)| {
            Ok(MasterLink {
                uuid,
                sources: serde_json::from_str(&sources_json)
                    .context("reading calibration master source provenance")?,
                bias,
                dark,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut invalid = links
        .iter()
        .filter(|master| master.sources.iter().any(|source| source == frame_uuid))
        .map(|master| master.uuid.clone())
        .collect::<std::collections::HashSet<_>>();
    loop {
        let before = invalid.len();
        for master in &links {
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
            break;
        }
    }
    for master_uuid in &invalid {
        transaction.execute(
            "DELETE FROM psf_guard_calibration_master WHERE master_uuid = ?1",
            [master_uuid],
        )?;
    }
    let frames_removed = transaction.execute(
        "DELETE FROM psf_guard_calibration_frame WHERE frame_uuid = ?1",
        [frame_uuid],
    )?;
    transaction.commit()?;
    Ok(CalibrationMutationOutcome {
        frames_removed,
        masters_removed: invalid.len(),
    })
}

pub fn clear_generated_masters(conn: &Connection) -> Result<CalibrationMutationOutcome> {
    if !schema_exists(conn) {
        return Ok(CalibrationMutationOutcome::default());
    }
    let masters_removed = conn.execute("DELETE FROM psf_guard_calibration_master", [])?;
    Ok(CalibrationMutationOutcome {
        frames_removed: 0,
        masters_removed,
    })
}

/// Merge PSF Guard calibration metadata into another scheduler database.
/// Generated master rows stay local because their cache paths belong to the
/// source host; the destination rebuilds them from the synced raw-frame set.
pub fn sync_library(
    source: &Connection,
    destination: &Transaction<'_>,
) -> Result<CalibrationSyncOutcome> {
    if !schema_exists(source) {
        return Ok(CalibrationSyncOutcome::default());
    }
    ensure_schema(destination)?;
    Ok(CalibrationSyncOutcome {
        rigs: sync_table_by_key(source, destination, "psf_guard_rig", "rig_uuid")?,
        rig_bindings: sync_table_by_key(
            source,
            destination,
            "psf_guard_rig_binding",
            "binding_uuid",
        )?,
        frames: sync_table_by_key(
            source,
            destination,
            "psf_guard_calibration_frame",
            "frame_uuid",
        )?,
    })
}

fn sync_table_by_key(
    source: &Connection,
    destination: &Transaction<'_>,
    table: &str,
    key: &str,
) -> Result<CalibrationSyncCounts> {
    let columns = table_column_names(source, table)?;
    let destination_columns = table_column_names(destination, table)?;
    let destination_set = destination_columns
        .iter()
        .map(|column| column.to_ascii_lowercase())
        .collect::<std::collections::HashSet<_>>();
    let columns = columns
        .into_iter()
        .filter(|column| destination_set.contains(&column.to_ascii_lowercase()))
        .filter(|column| !column.eq_ignore_ascii_case("id"))
        .collect::<Vec<_>>();
    let key_position = columns
        .iter()
        .position(|column| column.eq_ignore_ascii_case(key))
        .with_context(|| format!("{table} has no {key} column"))?;
    let quoted = columns
        .iter()
        .map(|column| format!("\"{column}\""))
        .collect::<Vec<_>>()
        .join(",");
    let mut source_statement = source.prepare(&format!("SELECT {quoted} FROM {table}"))?;
    let mut source_rows = source_statement.query([])?;
    let mut counts = CalibrationSyncCounts::default();
    while let Some(row) = source_rows.next()? {
        let values = (0..columns.len())
            .map(|index| row.get::<_, Value>(index))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let Value::Text(key_value) = &values[key_position] else {
            continue;
        };
        let existing = destination
            .query_row(
                &format!("SELECT {quoted} FROM {table} WHERE \"{key}\" = ?1"),
                [key_value],
                |row| {
                    (0..columns.len())
                        .map(|index| row.get::<_, Value>(index))
                        .collect::<rusqlite::Result<Vec<_>>>()
                },
            )
            .optional()?;
        if existing.as_ref().is_some_and(|current| current == &values) {
            counts.unchanged += 1;
            continue;
        }
        if table == "psf_guard_calibration_frame"
            && let Some(path_position) = columns
                .iter()
                .position(|column| column.eq_ignore_ascii_case("source_path"))
        {
            destination.execute(
                "DELETE FROM psf_guard_calibration_frame
                 WHERE source_path = ?1 AND frame_uuid <> ?2",
                params![values[path_position], key_value],
            )?;
        }
        if existing.is_some() {
            let update_columns = columns
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != key_position)
                .map(|(_, column)| column)
                .collect::<Vec<_>>();
            let assignments = update_columns
                .iter()
                .enumerate()
                .map(|(index, column)| format!("\"{column}\" = ?{}", index + 1))
                .collect::<Vec<_>>()
                .join(",");
            let update_values = values
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != key_position)
                .map(|(_, value)| value as &dyn rusqlite::ToSql);
            destination.execute(
                &format!(
                    "UPDATE {table} SET {assignments} WHERE \"{key}\" = ?{}",
                    update_columns.len() + 1
                ),
                rusqlite::params_from_iter(
                    update_values.chain(std::iter::once(key_value as &dyn rusqlite::ToSql)),
                ),
            )?;
            counts.updated += 1;
        } else {
            let placeholders = (1..=values.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(",");
            destination.execute(
                &format!("INSERT INTO {table} ({quoted}) VALUES ({placeholders})"),
                rusqlite::params_from_iter(
                    values.iter().map(|value| value as &dyn rusqlite::ToSql),
                ),
            )?;
            counts.inserted += 1;
        }
    }
    Ok(counts)
}

fn table_column_names(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info('{table}')"))?;
    Ok(statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn select_for_light(conn: &Connection, light: &FrameMeta) -> Result<CalibrationSelection> {
    if !schema_exists(conn) {
        return Ok(CalibrationSelection::default());
    }
    let mut statement = conn.prepare(
        r#"
        SELECT id, frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
               captured_at, telescope, camera, width, height, channels,
               binning_x, binning_y, gain, offset, readout_mode, bayer_pattern,
               exposure_s, camera_temp, filter_name, focal_length_mm
        FROM psf_guard_calibration_frame
        ORDER BY captured_at DESC, id DESC
        "#,
    )?;
    let frames = statement
        .query_map([], row_to_frame)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut selected = CalibrationSelection::default();
    for candidate in frames {
        if !sensor_matches(light, &candidate) {
            continue;
        }
        let matches = match candidate.kind {
            CalibrationKind::Bias => true,
            CalibrationKind::Dark => {
                exposure_matches(light.exposure_s, candidate.exposure_s)
                    && temperature_matches(light.camera_temp, candidate.camera_temp)
            }
            CalibrationKind::DarkFlat => false,
            CalibrationKind::Flat => flat_matches(light, &candidate),
        };
        if matches {
            match candidate.kind {
                CalibrationKind::Bias => selected.bias.push(candidate),
                CalibrationKind::Dark => selected.dark.push(candidate),
                CalibrationKind::Flat => selected.flat.push(candidate),
                CalibrationKind::DarkFlat => {}
            }
        }
    }

    sort_candidates(&mut selected.bias, light.timestamp);
    sort_candidates(&mut selected.dark, light.timestamp);
    sort_candidates(&mut selected.flat, light.timestamp);
    if let Some(flat) = selected.flat.first() {
        for candidate in query_kind(conn, CalibrationKind::DarkFlat)? {
            if frame_pair_matches(flat, &candidate)
                && exposure_matches(flat.exposure_s, candidate.exposure_s)
                && temperature_matches(flat.camera_temp, candidate.camera_temp)
            {
                selected.dark_flat.push(candidate);
            }
        }
    }
    sort_candidates(&mut selected.dark_flat, light.timestamp);
    Ok(selected)
}

pub fn selection_fingerprint(
    conn: &Connection,
    light_path: &Path,
    directory_tree: Option<&crate::directory_tree::DirectoryTree>,
) -> Result<String> {
    let light = crate::commands::import::headers::read_frame_meta(light_path);
    let mut selected = select_for_light(conn, &light)?;
    remap_missing_sources(&mut selected, directory_tree);
    Ok(selection_hash(&selected))
}

/// Match a light against the calibration library and build whatever masters
/// it needs. `cancel` is read between masters, so a caller that stops an
/// interactive job does not wait for the whole set. Cancelling leaves the
/// masters already written in place — they are complete and cached.
/// A stop between masters. Building one master is a single Seiza call over up
/// to 64 frames, so this is the finest granularity PSF Guard can offer on its
/// own; seiza#107 adds the per-frame check inside a master.
fn stop_requested(cancel: Option<&AtomicBool>) -> Result<()> {
    match cancel {
        Some(flag) if flag.load(Ordering::Relaxed) => {
            anyhow::bail!("calibration stopped before the masters were built")
        }
        _ => Ok(()),
    }
}

/// The resolution returned when calibration is switched off: no matching, no
/// masters, and a fingerprint that cannot collide with a real selection.
fn calibration_off() -> (seiza_stacking::CalibrationMasters, AppliedCalibration) {
    (
        seiza_stacking::CalibrationMasters::default(),
        AppliedCalibration {
            mode: CalibrationMode::Off,
            state: "off".into(),
            fingerprint: "off".into(),
            ..Default::default()
        },
    )
}

pub fn resolve_or_build_masters(
    conn: &Connection,
    cache_root: &Path,
    light_path: &Path,
    directory_tree: Option<&crate::directory_tree::DirectoryTree>,
    cancel: Option<&AtomicBool>,
    mode: CalibrationMode,
) -> Result<(seiza_stacking::CalibrationMasters, AppliedCalibration)> {
    if mode == CalibrationMode::Off {
        return Ok(calibration_off());
    }
    let light = crate::commands::import::headers::read_frame_meta(light_path);
    let mut selected = select_for_light(conn, &light)?;
    let missing_sources = remap_missing_sources(&mut selected, directory_tree);
    let fingerprint = selection_hash(&selected);
    let mut applied = AppliedCalibration {
        mode,
        state: "matching".into(),
        bias_frames: selected.bias.len(),
        dark_frames: selected.dark.len(),
        dark_flat_frames: selected.dark_flat.len(),
        flat_frames: selected.flat.len(),
        fingerprint,
        ..Default::default()
    };
    if missing_sources > 0 {
        applied.warning = Some(format!(
            "{missing_sources} matched calibration frame(s) could not be found"
        ));
    }
    if selected.is_empty() {
        if missing_sources > 0 {
            applied.state = "incomplete".into();
            applied.warning = Some(format!(
                "{missing_sources} matched calibration frame(s) could not be found in the configured image folders"
            ));
        } else {
            applied.state = "none".into();
        }
        return Ok((seiza_stacking::CalibrationMasters::default(), applied));
    }

    ensure_schema(conn)?;
    let master_root = cache_root.join("calibration-masters");
    std::fs::create_dir_all(&master_root)
        .with_context(|| format!("creating {}", master_root.display()))?;

    // A failed master build must not kill the stack: the frames can still
    // integrate without that master, degraded but useful. Each failure is
    // recorded on the applied-calibration warning so the UI says exactly
    // what was skipped and why. A master whose declared DEPENDENCY failed
    // is skipped rather than built without it — a flat built without its
    // failed bias bakes the bias pedestal into the flat's normalization
    // and actively miscorrects every light, which is worse than a missing
    // master. Cancellation stays fatal via `stop_requested` between
    // builds.
    let mut build_failures: Vec<(CalibrationKind, String)> = Vec::new();
    let build_or_warn = |kind: CalibrationKind,
                         frames: &[CalibrationFrame],
                         inputs: MasterInputs<'_>,
                         skip_because: Option<&str>,
                         failures: &mut Vec<(CalibrationKind, String)>|
     -> Option<BuiltMaster> {
        if frames.is_empty() {
            return None;
        }
        if let Some(failed_dependency) = skip_because {
            let reason = format!("skipped because the {failed_dependency} master failed to build");
            tracing::warn!("{} master {reason}", kind.as_str());
            failures.push((kind, reason));
            return None;
        }
        match build_master(conn, &master_root, kind, frames, inputs) {
            Ok(master) => master,
            Err(error) => {
                tracing::warn!(
                    "{} master build failed; stacking without it: {error:#}",
                    kind.as_str()
                );
                failures.push((kind, format!("{error:#}")));
                None
            }
        }
    };

    stop_requested(cancel)?;
    let bias = build_or_warn(
        CalibrationKind::Bias,
        &selected.bias,
        MasterInputs::default(),
        None,
        &mut build_failures,
    );
    applied.bias_master = bias.as_ref().map(|master| master.label());
    let bias_failed = !selected.bias.is_empty()
        && bias.is_none()
        && build_failures
            .iter()
            .any(|(kind, _)| *kind == CalibrationKind::Bias);

    let bias_image = bias
        .as_ref()
        .map(|master| crate::image_io::open_linear_frame(&master.path).map(|frame| frame.image))
        .transpose()
        .context("loading the cached master bias")?;
    stop_requested(cancel)?;
    let dark_flat = build_or_warn(
        CalibrationKind::DarkFlat,
        &selected.dark_flat,
        MasterInputs {
            bias: bias_image.clone(),
            bias_dependency: bias.as_ref(),
            ..Default::default()
        },
        bias_failed.then_some("bias"),
        &mut build_failures,
    );
    let dark_flat_failed = !selected.dark_flat.is_empty()
        && dark_flat.is_none()
        && build_failures
            .iter()
            .any(|(kind, _)| *kind == CalibrationKind::DarkFlat);
    applied.dark_flat_master = dark_flat.as_ref().map(|master| master.label());
    let flat_dark = dark_flat
        .as_ref()
        .map(|master| {
            crate::image_io::open_linear_frame(&master.path).and_then(|frame| {
                seiza_stacking::MasterDark::from_fits_frame(
                    frame,
                    selected
                        .dark_flat
                        .first()
                        .and_then(|value| value.exposure_s),
                )
            })
        })
        .transpose()
        .context("preparing the master dark-flat")?;
    stop_requested(cancel)?;
    let dark = build_or_warn(
        CalibrationKind::Dark,
        &selected.dark,
        MasterInputs {
            bias: bias_image.clone(),
            bias_dependency: bias.as_ref(),
            ..Default::default()
        },
        bias_failed.then_some("bias"),
        &mut build_failures,
    );
    applied.dark_master = dark.as_ref().map(|master| master.label());
    stop_requested(cancel)?;
    let flat = build_or_warn(
        CalibrationKind::Flat,
        &selected.flat,
        MasterInputs {
            bias: bias_image,
            dark: flat_dark,
            bias_dependency: bias.as_ref(),
            dark_dependency: dark_flat.as_ref(),
        },
        if bias_failed {
            Some("bias")
        } else if dark_flat_failed {
            Some("dark-flat")
        } else {
            None
        },
        &mut build_failures,
    );
    // A flat can only be DIVIDED into a light whose pedestal has been
    // removed. With neither a bias nor a dark master, the division
    // amplifies the light's uncorrected pedestal by 1/vignette at the
    // frame edges — the stack comes out with the vignette inverted
    // (bright edges), which is worse than no flat at all. The flat master
    // itself stays cached: once bias or dark frames are imported, the
    // dependency-aware hash rebuilds and applies it properly.
    let mut forced_flat_note = None;
    let flat = if flat.is_some() && bias.is_none() && dark.is_none() {
        if mode == CalibrationMode::On {
            // The caller forced calibration on, accepting the damage Auto
            // refuses. Say what to expect so a bright-edged result is not
            // mistaken for a broken flat.
            forced_flat_note = Some(
                "Flat applied without a bias or dark master because calibration is forced on; \
                 vignetted edges may brighten (the light's pedestal is amplified by the flat \
                 correction)",
            );
            flat
        } else {
            let reason = "skipped: dividing a flat into a light with no bias or dark master \
                      brightens vignetted edges (the light's pedestal is amplified by \
                      the flat correction); import bias or dark frames for this camera, \
                      or force calibration on for this stack";
            tracing::warn!("flat master {reason}");
            build_failures.push((CalibrationKind::Flat, reason.into()));
            None
        }
    } else {
        flat
    };
    applied.flat_master = flat.as_ref().map(|master| master.label());

    let masters = seiza_stacking::CalibrationMasters::from_fits_paths(
        bias.as_ref().map(|value| value.path.as_path()),
        dark.as_ref().map(|value| value.path.as_path()),
        flat.as_ref().map(|value| value.path.as_path()),
        light.exposure_s,
    )
    .context("loading matched calibration masters")?;
    applied.state = if masters.is_empty() {
        "incomplete".into()
    } else {
        "applied".into()
    };
    // Kinds whose build FAILED (or were skipped on a failed dependency)
    // carry their own message below; the quietly-skipped kinds — matched
    // frames, but not enough coherent ones to build — are listed here,
    // whatever else happened. Exact kind comparison: a "dark_flat" failure
    // must not hide a quietly-skipped "dark".
    let failed_kind =
        |kind: CalibrationKind| build_failures.iter().any(|(failed, _)| *failed == kind);
    let mut missing = Vec::new();
    if !selected.bias.is_empty() && bias.is_none() && !failed_kind(CalibrationKind::Bias) {
        missing.push("bias");
    }
    if !selected.dark.is_empty() && dark.is_none() && !failed_kind(CalibrationKind::Dark) {
        missing.push("dark");
    }
    if !selected.flat.is_empty() && flat.is_none() && !failed_kind(CalibrationKind::Flat) {
        missing.push("flat");
    }
    if !missing.is_empty() {
        let partial = format!(
            "{} lacked enough matching coherent frames (each master needs at least {MIN_MASTER_FRAMES})",
            missing.join(", ")
        );
        applied.warning = Some(match applied.warning.take() {
            Some(previous) => format!("{previous}. {partial}"),
            None => partial,
        });
    }
    if !build_failures.is_empty() {
        let failed = format!(
            "Stacked without masters that failed to build — {}",
            build_failures
                .iter()
                .map(|(kind, reason)| format!("{}: {reason}", kind.as_str()))
                .collect::<Vec<_>>()
                .join("; ")
        );
        applied.warning = Some(match applied.warning.take() {
            Some(previous) => format!("{previous}. {failed}"),
            None => failed,
        });
    }
    if let Some(note) = forced_flat_note {
        applied.warning = Some(match applied.warning.take() {
            Some(previous) => format!("{previous}. {note}"),
            None => note.into(),
        });
    }
    applied.masters_signature = masters_signature(&applied);
    Ok((masters, applied))
}

/// The masters a resolution actually applied, as one comparable string.
/// The selection fingerprint is computed before any build and cannot see a
/// failed or skipped build, so consumers that must not mix calibrations —
/// the stack resume checkpoint and the source-frame search — compare this
/// too.
fn masters_signature(applied: &AppliedCalibration) -> String {
    let label = |master: &Option<String>| master.as_deref().unwrap_or("none").to_string();
    format!(
        "bias={};dark={};dark_flat={};flat={}",
        label(&applied.bias_master),
        label(&applied.dark_master),
        label(&applied.dark_flat_master),
        label(&applied.flat_master),
    )
}

pub fn resolve_or_build_masters_for_group(
    conn: &Connection,
    cache_root: &Path,
    light_paths: &[PathBuf],
    directory_tree: Option<&crate::directory_tree::DirectoryTree>,
    cancel: Option<&AtomicBool>,
    mode: CalibrationMode,
) -> Result<(seiza_stacking::CalibrationMasters, AppliedCalibration)> {
    if mode == CalibrationMode::Off {
        return Ok(calibration_off());
    }
    let Some(reference) = light_paths.first() else {
        return Ok((
            seiza_stacking::CalibrationMasters::default(),
            AppliedCalibration::default(),
        ));
    };
    let fingerprints = light_paths
        .iter()
        .map(|path| selection_fingerprint(conn, path, directory_tree))
        .collect::<Result<Vec<_>>>()?;
    if fingerprints
        .iter()
        .skip(1)
        .any(|fingerprint| fingerprint != &fingerprints[0])
    {
        return Ok((
            seiza_stacking::CalibrationMasters::default(),
            AppliedCalibration {
                mode,
                state: "incomplete".into(),
                warning: Some(
                    "Stack inputs need different calibration sets; this preview was left uncalibrated"
                        .into(),
                ),
                fingerprint: hex_digest(&fingerprints.join("\u{1e}")),
                ..Default::default()
            },
        ));
    }
    resolve_or_build_masters(conn, cache_root, reference, directory_tree, cancel, mode)
}

struct BuiltMaster {
    path: PathBuf,
    master_uuid: String,
}

impl BuiltMaster {
    fn label(&self) -> String {
        self.path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }
}

#[derive(Default)]
struct MasterInputs<'a> {
    bias: Option<seiza_stacking::LinearImage>,
    dark: Option<seiza_stacking::MasterDark>,
    bias_dependency: Option<&'a BuiltMaster>,
    dark_dependency: Option<&'a BuiltMaster>,
}

fn build_master(
    conn: &Connection,
    root: &Path,
    kind: CalibrationKind,
    frames: &[CalibrationFrame],
    inputs: MasterInputs<'_>,
) -> Result<Option<BuiltMaster>> {
    // Reduce to the frames that can actually combine (one temperature, one
    // flat session). The master's content hash below covers exactly this
    // subset, so a subset change re-keys the cache.
    let frames = coherent_master_subset(kind, frames);
    let frames = frames.as_slice();
    if frames.len() < MIN_MASTER_FRAMES {
        return Ok(None);
    }
    let MasterInputs {
        bias,
        dark,
        bias_dependency,
        dark_dependency,
    } = inputs;
    let source_hash = source_set_hash(frames, bias_dependency, dark_dependency);
    let master_uuid = stable_uuid(&format!("{}:{source_hash}", kind.as_str()));
    let path = root.join(format!("{}-{source_hash}.fits", kind.as_str()));
    if path.exists() {
        let expected_kind = match kind {
            CalibrationKind::Bias => "BIAS",
            CalibrationKind::Dark | CalibrationKind::DarkFlat => "DARK",
            CalibrationKind::Flat => "FLAT",
        };
        let row_exists = conn
            .query_row(
                "SELECT 1 FROM psf_guard_calibration_master WHERE cache_path = ?1",
                [path.to_string_lossy().as_ref()],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        let file_valid = crate::image_io::open_linear_frame(&path)
            .and_then(|frame| frame.validate_master_kind(expected_kind))
            .is_ok();
        if row_exists && file_valid {
            return Ok(Some(BuiltMaster { path, master_uuid }));
        }
        std::fs::remove_file(&path)
            .with_context(|| format!("removing stale master {}", path.display()))?;
    }
    let seiza_kind = match kind {
        CalibrationKind::Bias => seiza_stacking::MasterFrameKind::Bias,
        CalibrationKind::Dark | CalibrationKind::DarkFlat => seiza_stacking::MasterFrameKind::Dark,
        CalibrationKind::Flat => seiza_stacking::MasterFrameKind::Flat,
    };
    let options = seiza_stacking::MasterBuildOptions {
        exposure_seconds: frames.first().and_then(|frame| frame.exposure_s),
        bias,
        dark,
        ..Default::default()
    };
    let paths = frames
        .iter()
        .map(|frame| frame.source_path.clone())
        .collect::<Vec<_>>();
    let frame = seiza_stacking::build_master_from_fits(&paths, seiza_kind, &options)
        .with_context(|| format!("building master {}", kind.as_str()))?;
    if !path.exists() {
        let temporary = path.with_extension(format!("fits.tmp-{}", std::process::id()));
        seiza_stacking::write_master_fits_f32(&temporary, &frame)
            .with_context(|| format!("writing {}", temporary.display()))?;
        std::fs::rename(&temporary, &path)
            .with_context(|| format!("publishing {}", path.display()))?;
    }
    record_master(
        conn,
        kind,
        frames,
        &source_hash,
        &path,
        &frame,
        MasterInputs {
            bias: None,
            dark: None,
            bias_dependency,
            dark_dependency,
        },
    )?;
    Ok(Some(BuiltMaster { path, master_uuid }))
}

fn record_master(
    conn: &Connection,
    kind: CalibrationKind,
    frames: &[CalibrationFrame],
    source_hash: &str,
    path: &Path,
    master: &seiza_stacking::MasterFrame,
    inputs: MasterInputs<'_>,
) -> Result<()> {
    let master_kind = kind.as_str();
    let master_uuid = stable_uuid(&format!("{}:{source_hash}", kind.as_str()));
    let frame_uuids = serde_json::to_string(
        &frames
            .iter()
            .map(|frame| &frame.frame_uuid)
            .collect::<Vec<_>>(),
    )?;
    let stats = serde_json::json!({
        "accepted_samples": master.accepted_samples,
        "rejected_samples": master.rejected_samples,
        "fallback_pixels": master.fallback_pixels,
    });
    conn.execute(
        r#"
        INSERT INTO psf_guard_calibration_master (
            master_uuid, rig_uuid, kind, cache_path, source_set_hash,
            source_count, source_frame_uuids, created_at, seiza_version,
            cache_version, exposure_s, filter_name, camera_temp,
            bias_master_uuid, dark_master_uuid, statistics_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
        ON CONFLICT(master_uuid) DO UPDATE SET
            cache_path=excluded.cache_path,
            source_count=excluded.source_count,
            source_frame_uuids=excluded.source_frame_uuids,
            created_at=excluded.created_at,
            seiza_version=excluded.seiza_version,
            cache_version=excluded.cache_version,
            bias_master_uuid=excluded.bias_master_uuid,
            dark_master_uuid=excluded.dark_master_uuid,
            statistics_json=excluded.statistics_json
        ON CONFLICT(cache_path) DO UPDATE SET
            master_uuid=excluded.master_uuid,
            source_count=excluded.source_count,
            source_frame_uuids=excluded.source_frame_uuids,
            created_at=excluded.created_at,
            seiza_version=excluded.seiza_version,
            cache_version=excluded.cache_version,
            bias_master_uuid=excluded.bias_master_uuid,
            dark_master_uuid=excluded.dark_master_uuid,
            statistics_json=excluded.statistics_json
        "#,
        params![
            master_uuid,
            frames[0].rig_uuid,
            master_kind,
            path.to_string_lossy(),
            source_hash,
            frames.len() as i64,
            frame_uuids,
            chrono::Utc::now().timestamp(),
            crate::server::stack_preview::SEIZA_STACKING_VERSION,
            MASTER_CACHE_VERSION,
            master.exposure_seconds,
            frames[0].filter,
            frames[0].camera_temp,
            inputs
                .bias_dependency
                .map(|value| value.master_uuid.as_str()),
            inputs
                .dark_dependency
                .map(|value| value.master_uuid.as_str()),
            stats.to_string(),
        ],
    )?;
    Ok(())
}

fn ensure_rig(
    conn: &Connection,
    signature: &str,
    profile_id: Option<&str>,
    frame: &FrameMeta,
    now: i64,
) -> Result<String> {
    if let Some(uuid) = conn
        .query_row(
            "SELECT rig_uuid FROM psf_guard_rig WHERE signature = ?1",
            [signature],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        conn.execute(
            "UPDATE psf_guard_rig
             SET updated_at = ?2, profile_id = COALESCE(profile_id, ?3)
             WHERE rig_uuid = ?1",
            params![uuid, now, profile_id],
        )?;
        ensure_rig_binding(conn, &uuid, profile_id, now)?;
        return Ok(uuid);
    }
    let rig_uuid = stable_uuid(signature);
    let name = match (&frame.telescope, &frame.camera) {
        (Some(telescope), Some(camera)) => format!("{telescope} · {camera}"),
        (Some(telescope), None) => telescope.clone(),
        (None, Some(camera)) => camera.clone(),
        (None, None) => "Unidentified rig".into(),
    };
    conn.execute(
        "INSERT INTO psf_guard_rig (
            rig_uuid, signature, name, profile_id, telescope, camera,
            width, height, binning_x, binning_y, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            rig_uuid,
            signature,
            name,
            profile_id,
            frame.telescope,
            frame.camera,
            frame.width,
            frame.height,
            frame.binning_x,
            frame.binning_y,
            now,
        ],
    )?;
    ensure_rig_binding(conn, &rig_uuid, profile_id, now)?;
    Ok(rig_uuid)
}

fn ensure_rig_binding(
    conn: &Connection,
    rig_uuid: &str,
    profile_id: Option<&str>,
    now: i64,
) -> Result<()> {
    let Some(profile_id) = profile_id.filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    let binding_uuid = stable_uuid(&format!("{rig_uuid}\u{1f}{profile_id}"));
    conn.execute(
        "INSERT INTO psf_guard_rig_binding
            (binding_uuid, rig_uuid, profile_id, created_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(rig_uuid, profile_id) DO NOTHING",
        params![binding_uuid, rig_uuid, profile_id, now],
    )?;
    Ok(())
}

fn rig_signature(_profile_id: Option<&str>, frame: &FrameMeta) -> String {
    [
        frame.telescope.clone().unwrap_or_default(),
        frame.camera.clone().unwrap_or_default(),
        frame.width.unwrap_or_default().to_string(),
        frame.height.unwrap_or_default().to_string(),
        frame.binning_x.unwrap_or(1).to_string(),
        frame
            .binning_y
            .unwrap_or(frame.binning_x.unwrap_or(1))
            .to_string(),
    ]
    .join("\u{1f}")
    .to_ascii_lowercase()
}

fn stable_uuid(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

fn canonical_text(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn file_fingerprint(path: &Path) -> (String, i64, String) {
    let canonical = canonical_text(path);
    let metadata = std::fs::metadata(path).ok();
    let size = metadata
        .as_ref()
        .and_then(|value| i64::try_from(value.len()).ok())
        .unwrap_or(0);
    let mtime_ns = metadata
        .and_then(|value| value.modified().ok())
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().to_string())
        .unwrap_or_else(|| "0".into());
    let fingerprint = hex_digest(&format!("{canonical}\u{1f}{size}\u{1f}{mtime_ns}"));
    (fingerprint, size, mtime_ns)
}

fn source_set_hash(
    frames: &[CalibrationFrame],
    bias_dependency: Option<&BuiltMaster>,
    dark_dependency: Option<&BuiltMaster>,
) -> String {
    let mut values = frames
        .iter()
        .map(|frame| {
            let fingerprint = if frame.source_path.is_file() {
                file_fingerprint(&frame.source_path).0
            } else {
                frame.source_fingerprint.clone()
            };
            format!("{}:{fingerprint}", frame.frame_uuid)
        })
        .collect::<Vec<_>>();
    values.sort();
    hex_digest(&format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        MASTER_CACHE_VERSION,
        crate::server::stack_preview::SEIZA_STACKING_VERSION,
        bias_dependency
            .map(|value| value.master_uuid.as_str())
            .unwrap_or("none"),
        dark_dependency
            .map(|value| value.master_uuid.as_str())
            .unwrap_or("none"),
        values.join("\u{1e}")
    ))
}

fn selection_hash(selection: &CalibrationSelection) -> String {
    let mut values = Vec::new();
    for (kind, frames) in [
        (CalibrationKind::Bias, &selection.bias),
        (CalibrationKind::Dark, &selection.dark),
        (CalibrationKind::DarkFlat, &selection.dark_flat),
        (CalibrationKind::Flat, &selection.flat),
    ] {
        for frame in frames {
            let current_fingerprint = if frame.source_path.is_file() {
                file_fingerprint(&frame.source_path).0
            } else {
                frame.source_fingerprint.clone()
            };
            values.push(format!(
                "{}:{}:{}",
                kind.as_str(),
                frame.frame_uuid,
                current_fingerprint
            ));
        }
    }
    values.sort();
    if values.is_empty() {
        "none".into()
    } else {
        hex_digest(&values.join("\u{1e}"))
    }
}

fn hex_digest(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn count_kind(outcome: &mut CalibrationImportOutcome, kind: CalibrationKind) {
    match kind {
        CalibrationKind::Bias => outcome.bias += 1,
        CalibrationKind::Dark => outcome.dark += 1,
        CalibrationKind::DarkFlat => outcome.dark_flat += 1,
        CalibrationKind::Flat => outcome.flat += 1,
    }
}

fn sensor_matches(light: &FrameMeta, candidate: &CalibrationFrame) -> bool {
    sensor_identity_matches(
        light.camera.as_deref(),
        candidate.camera.as_deref(),
        light.width,
        candidate.width,
        light.height,
        candidate.height,
    ) && text_equal_if_known(light.camera.as_deref(), candidate.camera.as_deref())
        && equal_if_known(light.width, candidate.width)
        && equal_if_known(light.height, candidate.height)
        && equal_if_known(light.channels, candidate.channels)
        && equal_if_known(light.binning_x, candidate.binning_x)
        && equal_if_known(light.binning_y, candidate.binning_y)
        && equal_if_known(light.gain, candidate.gain)
        && equal_if_known(light.offset, candidate.offset)
        && equal_if_known(light.readout_mode, candidate.readout_mode)
        && text_equal_if_known(
            light.bayer_pattern.as_deref(),
            candidate.bayer_pattern.as_deref(),
        )
}

fn flat_matches(light: &FrameMeta, candidate: &CalibrationFrame) -> bool {
    text_equal_if_known(light.filter.as_deref(), candidate.filter.as_deref())
        && text_equal_if_known(light.telescope.as_deref(), candidate.telescope.as_deref())
        && option_near(
            light.focal_length_mm,
            candidate.focal_length_mm,
            |left, right| (left - right).abs() <= 1.0,
        )
}

fn frame_pair_matches(left: &CalibrationFrame, right: &CalibrationFrame) -> bool {
    sensor_identity_matches(
        left.camera.as_deref(),
        right.camera.as_deref(),
        left.width,
        right.width,
        left.height,
        right.height,
    ) && text_equal_if_known(left.camera.as_deref(), right.camera.as_deref())
        && equal_if_known(left.width, right.width)
        && equal_if_known(left.height, right.height)
        && equal_if_known(left.channels, right.channels)
        && equal_if_known(left.binning_x, right.binning_x)
        && equal_if_known(left.binning_y, right.binning_y)
        && equal_if_known(left.gain, right.gain)
        && equal_if_known(left.offset, right.offset)
        && equal_if_known(left.readout_mode, right.readout_mode)
        && text_equal_if_known(
            left.bayer_pattern.as_deref(),
            right.bayer_pattern.as_deref(),
        )
}

fn sensor_identity_matches(
    left_camera: Option<&str>,
    right_camera: Option<&str>,
    left_width: Option<i64>,
    right_width: Option<i64>,
    left_height: Option<i64>,
    right_height: Option<i64>,
) -> bool {
    matches!(
        (left_camera, right_camera),
        (Some(left), Some(right)) if left.trim().eq_ignore_ascii_case(right.trim())
    ) || matches!(
        (left_width, right_width, left_height, right_height),
        (Some(lw), Some(rw), Some(lh), Some(rh)) if lw == rw && lh == rh
    )
}

fn exposure_matches(left: Option<f64>, right: Option<f64>) -> bool {
    option_near(left, right, |a, b| {
        (a - b).abs() <= EXPOSURE_TOLERANCE_SECONDS
    })
}

fn temperature_matches(left: Option<f64>, right: Option<f64>) -> bool {
    option_near(left, right, |a, b| {
        (a - b).abs() <= DARK_TEMPERATURE_TOLERANCE_C
    })
}

fn equal_if_known<T: PartialEq>(left: Option<T>, right: Option<T>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        (Some(_), None) => false,
        (None, _) => true,
    }
}

fn text_equal_if_known(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.trim().eq_ignore_ascii_case(right.trim()),
        (Some(_), None) => false,
        (None, _) => true,
    }
}

fn option_near(
    left: Option<f64>,
    right: Option<f64>,
    compare: impl FnOnce(f64, f64) -> bool,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => compare(left, right),
        (Some(_), None) => false,
        (None, _) => true,
    }
}

/// The coherent subset of verified, nearest-first candidates that can
/// actually feed one master, chosen at BUILD time so it never touches the
/// per-light selection fingerprint (per-light trimming split fingerprints
/// across multi-night stack groups, which refuse mixed selections).
///
/// Each candidate in order anchors a cluster: frames within
/// [`MASTER_TEMPERATURE_COHERENCE_C`] of the anchor (unknown temperatures
/// cannot prove incoherence and join), and for flats also captured within
/// [`FLAT_SESSION_WINDOW_SECONDS`] of it. The first cluster with enough
/// frames to build wins, so a stray single flat shot near the lights does
/// not orphan a complete session from a week earlier. With no viable
/// cluster the first cluster is returned and the build skips quietly.
fn coherent_master_subset(
    kind: CalibrationKind,
    frames: &[CalibrationFrame],
) -> Vec<CalibrationFrame> {
    let coherent = |anchor: &CalibrationFrame, frame: &CalibrationFrame| -> bool {
        let temperature_ok = match (anchor.camera_temp, frame.camera_temp) {
            (Some(a), Some(b)) => (a - b).abs() <= MASTER_TEMPERATURE_COHERENCE_C,
            _ => true,
        };
        let session_ok = if kind == CalibrationKind::Flat {
            match (anchor.captured_at, frame.captured_at) {
                (Some(a), Some(b)) => a.abs_diff(b) <= FLAT_SESSION_WINDOW_SECONDS,
                _ => true,
            }
        } else {
            true
        };
        temperature_ok && session_ok
    };

    let mut first_cluster: Option<Vec<CalibrationFrame>> = None;
    for anchor in frames {
        let cluster: Vec<CalibrationFrame> = frames
            .iter()
            .filter(|frame| coherent(anchor, frame))
            .cloned()
            .collect();
        if cluster.len() >= MIN_MASTER_FRAMES {
            return cluster;
        }
        if first_cluster.is_none() {
            first_cluster = Some(cluster);
        }
    }
    first_cluster.unwrap_or_default()
}

fn sort_candidates(frames: &mut [CalibrationFrame], reference_at: Option<i64>) {
    frames.sort_by_key(|frame| match (reference_at, frame.captured_at) {
        (Some(reference), Some(captured)) => reference.abs_diff(captured),
        (Some(_), None) => u64::MAX,
        _ => 0,
    });
}

fn query_kind(conn: &Connection, kind: CalibrationKind) -> Result<Vec<CalibrationFrame>> {
    let mut statement = conn.prepare(
        r#"
        SELECT id, frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
               captured_at, telescope, camera, width, height, channels,
               binning_x, binning_y, gain, offset, readout_mode, bayer_pattern,
               exposure_s, camera_temp, filter_name, focal_length_mm
        FROM psf_guard_calibration_frame WHERE kind = ?1
        ORDER BY captured_at DESC, id DESC
        "#,
    )?;
    Ok(statement
        .query_map([kind.as_str()], row_to_frame)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn row_to_frame(row: &rusqlite::Row<'_>) -> rusqlite::Result<CalibrationFrame> {
    let raw_kind: String = row.get(3)?;
    let kind = CalibrationKind::from_db(&raw_kind).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            format!("unknown calibration kind {raw_kind}").into(),
        )
    })?;
    Ok(CalibrationFrame {
        id: row.get(0)?,
        frame_uuid: row.get(1)?,
        rig_uuid: row.get(2)?,
        kind,
        source_path: PathBuf::from(row.get::<_, String>(4)?),
        source_fingerprint: row.get(5)?,
        captured_at: row.get(6)?,
        telescope: row.get(7)?,
        camera: row.get(8)?,
        width: row.get(9)?,
        height: row.get(10)?,
        channels: row.get(11)?,
        binning_x: row.get(12)?,
        binning_y: row.get(13)?,
        gain: row.get(14)?,
        offset: row.get(15)?,
        readout_mode: row.get(16)?,
        bayer_pattern: row.get(17)?,
        exposure_s: row.get(18)?,
        camera_temp: row.get(19)?,
        filter: row.get(20)?,
        focal_length_mm: row.get(21)?,
        source_verified: false,
    })
}

pub fn export_destinations(
    conn: &Connection,
    light: &FrameMeta,
    target_name: &str,
    directory_tree: Option<&crate::directory_tree::DirectoryTree>,
    layout: crate::commands::export::ExportLayout,
) -> Result<Vec<(CalibrationKind, CalibrationFrame, PathBuf)>> {
    use crate::commands::export::ExportLayout;
    let mut selected = select_for_light(conn, light)?;
    remap_missing_sources(&mut selected, directory_tree);
    let target = crate::commands::export::sanitize_component(target_name);
    let filter =
        crate::commands::export::sanitize_component(light.filter.as_deref().unwrap_or("NONE"));
    let mut output = Vec::new();
    for frame in selected.bias {
        let name = frame
            .source_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let destination = match layout {
            ExportLayout::Standard => PathBuf::from("BIAS").join(&name),
            // Grouped by gain so WBPP does not integrate two sensors'
            // settings into one master bias.
            ExportLayout::Wbpp => PathBuf::from("bias")
                .join(gain_group(frame.gain))
                .join(&name),
        };
        output.push((CalibrationKind::Bias, frame, destination));
    }
    for frame in selected.dark {
        let name = frame
            .source_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let group = format!(
            "{}s_G{}",
            format_number(frame.exposure_s),
            frame
                .gain
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".into())
        );
        let destination = match layout {
            ExportLayout::Standard => PathBuf::from("DARK").join(&group).join(&name),
            ExportLayout::Wbpp => PathBuf::from("darks").join(&group).join(&name),
        };
        output.push((CalibrationKind::Dark, frame, destination));
    }
    for frame in selected.dark_flat {
        let name = frame
            .source_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let group = format!(
            "{}s_G{}",
            format_number(frame.exposure_s),
            frame
                .gain
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".into())
        );
        let destination = match layout {
            ExportLayout::Standard => PathBuf::from("DARKFLAT").join(&group).join(&name),
            // WBPP has no dark-flat type: a dark flat is a dark that happens
            // to match the flats' exposure, and WBPP pairs them by exposure.
            // Keeping a separate folder would mean adding it to WBPP a second
            // time, as darks.
            ExportLayout::Wbpp => PathBuf::from("darks").join(&group).join(&name),
        };
        output.push((CalibrationKind::DarkFlat, frame, destination));
    }
    for frame in selected.flat {
        let name = frame
            .source_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let destination = match layout {
            ExportLayout::Standard => PathBuf::from(&target)
                .join("FLAT")
                .join(&filter)
                .join(&name),
            // Kept under the target, because PSF Guard matched these flats to
            // that target's lights. Two targets shot on different nights can
            // need different flats for one filter, and merging them would have
            // WBPP integrate both into a single master.
            ExportLayout::Wbpp => PathBuf::from("flats")
                .join(&target)
                .join(&filter)
                .join(&name),
        };
        output.push((CalibrationKind::Flat, frame, destination));
    }
    output.sort_by(|left, right| left.2.cmp(&right.2));
    Ok(output)
}

/// The folder a bias frame's gain puts it in. An unrecorded gain gets its own
/// group rather than joining a numbered one.
fn gain_group(gain: Option<i64>) -> String {
    match gain {
        Some(gain) => format!("G{gain}"),
        None => "G-unknown".to_string(),
    }
}

fn remap_missing_sources(
    selection: &mut CalibrationSelection,
    directory_tree: Option<&crate::directory_tree::DirectoryTree>,
) -> usize {
    verify_sources(&mut selection.bias, directory_tree)
        + verify_sources(&mut selection.dark, directory_tree)
        + verify_sources(&mut selection.dark_flat, directory_tree)
        + verify_sources(&mut selection.flat, directory_tree)
}

fn verify_sources(
    frames: &mut Vec<CalibrationFrame>,
    directory_tree: Option<&crate::directory_tree::DirectoryTree>,
) -> usize {
    let mut output = Vec::with_capacity(frames.len().min(MAX_MASTER_FRAMES));
    let mut missing = 0;
    for mut frame in std::mem::take(frames) {
        if output.len() >= MAX_MASTER_FRAMES {
            break;
        }
        if calibration_file_matches(&frame, &frame.source_path) {
            frame.source_verified = true;
            output.push(frame);
            continue;
        }
        let Some(filename) = frame.source_path.file_name().and_then(|name| name.to_str()) else {
            missing += 1;
            continue;
        };
        let Some(tree) = directory_tree else {
            missing += 1;
            continue;
        };
        let Some(paths) = tree.find_file(filename) else {
            missing += 1;
            continue;
        };
        if let Some(path) = paths
            .iter()
            .find(|path| calibration_file_matches(&frame, path))
        {
            frame.source_path = path.clone();
            frame.source_verified = true;
            output.push(frame);
        } else {
            missing += 1;
        }
    }
    *frames = output;
    missing
}

fn calibration_file_matches(frame: &CalibrationFrame, path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let meta = crate::commands::import::headers::read_frame_meta(path);
    if !meta.readable || kind_from_meta(&meta) != Some(frame.kind) {
        return false;
    }
    let hard_matches = text_equal_if_known(frame.camera.as_deref(), meta.camera.as_deref())
        && equal_if_known(frame.width, meta.width)
        && equal_if_known(frame.height, meta.height)
        && equal_if_known(frame.channels, meta.channels)
        && equal_if_known(frame.binning_x, meta.binning_x)
        && equal_if_known(frame.binning_y, meta.binning_y)
        && equal_if_known(frame.gain, meta.gain)
        && equal_if_known(frame.offset, meta.offset)
        && equal_if_known(frame.readout_mode, meta.readout_mode)
        && text_equal_if_known(
            frame.bayer_pattern.as_deref(),
            meta.bayer_pattern.as_deref(),
        );
    if !hard_matches {
        return false;
    }
    match frame.kind {
        CalibrationKind::Bias => true,
        CalibrationKind::Dark | CalibrationKind::DarkFlat => {
            exposure_matches(frame.exposure_s, meta.exposure_s)
                && temperature_matches(frame.camera_temp, meta.camera_temp)
        }
        CalibrationKind::Flat => {
            text_equal_if_known(frame.filter.as_deref(), meta.filter.as_deref())
                && text_equal_if_known(frame.telescope.as_deref(), meta.telescope.as_deref())
                && option_near(
                    frame.focal_length_mm,
                    meta.focal_length_mm,
                    |left, right| (left - right).abs() <= 1.0,
                )
        }
    }
}

fn format_number(value: Option<f64>) -> String {
    value
        .map(|value| {
            if value.fract().abs() < f64::EPSILON {
                format!("{value:.0}")
            } else {
                format!("{value:.3}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string()
            }
        })
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn frame(path: &str, kind: &str) -> FrameMeta {
        FrameMeta {
            path: PathBuf::from(path),
            readable: true,
            image_type: Some(kind.into()),
            telescope: Some("Scope".into()),
            camera: Some("Camera".into()),
            width: Some(3000),
            height: Some(2000),
            channels: Some(1),
            binning_x: Some(1),
            binning_y: Some(1),
            gain: Some(100),
            offset: Some(20),
            exposure_s: Some(300.0),
            camera_temp: Some(-10.0),
            filter: Some("Ha".into()),
            ..Default::default()
        }
    }

    fn fits_card(output: &mut Vec<u8>, value: &str) {
        let mut card = value.as_bytes().to_vec();
        card.resize(80, b' ');
        output.extend(card);
    }

    fn write_test_fits(path: &Path, kind: &str, value: i16) {
        write_test_fits_with_gain(path, kind, value, 100);
    }

    fn write_test_fits_with_gain(path: &Path, kind: &str, value: i16, gain: i64) {
        let mut header = Vec::new();
        fits_card(&mut header, "SIMPLE  =                    T");
        fits_card(&mut header, "BITPIX  =                   16");
        fits_card(&mut header, "NAXIS   =                    2");
        fits_card(&mut header, "NAXIS1  =                    4");
        fits_card(&mut header, "NAXIS2  =                    4");
        fits_card(&mut header, &format!("IMAGETYP= '{kind}'"));
        fits_card(&mut header, "FILTER  = 'Ha'");
        fits_card(&mut header, "EXPTIME =                300.0");
        fits_card(&mut header, &format!("GAIN    = {gain:20}"));
        fits_card(&mut header, "OFFSET  =                   20");
        fits_card(&mut header, "XBINNING=                    1");
        fits_card(&mut header, "YBINNING=                    1");
        fits_card(&mut header, "CCD-TEMP=                -10.0");
        fits_card(&mut header, "TELESCOP= 'Scope'");
        fits_card(&mut header, "INSTRUME= 'Camera'");
        fits_card(&mut header, "END");
        header.resize(header.len().div_ceil(2880) * 2880, b' ');
        let mut payload = Vec::new();
        for _ in 0..16 {
            payload.extend(value.to_be_bytes());
        }
        payload.resize(2880, 0);
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(&header).unwrap();
        file.write_all(&payload).unwrap();
    }

    #[test]
    fn classifies_calibration_headers() {
        assert_eq!(
            kind_from_meta(&frame("/bias.fits", "BIAS FRAME")),
            Some(CalibrationKind::Bias)
        );
        assert_eq!(
            kind_from_meta(&frame("/dark-flat.fits", "DARK FLAT")),
            Some(CalibrationKind::DarkFlat)
        );
        assert_eq!(
            kind_from_meta(&frame("/flat.fits", "FLAT")),
            Some(CalibrationKind::Flat)
        );
    }

    #[test]
    fn empty_summary_does_not_create_tables() {
        let conn = Connection::open_in_memory().unwrap();
        let summary = library_summary(&conn).unwrap();
        assert_eq!(summary.frame_count, 0);
        assert!(!schema_exists(&conn));
    }

    #[test]
    fn rejects_wrong_sensor_or_dark_temperature() {
        let light = frame("/light.fits", "LIGHT");
        let candidate = CalibrationFrame {
            id: 1,
            frame_uuid: "f".into(),
            rig_uuid: "r".into(),
            kind: CalibrationKind::Dark,
            source_path: "/dark.fits".into(),
            source_fingerprint: "x".into(),
            captured_at: None,
            telescope: None,
            camera: Some("Other".into()),
            width: Some(3000),
            height: Some(2000),
            channels: Some(1),
            binning_x: Some(1),
            binning_y: Some(1),
            gain: Some(100),
            offset: Some(20),
            readout_mode: None,
            bayer_pattern: None,
            exposure_s: Some(300.0),
            camera_temp: Some(0.0),
            filter: None,
            focal_length_mm: None,
            source_verified: false,
        };
        assert!(!sensor_matches(&light, &candidate));
        assert!(!temperature_matches(
            light.camera_temp,
            candidate.camera_temp
        ));
    }

    #[test]
    fn imports_matches_and_deduplicates_calibration_frames() {
        let temp = tempfile::tempdir().unwrap();
        let dark_path = temp.path().join("dark.fits");
        std::fs::write(&dark_path, b"dark").unwrap();
        let mut dark = frame(dark_path.to_str().unwrap(), "DARK");
        dark.timestamp = Some(1_000);

        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            let first = import_calibration_frames(&tx, &[dark.clone()], Some("profile")).unwrap();
            assert_eq!(first.imported, 1);
            tx.commit().unwrap();
        }
        {
            let tx = conn.transaction().unwrap();
            let second = import_calibration_frames(&tx, &[dark], Some("profile")).unwrap();
            assert_eq!(second.skipped_existing, 1);
            tx.commit().unwrap();
        }

        let mut light = frame("/light.fits", "LIGHT");
        light.timestamp = Some(1_100);
        let selection = select_for_light(&conn, &light).unwrap();
        assert_eq!(selection.dark.len(), 1);
        assert!(selection.bias.is_empty());
        assert_eq!(library_summary(&conn).unwrap().frame_count, 1);
    }

    #[test]
    fn sync_copies_owned_metadata_but_not_cached_masters() {
        let temp = tempfile::tempdir().unwrap();
        let bias_path = temp.path().join("bias.fits");
        std::fs::write(&bias_path, b"bias").unwrap();
        let bias = frame(bias_path.to_str().unwrap(), "BIAS");

        let mut source = Connection::open_in_memory().unwrap();
        {
            let tx = source.transaction().unwrap();
            import_calibration_frames(&tx, &[bias], Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        let mut destination = Connection::open_in_memory().unwrap();
        {
            let tx = destination.transaction().unwrap();
            let outcome = sync_library(&source, &tx).unwrap();
            assert_eq!(outcome.rigs.inserted, 1);
            assert_eq!(outcome.rig_bindings.inserted, 1);
            assert_eq!(outcome.frames.inserted, 1);
            tx.commit().unwrap();
        }
        let summary = library_summary(&destination).unwrap();
        assert_eq!(summary.frame_count, 1);
        assert_eq!(summary.master_count, 0);

        source
            .execute(
                "UPDATE psf_guard_calibration_frame
                 SET source_fingerprint = 'changed'",
                [],
            )
            .unwrap();
        {
            let tx = destination.transaction().unwrap();
            let outcome = sync_library(&source, &tx).unwrap();
            assert_eq!(outcome.rigs.unchanged, 1);
            assert_eq!(outcome.rig_bindings.unchanged, 1);
            assert_eq!(outcome.frames.updated, 1);
            tx.commit().unwrap();
        }
        assert_eq!(
            destination
                .query_row(
                    "SELECT source_fingerprint
                     FROM psf_guard_calibration_frame",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "changed"
        );
    }

    #[test]
    fn master_subset_keeps_one_coherent_session() {
        let flat = |id: i64, captured_at: Option<i64>, temp: Option<f64>| CalibrationFrame {
            id,
            frame_uuid: format!("f{id}"),
            rig_uuid: "r".into(),
            kind: CalibrationKind::Flat,
            source_path: format!("/flat-{id}.fits").into(),
            source_fingerprint: "x".into(),
            captured_at,
            telescope: None,
            camera: None,
            width: Some(3000),
            height: Some(2000),
            channels: Some(1),
            binning_x: Some(1),
            binning_y: Some(1),
            gain: Some(100),
            offset: Some(20),
            readout_mode: None,
            bayer_pattern: None,
            exposure_s: Some(3.0),
            camera_temp: temp,
            filter: Some("Ha".into()),
            focal_length_mm: None,
            source_verified: false,
        };
        let day = 24 * 60 * 60;
        let now = 1_000_000_000_i64;
        let ids = |frames: &[CalibrationFrame]| -> Vec<i64> {
            frames.iter().map(|frame| frame.id).collect()
        };

        // Fresh cooled session first (nearest to the light), then an
        // uncooled set from months earlier: the old set must not join.
        let mixed = vec![
            flat(1, Some(now), Some(-10.2)),
            flat(2, Some(now - 600), Some(-10.0)),
            flat(3, Some(now - 80 * day), Some(39.5)),
            flat(4, Some(now - 80 * day + 60), Some(39.4)),
        ];
        assert_eq!(
            ids(&coherent_master_subset(CalibrationKind::Flat, &mixed)),
            vec![1, 2],
            "the old uncooled session must not feed the master"
        );

        // A stray single flat near the lights must not orphan the complete
        // session from a week earlier: the stray's cluster has one frame,
        // so the next anchor's full session wins.
        let stray = vec![
            flat(1, Some(now), Some(-10.0)),
            flat(2, Some(now - 7 * day), Some(-10.1)),
            flat(3, Some(now - 7 * day + 120), Some(-10.0)),
            flat(4, Some(now - 7 * day + 240), Some(-10.2)),
        ];
        let subset = coherent_master_subset(CalibrationKind::Flat, &stray);
        assert_eq!(ids(&subset), vec![2, 3, 4], "the full session wins");

        // Temperature coherence matches seiza's ±1 °C gate, not the ±3 °C
        // light-matching tolerance: a cooler settling 2 °C splits the set.
        let settling = vec![
            flat(1, Some(now), Some(-10.0)),
            flat(2, Some(now - 60), Some(-12.5)),
            flat(3, Some(now - 120), Some(-10.4)),
        ];
        assert_eq!(
            ids(&coherent_master_subset(CalibrationKind::Flat, &settling)),
            vec![1, 3],
            "frames beyond seiza's 1 °C gate must not feed the master"
        );

        // Unknown capture time or temperature cannot prove incoherence.
        let unknown = vec![flat(1, Some(now), Some(-10.0)), flat(2, None, None)];
        assert_eq!(
            coherent_master_subset(CalibrationKind::Flat, &unknown).len(),
            2
        );

        // Bias/darks get temperature coherence but no session window: a
        // months-old same-temperature dark library still combines.
        let mut dark_library = vec![
            flat(1, Some(now), Some(-10.0)),
            flat(2, Some(now - 90 * day), Some(-10.3)),
        ];
        for frame in &mut dark_library {
            frame.kind = CalibrationKind::Dark;
        }
        assert_eq!(
            coherent_master_subset(CalibrationKind::Dark, &dark_library).len(),
            2,
            "dark libraries span months"
        );

        // No viable cluster: the nearest cluster comes back and the build
        // skips quietly below MIN_MASTER_FRAMES.
        let lone = vec![flat(1, Some(now), Some(-10.0))];
        assert_eq!(
            coherent_master_subset(CalibrationKind::Flat, &lone).len(),
            1
        );
        assert!(coherent_master_subset(CalibrationKind::Flat, &[]).is_empty());
    }

    #[test]
    fn multi_night_group_keeps_identical_selections_and_builds_one_flat_master() {
        // Two nights of lights with per-night flats more than a day apart:
        // selection must stay identical across the group (the group path
        // refuses mixed selections outright), and the master must build
        // from one coherent session rather than failing on the mix.
        let temp = tempfile::tempdir().unwrap();
        let day = 24 * 60 * 60;
        let mut calibration_meta = Vec::new();
        for (night, offset) in [(1, 0i64), (2, 3 * day)] {
            for index in 0..2 {
                let path = temp.path().join(format!("flat-n{night}-{index}.fits"));
                write_test_fits(&path, "FLAT", 1_000 + index);
                let mut meta = crate::commands::import::headers::read_frame_meta(&path);
                meta.timestamp = Some(1_000_000_000 + offset + index as i64 * 60);
                calibration_meta.push(meta);
            }
        }
        for index in 0..2 {
            let path = temp.path().join(format!("bias-{index}.fits"));
            write_test_fits(&path, "BIAS", 100 + index);
            calibration_meta.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        let light_a = temp.path().join("light-a.fits");
        let light_b = temp.path().join("light-b.fits");
        write_test_fits(&light_a, "LIGHT", 1_100);
        write_test_fits(&light_b, "LIGHT", 1_100);

        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &calibration_meta, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        let cache = temp.path().join("cache");
        let (masters, applied) = resolve_or_build_masters_for_group(
            &conn,
            &cache,
            &[light_a, light_b],
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        assert!(
            applied.flat_master.is_some(),
            "one coherent session must build: {:?}",
            applied.warning
        );
        assert!(!masters.is_empty());
        assert!(!applied.masters_signature.is_empty());
    }

    #[test]
    fn moving_the_cache_directory_re_records_masters_instead_of_erroring() {
        // The master row is keyed by master_uuid; a new cache directory
        // rebuilds the same master at a new path. That upsert used to hit
        // the UNIQUE(master_uuid) constraint and fail the build.
        let temp = tempfile::tempdir().unwrap();
        let mut calibration_meta = Vec::new();
        for (kind, stem, value) in [("BIAS", "bias", 100), ("FLAT", "flat", 1_000)] {
            for index in 0..2 {
                let path = temp.path().join(format!("{stem}-{index}.fits"));
                write_test_fits(&path, kind, value + index);
                calibration_meta.push(crate::commands::import::headers::read_frame_meta(&path));
            }
        }
        let light_path = temp.path().join("light.fits");
        write_test_fits(&light_path, "LIGHT", 1_100);

        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &calibration_meta, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        let (_, first) = resolve_or_build_masters(
            &conn,
            &temp.path().join("cache-a"),
            &light_path,
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        assert!(first.flat_master.is_some());

        let (_, second) = resolve_or_build_masters(
            &conn,
            &temp.path().join("cache-b"),
            &light_path,
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        assert!(
            second.flat_master.is_some(),
            "same masters in a new cache dir must re-record cleanly: {:?}",
            second.warning
        );
        assert!(second.warning.is_none(), "no failure: {:?}", second.warning);
    }

    #[test]
    fn flat_only_library_stacks_without_the_flat_and_says_why() {
        // Dividing a flat into a light that keeps its pedestal amplifies
        // that pedestal by 1/vignette at the edges: the stack comes out
        // with the vignette inverted (bright edges). A library holding
        // only flats must therefore stack without the flat, and the
        // warning must say what to import.
        let temp = tempfile::tempdir().unwrap();
        let mut calibration_meta = Vec::new();
        for index in 0..2 {
            let path = temp.path().join(format!("flat-{index}.fits"));
            write_test_fits(&path, "FLAT", 1_000 + index);
            calibration_meta.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        let light_path = temp.path().join("light.fits");
        write_test_fits(&light_path, "LIGHT", 1_100);

        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &calibration_meta, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        let cache = temp.path().join("cache");
        let (masters, applied) = resolve_or_build_masters(
            &conn,
            &cache,
            &light_path,
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();

        assert!(
            applied.flat_master.is_none(),
            "a flat must not divide an unsubtracted light"
        );
        assert!(masters.is_empty());
        let warning = applied.warning.as_deref().unwrap_or_default();
        assert!(
            warning.contains("brightens vignetted edges")
                && warning.contains("import bias or dark frames"),
            "warning must explain the inversion and the remedy: {warning}"
        );
    }

    #[test]
    fn forced_calibration_applies_a_flat_only_library_and_says_what_to_expect() {
        // `On` overrides the flat-only refusal: the caller accepts the
        // pedestal amplification. The flat must apply, and the warning must
        // still say what the result may look like.
        let temp = tempfile::tempdir().unwrap();
        let mut calibration_meta = Vec::new();
        for index in 0..2 {
            let path = temp.path().join(format!("flat-{index}.fits"));
            write_test_fits(&path, "FLAT", 1_000 + index);
            calibration_meta.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        let light_path = temp.path().join("light.fits");
        write_test_fits(&light_path, "LIGHT", 1_100);

        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &calibration_meta, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        let cache = temp.path().join("cache");
        let (masters, applied) =
            resolve_or_build_masters(&conn, &cache, &light_path, None, None, CalibrationMode::On)
                .unwrap();

        assert!(
            applied.flat_master.is_some(),
            "forced-on must apply the flat"
        );
        assert!(!masters.is_empty());
        assert_eq!(applied.mode, CalibrationMode::On);
        assert_eq!(applied.state, "applied");
        assert!(
            applied.masters_signature.contains(";flat=flat-"),
            "signature must record the forced flat: {}",
            applied.masters_signature
        );
        let warning = applied.warning.as_deref().unwrap_or_default();
        assert!(
            warning.contains("forced on") && warning.contains("vignetted edges may brighten"),
            "warning must say what forcing the flat can do: {warning}"
        );
    }

    #[test]
    fn calibration_off_skips_matching_and_cannot_collide_with_a_real_selection() {
        // `Off` must not read the library at all, and its fingerprint must
        // differ from both a real selection and the empty-selection "none"
        // state, so resume checkpoints and searches never mix the two.
        let temp = tempfile::tempdir().unwrap();
        let mut calibration_meta = Vec::new();
        for index in 0..2 {
            let path = temp.path().join(format!("flat-{index}.fits"));
            write_test_fits(&path, "FLAT", 1_000 + index);
            calibration_meta.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        let light_path = temp.path().join("light.fits");
        write_test_fits(&light_path, "LIGHT", 1_100);

        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &calibration_meta, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        let cache = temp.path().join("cache");
        let (masters, applied) = resolve_or_build_masters_for_group(
            &conn,
            &cache,
            std::slice::from_ref(&light_path),
            None,
            None,
            CalibrationMode::Off,
        )
        .unwrap();

        assert!(masters.is_empty());
        assert_eq!(applied.mode, CalibrationMode::Off);
        assert_eq!(applied.state, "off");
        assert_eq!(applied.fingerprint, "off");
        assert!(applied.warning.is_none());
        let (_, auto_applied) = resolve_or_build_masters(
            &conn,
            &cache,
            &light_path,
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        assert_ne!(applied.fingerprint, auto_applied.fingerprint);
    }

    #[test]
    fn failed_bias_skips_dependent_masters_instead_of_building_without_them() {
        // A flat built without its failed bias bakes the bias pedestal into
        // the flat's normalization — actively miscorrecting every light.
        // The dependents must be skipped, with the reason recorded.
        let temp = tempfile::tempdir().unwrap();
        let mut calibration_meta = Vec::new();
        for (kind, stem, value) in [("BIAS", "bias", 100), ("FLAT", "flat", 1_000)] {
            for index in 0..2 {
                let path = temp.path().join(format!("{stem}-{index}.fits"));
                write_test_fits(&path, kind, value + index);
                calibration_meta.push(crate::commands::import::headers::read_frame_meta(&path));
            }
        }
        let light_path = temp.path().join("light.fits");
        write_test_fits(&light_path, "LIGHT", 1_100);

        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &calibration_meta, Some("profile")).unwrap();
            tx.commit().unwrap();
        }

        // Rewrite one bias with an incompatible sensor temperature so the
        // bias master build fails after verification passes.
        let mut hot = Vec::new();
        fits_card(&mut hot, "SIMPLE  =                    T");
        fits_card(&mut hot, "BITPIX  =                   16");
        fits_card(&mut hot, "NAXIS   =                    2");
        fits_card(&mut hot, "NAXIS1  =                    4");
        fits_card(&mut hot, "NAXIS2  =                    4");
        fits_card(&mut hot, "IMAGETYP= 'BIAS'");
        fits_card(&mut hot, "FILTER  = 'Ha'");
        fits_card(&mut hot, "EXPTIME =                300.0");
        fits_card(&mut hot, "GAIN    =                  100");
        fits_card(&mut hot, "OFFSET  =                   20");
        fits_card(&mut hot, "XBINNING=                    1");
        fits_card(&mut hot, "YBINNING=                    1");
        fits_card(&mut hot, "CCD-TEMP=                 39.5");
        fits_card(&mut hot, "TELESCOP= 'Scope'");
        fits_card(&mut hot, "INSTRUME= 'Camera'");
        fits_card(&mut hot, "END");
        hot.resize(hot.len().div_ceil(2880) * 2880, b' ');
        let mut payload = Vec::new();
        for _ in 0..16 {
            payload.extend(101_i16.to_be_bytes());
        }
        payload.resize(2880, 0);
        hot.extend(payload);
        std::fs::write(temp.path().join("bias-1.fits"), hot).unwrap();

        let cache = temp.path().join("cache");
        let (masters, applied) = resolve_or_build_masters(
            &conn,
            &cache,
            &light_path,
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();

        assert!(applied.bias_master.is_none(), "bias build fails");
        assert!(
            applied.flat_master.is_none(),
            "the flat must not build without its failed bias"
        );
        assert!(masters.is_empty());
        let warning = applied.warning.as_deref().unwrap_or_default();
        assert!(
            warning.contains("skipped because the bias master failed"),
            "warning must say why the flat was skipped: {warning}"
        );
        assert_eq!(applied.state, "incomplete");
    }

    #[test]
    fn failed_master_build_degrades_to_a_warning_not_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let mut calibration_meta = Vec::new();
        for (kind, stem, value) in [("BIAS", "bias", 100), ("FLAT", "flat", 1_000)] {
            for index in 0..2 {
                let path = temp.path().join(format!("{stem}-{index}.fits"));
                write_test_fits(&path, kind, value + index);
                calibration_meta.push(crate::commands::import::headers::read_frame_meta(&path));
            }
        }
        let light_path = temp.path().join("light.fits");
        write_test_fits(&light_path, "LIGHT", 1_100);

        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &calibration_meta, Some("profile")).unwrap();
            tx.commit().unwrap();
        }

        // The flat file is rewritten after import with a wildly different
        // sensor temperature — headers still verify (temperature is not a
        // hard identity field), the DB rows still agree, but seiza's
        // frame-for-frame validation refuses to mix +39.5° with −10°.
        // The master build fails, and the stack must proceed without a
        // flat instead of dying with no result at all.
        let mut header = Vec::new();
        fits_card(&mut header, "SIMPLE  =                    T");
        fits_card(&mut header, "BITPIX  =                   16");
        fits_card(&mut header, "NAXIS   =                    2");
        fits_card(&mut header, "NAXIS1  =                    4");
        fits_card(&mut header, "NAXIS2  =                    4");
        fits_card(&mut header, "IMAGETYP= 'FLAT'");
        fits_card(&mut header, "FILTER  = 'Ha'");
        fits_card(&mut header, "EXPTIME =                300.0");
        fits_card(&mut header, "GAIN    =                  100");
        fits_card(&mut header, "OFFSET  =                   20");
        fits_card(&mut header, "XBINNING=                    1");
        fits_card(&mut header, "YBINNING=                    1");
        fits_card(&mut header, "CCD-TEMP=                 39.5");
        fits_card(&mut header, "TELESCOP= 'Scope'");
        fits_card(&mut header, "INSTRUME= 'Camera'");
        fits_card(&mut header, "END");
        header.resize(header.len().div_ceil(2880) * 2880, b' ');
        let mut payload = Vec::new();
        for _ in 0..16 {
            payload.extend(1_001_i16.to_be_bytes());
        }
        payload.resize(2880, 0);
        let mut hot_flat = header;
        hot_flat.extend(payload);
        std::fs::write(temp.path().join("flat-1.fits"), hot_flat).unwrap();

        let cache = temp.path().join("cache");
        let (masters, applied) = resolve_or_build_masters(
            &conn,
            &cache,
            &light_path,
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();

        assert!(applied.bias_master.is_some(), "bias still builds");
        assert!(applied.flat_master.is_none(), "flat cannot build");
        assert!(!masters.is_empty(), "the surviving masters still apply");
        let warning = applied.warning.as_deref().unwrap_or_default();
        assert!(
            warning.contains("failed to build") && warning.contains("flat"),
            "warning must name the failed master and why: {warning}"
        );
        assert_eq!(applied.state, "applied");
    }

    #[test]
    fn builds_and_reuses_a_complete_master_set() {
        let temp = tempfile::tempdir().unwrap();
        let mut calibration_meta = Vec::new();
        for (kind, stem, value) in [
            ("BIAS", "bias", 100),
            ("DARK", "dark", 120),
            ("DARKFLAT", "dark-flat", 110),
            ("FLAT", "flat", 1_000),
        ] {
            for index in 0..2 {
                let path = temp.path().join(format!("{stem}-{index}.fits"));
                write_test_fits(&path, kind, value + index);
                calibration_meta.push(crate::commands::import::headers::read_frame_meta(&path));
            }
        }
        let light_path = temp.path().join("light.fits");
        write_test_fits(&light_path, "LIGHT", 1_100);

        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &calibration_meta, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        let cache = temp.path().join("cache");
        let (masters, applied) = resolve_or_build_masters(
            &conn,
            &cache,
            &light_path,
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        assert!(!masters.is_empty());
        assert_eq!(applied.state, "applied");
        assert!(applied
            .bias_master
            .as_ref()
            .is_some_and(|name| name.starts_with("bias-") && name.ends_with(".fits")));
        assert!(applied
            .dark_master
            .as_ref()
            .is_some_and(|name| name.starts_with("dark-") && name.ends_with(".fits")));
        assert!(applied
            .dark_flat_master
            .as_ref()
            .is_some_and(|name| name.starts_with("dark_flat-") && name.ends_with(".fits")));
        assert!(applied
            .flat_master
            .as_ref()
            .is_some_and(|name| name.starts_with("flat-") && name.ends_with(".fits")));
        assert_eq!(library_summary(&conn).unwrap().master_count, 4);
        let dark_bias_dependency: Option<String> = conn
            .query_row(
                "SELECT bias_master_uuid FROM psf_guard_calibration_master
                 WHERE kind = 'dark'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(dark_bias_dependency.is_some());
        let flat_dependencies: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT bias_master_uuid, dark_master_uuid
                 FROM psf_guard_calibration_master WHERE kind = 'flat'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(flat_dependencies.0.is_some());
        assert!(flat_dependencies.1.is_some());

        let (_, reused) = resolve_or_build_masters(
            &conn,
            &cache,
            &light_path,
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        assert_eq!(reused.fingerprint, applied.fingerprint);
        assert_eq!(library_summary(&conn).unwrap().master_count, 4);

        let bias_uuid: String = conn
            .query_row(
                "SELECT frame_uuid FROM psf_guard_calibration_frame
                 WHERE kind = 'bias' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let forgotten = forget_frame(&mut conn, &bias_uuid).unwrap();
        assert_eq!(forgotten.frames_removed, 1);
        assert_eq!(forgotten.masters_removed, 4);
        assert_eq!(library_summary(&conn).unwrap().master_count, 0);
    }

    #[test]
    fn group_with_different_calibration_sets_stays_uncalibrated() {
        let temp = tempfile::tempdir().unwrap();
        let mut calibration_meta = Vec::new();
        for gain in [100, 200] {
            for index in 0..2 {
                let path = temp.path().join(format!("bias-{gain}-{index}.fits"));
                write_test_fits_with_gain(&path, "BIAS", 100 + index, gain);
                calibration_meta.push(crate::commands::import::headers::read_frame_meta(&path));
            }
        }
        let first = temp.path().join("light-100.fits");
        let second = temp.path().join("light-200.fits");
        write_test_fits_with_gain(&first, "LIGHT", 1_000, 100);
        write_test_fits_with_gain(&second, "LIGHT", 1_000, 200);

        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &calibration_meta, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        let (masters, applied) = resolve_or_build_masters_for_group(
            &conn,
            &temp.path().join("cache"),
            &[first, second],
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        assert!(masters.is_empty());
        assert_eq!(applied.state, "incomplete");
        assert!(applied
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("different calibration sets")));
    }

    #[test]
    fn stale_source_path_remaps_only_to_a_matching_calibration_file() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original/bias.fits");
        std::fs::create_dir_all(original.parent().unwrap()).unwrap();
        write_test_fits(&original, "BIAS", 100);
        let original_meta = crate::commands::import::headers::read_frame_meta(&original);
        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &[original_meta], Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        std::fs::remove_file(&original).unwrap();

        let wrong = temp.path().join("FLAT/bias.fits");
        let correct = temp.path().join("BIAS/bias.fits");
        std::fs::create_dir_all(wrong.parent().unwrap()).unwrap();
        std::fs::create_dir_all(correct.parent().unwrap()).unwrap();
        write_test_fits(&wrong, "FLAT", 1_000);
        write_test_fits(&correct, "BIAS", 100);
        let light = temp.path().join("light.fits");
        write_test_fits(&light, "LIGHT", 1_000);
        let tree = crate::directory_tree::DirectoryTree::build(temp.path()).unwrap();

        let light_meta = crate::commands::import::headers::read_frame_meta(&light);
        let mut selection = select_for_light(&conn, &light_meta).unwrap();
        assert_eq!(remap_missing_sources(&mut selection, Some(&tree)), 0);
        assert_eq!(selection.bias.len(), 1);
        assert_eq!(selection.bias[0].source_path, correct);
        assert!(selection.bias[0].source_verified);
    }
}
