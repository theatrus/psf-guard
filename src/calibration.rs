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
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

/// Shape of PSF Guard's own tables inside a scheduler catalog.
///
/// Bump this whenever those tables change, and add the step that gets an
/// older catalog here to [`MIGRATIONS`]. A catalog is upgraded in place when
/// it is opened; see [`migrate_existing`].
///
/// 1: the original calibration library.
/// 2: `psf_guard_calibration_frame.rotation`, so a flat only matches a light
///    shot at the same rotator angle.
/// 4: `psf_guard_calibration_frame.valid_direction`, so a frame can be
///    marked usable only for lights captured after it (a set shot after an
///    optics change or cleaning) or only before it. NULL keeps the old
///    behavior: usable in both directions.
/// 5: `psf_guard_calibration_frame.readout_mode_name`, the readout mode as
///    N.I.N.A. spells it. The integer column stays empty on such rigs, so
///    without the name two modes of one camera matched each other's frames.
/// 6: that name read off the files for rows that predate the column.
pub const CALIBRATION_SCHEMA_VERSION: i64 = 6;
// 2: flat masters suppress defective pixels spatially after integration.
/// Version 3: masters preserve sensor, optics, exposure and capture-time
/// metadata (seiza-stacking 0.11.1). Masters written before that carry no
/// TELESCOP or FOCALLEN, which weakens validation and later matching to
/// "nothing recorded, nothing to check" — so they rebuild once.
pub const MASTER_CACHE_VERSION: u32 = 3;
const MIN_MASTER_FRAMES: usize = 2;
const MAX_MASTER_FRAMES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
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

/// Which lights a calibration frame may serve, relative to its own capture
/// time. Marked by the user around an optical change — a dust cleaning, a
/// re-spaced imaging train — so a frame is never matched to imaging done on
/// the other side of that change when nights lack their own calibration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidDirection {
    /// Only lights captured at or after this frame.
    Forward,
    /// Only lights captured at or before this frame.
    Backward,
}

impl ValidDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Backward => "backward",
        }
    }

    fn from_db(value: Option<String>) -> Option<Self> {
        match value.as_deref() {
            Some("forward") => Some(Self::Forward),
            Some("backward") => Some(Self::Backward),
            // Unknown text from a newer build reads as unrestricted rather
            // than failing the row: the old behavior, safely.
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
    /// The readout mode's name when the header spelled it that way; see
    /// [`FrameMeta::readout_mode_name`].
    pub readout_mode_name: Option<String>,
    pub bayer_pattern: Option<String>,
    pub exposure_s: Option<f64>,
    pub camera_temp: Option<f64>,
    pub filter: Option<String>,
    pub focal_length_mm: Option<f64>,
    /// Rotator angle in degrees at capture, when the rig recorded one.
    pub rotation: Option<f64>,
    /// User-set validity boundary; absent means both directions.
    pub valid_direction: Option<ValidDirection>,
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
    pub readout_mode_name: Option<String>,
    pub bayer_pattern: Option<String>,
    pub exposure_s: Option<f64>,
    pub camera_temp: Option<f64>,
    pub filter: Option<String>,
    pub focal_length_mm: Option<f64>,
    /// User-set validity boundary; absent means both directions.
    pub valid_direction: Option<ValidDirection>,
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
    /// The pedestal subtracted before flat division when the library holds
    /// no bias or dark master, fitted from the lights themselves. `None`
    /// when calibration used measured masters (or none at all).
    #[serde(default)]
    pub estimated_pedestal_adu: Option<f32>,
    /// How many calibration sessions the group's lights partitioned into.
    /// Multi-night stacks calibrate each session with its own masters. Zero
    /// on records written before sessions existed (single-session builds).
    #[serde(default)]
    pub sessions: usize,
    /// Per-session identity, in a stable order. The source-frame search
    /// pins each session's fitted pedestal from here instead of re-fitting
    /// it from a different sample of lights.
    #[serde(default)]
    pub session_details: Vec<CalibrationSessionDetail>,
}

/// One calibration session of a stack group, as recorded on the artifact.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct CalibrationSessionDetail {
    /// The session's selection fingerprint.
    pub fingerprint: String,
    /// The masters the session actually applied.
    pub masters_signature: String,
    /// The fitted pedestal the session's flat divided under, when one was.
    #[serde(default)]
    pub estimated_pedestal_adu: Option<f32>,
    /// How many of the group's lights calibrate in this session.
    pub lights: usize,
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
            estimated_pedestal_adu: None,
            sessions: 0,
            session_details: Vec::new(),
        }
    }
}

pub fn kind_from_meta(meta: &FrameMeta) -> Option<CalibrationKind> {
    let value = meta.image_type.as_deref()?.trim().to_ascii_uppercase();
    if value.contains("DARKFLAT")
        || value.contains("DARK FLAT")
        || value.contains("FLATDARK")
        || value.contains("FLAT DARK")
    {
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

/// One rung of the upgrade ladder: the version it produces, and the step that
/// gets there from the version below it.
///
/// Two rules bind every step.
///
/// It must be safe to run twice. A catalog can arrive here half upgraded — an
/// older build added the `rotation` column from its write path without
/// recording a version — and the version row is written after the step, so a
/// crash in between leaves the step to run again.
///
/// It must only add. Someone runs PSF Guard on two machines and one upgrades
/// first; the older build still has to read that catalog. Adding a column
/// leaves every older query working, which is why
/// [`schema_supports_current_reads`] checks physical columns instead of
/// rejecting a newer version number. Renaming or dropping one would not, and
/// would need a different mechanism than this ladder.
struct Migration {
    to_version: i64,
    apply: fn(&Connection) -> Result<bool>,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        to_version: 2,
        apply: add_rotation_column,
    },
    Migration {
        to_version: 3,
        apply: backfill_flat_rotation,
    },
    Migration {
        to_version: 4,
        apply: add_valid_direction_column,
    },
    Migration {
        to_version: 5,
        apply: add_readout_mode_name_column,
    },
    Migration {
        to_version: 6,
        apply: backfill_readout_mode_name,
    },
];

/// Columns selected by the calibration matching and master-building paths.
///
/// The version row says which migrations should have run, but it is not proof
/// that their physical changes are present. Older write paths and interrupted
/// upgrades can leave the two out of step, so readers verify the shape they
/// actually need before issuing a query that names these columns.
const READABLE_FRAME_COLUMNS: &[&str] = &[
    "id",
    "frame_uuid",
    "rig_uuid",
    "kind",
    "source_path",
    "source_fingerprint",
    "captured_at",
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
    "bayer_pattern",
    "exposure_s",
    "camera_temp",
    "filter_name",
    "focal_length_mm",
    "rotation",
    "valid_direction",
    "readout_mode_name",
];

fn add_valid_direction_column(conn: &Connection) -> Result<bool> {
    // NULL means "no boundary": the frame keeps matching in both time
    // directions, exactly as every frame did before the column existed.
    add_column_if_missing(
        conn,
        "psf_guard_calibration_frame",
        "valid_direction",
        "TEXT",
    )
}

fn add_rotation_column(conn: &Connection) -> Result<bool> {
    // NULL means "not recorded", which the matcher treats as compatible, so
    // an upgraded catalog keeps matching the flats it matched before.
    // [`backfill_flat_rotation`] then fills in what the files actually say.
    add_column_if_missing(conn, "psf_guard_calibration_frame", "rotation", "REAL")
}

/// Read the rotator angle off the flats that predate the `rotation` column.
///
/// Adding the column left every existing row NULL, and NULL means "not
/// recorded", which matching treats as compatible with any angle. A library
/// filled before the column existed therefore looked like one where the
/// rotator had never moved, and flats from nights an angle apart were offered
/// as one coherent set. The files knew all along; only the catalog did not.
///
/// Flats alone, because only a flat records an optical path — that also keeps
/// this to the smallest group in a library, where darks and biases usually
/// outnumber flats many times over. Headers only, no pixels. A file that has
/// moved away or has no `ROTATANG` keeps its NULL and is read, as before, as
/// an angle nobody wrote down.
fn backfill_flat_rotation(conn: &Connection) -> Result<bool> {
    // Data rungs run once, unlike column rungs, whose re-runs are free. A
    // flat whose file has no ROTATANG keeps its NULL forever, so re-reading
    // every NULL row's header on each open pays hundreds of file reads per
    // connection to learn nothing — a real catalog held 238 such flats and
    // paid on every open. Import records the angle itself now; the only rows
    // this can ever fill are the ones that existed before the column did,
    // and those were visited the first time.
    if recorded_schema_version(conn)? >= 3 {
        return Ok(false);
    }
    // A rung must survive a catalog that predates the columns it reads. This
    // one needs `kind` and `source_path` to find a flat and its file at all,
    // and a catalog without them holds no frame this could fill in. Failing
    // here would stall the ladder and leave every later rung unrun.
    let columns = table_column_names(conn, "psf_guard_calibration_frame").unwrap_or_default();
    let has = |wanted: &str| {
        columns
            .iter()
            .any(|column| column.eq_ignore_ascii_case(wanted))
    };
    if !(has("kind") && has("source_path") && has("rotation")) {
        return Ok(false);
    }

    let pending: Vec<(i64, String)> = conn
        .prepare(
            "SELECT id, source_path FROM psf_guard_calibration_frame
             WHERE kind = ?1 AND rotation IS NULL",
        )?
        .query_map([CalibrationKind::Flat.as_str()], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if pending.is_empty() {
        return Ok(false);
    }

    let mut recovered = 0_usize;
    for (id, source_path) in &pending {
        let meta = crate::commands::import::headers::read_frame_meta(Path::new(source_path));
        let Some(rotation) = meta.rotator_position else {
            continue;
        };
        conn.execute(
            "UPDATE psf_guard_calibration_frame SET rotation = ?1 WHERE id = ?2",
            rusqlite::params![rotation, id],
        )?;
        recovered += 1;
    }
    tracing::info!(
        "calibration upgrade: recovered a rotator angle for {recovered} of {} flat(s)",
        pending.len()
    );
    Ok(recovered > 0)
}

fn add_readout_mode_name_column(conn: &Connection) -> Result<bool> {
    // NULL means "not recorded", which matching treats as compatible, so an
    // upgraded catalog keeps matching what it matched before until
    // [`backfill_readout_mode_name`] reads the names off the files.
    add_column_if_missing(
        conn,
        "psf_guard_calibration_frame",
        "readout_mode_name",
        "TEXT",
    )
}

/// Read the readout mode's name off the frames that predate the column.
///
/// N.I.N.A. writes READOUTM as a display name, and the integer column has no
/// room for one, so every frame from such a rig recorded no readout mode at
/// all. That let a "High Gain Mode" dark serve an "Extend Fullwell" light on
/// the same camera. Every kind is affected, so unlike the rotation rung this
/// visits every row with no name. Headers only, no pixels; a file that has
/// moved away or names no mode keeps its NULL.
fn backfill_readout_mode_name(conn: &Connection) -> Result<bool> {
    // A data rung runs once; see `backfill_flat_rotation` for why re-reading
    // every NULL row's header on each open is a cost paid to learn nothing.
    if recorded_schema_version(conn)? >= 6 {
        return Ok(false);
    }
    let columns = table_column_names(conn, "psf_guard_calibration_frame").unwrap_or_default();
    let has = |wanted: &str| {
        columns
            .iter()
            .any(|column| column.eq_ignore_ascii_case(wanted))
    };
    if !(has("source_path") && has("readout_mode_name")) {
        return Ok(false);
    }

    let pending: Vec<(i64, String)> = conn
        .prepare(
            "SELECT id, source_path FROM psf_guard_calibration_frame
             WHERE readout_mode_name IS NULL",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if pending.is_empty() {
        return Ok(false);
    }

    let mut recovered = 0_usize;
    for (id, source_path) in &pending {
        let meta = crate::commands::import::headers::read_frame_meta(Path::new(source_path));
        let Some(name) = meta.readout_mode_name else {
            continue;
        };
        conn.execute(
            "UPDATE psf_guard_calibration_frame SET readout_mode_name = ?1 WHERE id = ?2",
            rusqlite::params![name, id],
        )?;
        recovered += 1;
    }
    tracing::info!(
        "calibration upgrade: recovered a readout mode name for {recovered} of {} frame(s)",
        pending.len()
    );
    Ok(recovered > 0)
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    declaration: &str,
) -> Result<bool> {
    let present = table_column_names(conn, table)?
        .iter()
        .any(|name| name.eq_ignore_ascii_case(column));
    if present {
        return Ok(false);
    }
    match conn.execute_batch(&format!(
        "ALTER TABLE {table} ADD COLUMN {column} {declaration};"
    )) {
        Ok(()) => Ok(true),
        // Two processes can open the same catalog at the same moment, both
        // find the column missing, and both try to add it. SQLite serializes
        // the writes, so one wins and the other is told the column is already
        // there — which is the state we wanted. Treating that as a failure
        // would make the loser log an upgrade error and report no calibration
        // library for a catalog that had just been upgraded correctly.
        Err(error) => {
            let present_after_error = table_column_names(conn, table)
                .map(|columns| columns.iter().any(|name| name.eq_ignore_ascii_case(column)))
                .unwrap_or(false);
            if present_after_error {
                Ok(false)
            } else {
                Err(error).with_context(|| format!("adding {table}.{column}"))
            }
        }
    }
}

fn recorded_schema_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT version FROM psf_guard_calibration_schema WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?)
}

/// Advance the migration cursor without ever overwriting a version written by
/// a newer process. Return the version currently stored and whether this call
/// moved it.
fn advance_schema_version(conn: &Connection, to_version: i64) -> Result<(i64, bool)> {
    let advanced = conn.execute(
        "UPDATE psf_guard_calibration_schema
         SET version = ?1
         WHERE singleton = 1 AND version < ?1",
        [to_version],
    )? > 0;
    Ok((recorded_schema_version(conn)?, advanced))
}

/// Run every idempotent physical check and advance the stored version through
/// any steps this catalog still needs. Assumes the tables exist.
fn run_migrations(conn: &Connection) -> Result<bool> {
    let mut version = recorded_schema_version(conn)?;
    if version > CALIBRATION_SCHEMA_VERSION {
        anyhow::bail!(
            "PSF Guard calibration schema version {version} is newer than this build \
             supports (expected at most {CALIBRATION_SCHEMA_VERSION}); upgrade PSF Guard"
        );
    }
    let mut changed = false;
    for migration in MIGRATIONS {
        // Another process may have advanced the catalog since the previous
        // rung. Known physical steps are additive, but this build must stop
        // before changing a schema whose recorded version is now newer.
        version = recorded_schema_version(conn)?;
        if version > CALIBRATION_SCHEMA_VERSION {
            anyhow::bail!(
                "PSF Guard calibration schema version {version} is newer than this build \
                 supports (expected at most {CALIBRATION_SCHEMA_VERSION}); upgrade PSF Guard"
            );
        }
        // The version row is a migration cursor, not proof that the physical
        // change survived. Each step is required to be idempotent, so rerun
        // it even at the recorded version and repair supported schema drift.
        changed |= (migration.apply)(conn).with_context(|| {
            format!(
                "upgrading the PSF Guard calibration schema to version {}",
                migration.to_version
            )
        })?;
        if version < migration.to_version {
            let (observed, advanced) = advance_schema_version(conn, migration.to_version)?;
            version = observed;
            changed |= advanced;
            if advanced {
                tracing::info!("Upgraded the PSF Guard calibration schema to version {version}");
            }
        } else {
            // Detect a concurrent newer writer even when this rung needed no
            // cursor update of its own.
            version = recorded_schema_version(conn)?;
        }
        if version > CALIBRATION_SCHEMA_VERSION {
            anyhow::bail!(
                "PSF Guard calibration schema version {version} is newer than this build \
                 supports (expected at most {CALIBRATION_SCHEMA_VERSION}); upgrade PSF Guard"
            );
        }
    }
    Ok(changed)
}

/// Upgrade a catalog that already carries PSF Guard's tables, and report
/// whether there was anything to upgrade.
///
/// This never creates the calibration tables. A catalog that has never held
/// calibration data is left alone by this function; the first calibration
/// import creates those tables.
pub fn migrate_existing(conn: &Connection) -> Result<bool> {
    if !schema_exists(conn) {
        return Ok(false);
    }
    run_migrations(conn)
}

/// Whether this catalog's PSF Guard tables carry every frame column this build
/// reads, regardless of what the version row claims.
///
/// An older or read-only catalog can have the required columns while its
/// version row still lags; it remains safe to read. Conversely, a version row
/// at or above the current number is not allowed to hide a missing physical
/// column and recreate the `no such column` failure this guard prevents.
pub fn schema_supports_current_reads(conn: &Connection) -> bool {
    if !schema_exists(conn) {
        return false;
    }
    let complete = table_column_names(conn, "psf_guard_calibration_frame")
        .map(|columns| {
            READABLE_FRAME_COLUMNS.iter().all(|required| {
                columns
                    .iter()
                    .any(|column| column.eq_ignore_ascii_case(required))
            })
        })
        .unwrap_or(false);
    if !complete {
        // Loud, because the alternative is a stack or export quietly
        // calibrating nothing. This is the one state a user has to act on.
        tracing::warn!(
            "This catalog's calibration tables are missing columns this build reads and \
             could not be upgraded, so no calibration will be applied. Open it with \
             write access once to upgrade it."
        );
    }
    complete
}

pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS psf_guard_calibration_schema (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            version   INTEGER NOT NULL
        );

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
            readout_mode_name  TEXT,
            bayer_pattern      TEXT,
            bayer_x_offset     INTEGER,
            bayer_y_offset     INTEGER,
            exposure_s         REAL,
            camera_temp        REAL,
            filter_name        TEXT,
            focal_length_mm    REAL,
            rotation           REAL,
            valid_direction    TEXT,
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
    // A catalog that has just had its tables created is already at the
    // current version; one that had them from an older build is stamped with
    // whatever it was, and the ladder below carries it up.
    conn.execute(
        "INSERT INTO psf_guard_calibration_schema (singleton, version)
             VALUES (1, ?1)
             ON CONFLICT(singleton) DO NOTHING",
        [CALIBRATION_SCHEMA_VERSION],
    )?;
    run_migrations(conn).map(|_| ())
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
                file_mtime_ns, added_at, updated_at, rotation, readout_mode_name
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24,
                ?25, ?26, ?27, ?27, ?28, ?29
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
                updated_at=excluded.updated_at, rotation=excluded.rotation,
                readout_mode_name=excluded.readout_mode_name
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
                frame.rotator_position,
                frame.readout_mode_name,
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
    // The listing tolerates a catalog that could not be upgraded: an absent
    // column reads as NULL rather than failing the whole dialog.
    let columns = table_column_names(conn, "psf_guard_calibration_frame")?;
    let has = |wanted: &str| columns.iter().any(|name| name.eq_ignore_ascii_case(wanted));
    let validity_column = if has("valid_direction") {
        "valid_direction"
    } else {
        "NULL AS valid_direction"
    };
    let readout_name_column = if has("readout_mode_name") {
        "readout_mode_name"
    } else {
        "NULL AS readout_mode_name"
    };
    let mut statement = conn.prepare(&format!(
        r#"
        SELECT id, frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
               captured_at, telescope, camera, width, height, channels,
               binning_x, binning_y, gain, offset, readout_mode, bayer_pattern,
               exposure_s, camera_temp, filter_name, focal_length_mm,
               NULL AS rotation, {validity_column}, {readout_name_column}
        FROM psf_guard_calibration_frame
        ORDER BY kind, captured_at DESC, source_path COLLATE NOCASE
        "#
    ))?;
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
                readout_mode_name: frame.readout_mode_name,
                bayer_pattern: frame.bayer_pattern,
                exposure_s: frame.exposure_s,
                camera_temp: frame.camera_temp,
                filter: frame.filter,
                focal_length_mm: frame.focal_length_mm,
                valid_direction: frame.valid_direction,
            })
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(CalibrationLibraryDetails { summary, frames })
}

/// Mark frames with a validity boundary (or clear one with `None`). Returns
/// how many rows changed. A catalog that could not be upgraded to carry the
/// column reports zero rather than failing.
pub fn set_frames_validity(
    conn: &Connection,
    frame_uuids: &[String],
    direction: Option<ValidDirection>,
) -> Result<usize> {
    if frame_uuids.is_empty() || !schema_exists(conn) {
        return Ok(0);
    }
    let has_validity = table_column_names(conn, "psf_guard_calibration_frame")?
        .iter()
        .any(|name| name.eq_ignore_ascii_case("valid_direction"));
    if !has_validity {
        anyhow::bail!(
            "this catalog has not been upgraded to record calibration validity; \
             open it with write access once and retry"
        );
    }
    let mut changed = 0usize;
    let mut statement = conn.prepare(
        "UPDATE psf_guard_calibration_frame
         SET valid_direction = ?1, updated_at = ?2
         WHERE frame_uuid = ?3",
    )?;
    let now = chrono::Utc::now().timestamp();
    for frame_uuid in frame_uuids {
        changed += statement.execute(rusqlite::params![
            direction.map(ValidDirection::as_str),
            now,
            frame_uuid
        ])?;
    }
    Ok(changed)
}

pub fn forget_frame(conn: &mut Connection, frame_uuid: &str) -> Result<CalibrationMutationOutcome> {
    forget_frames(conn, std::slice::from_ref(&frame_uuid.to_string()))
}

/// Remove several catalog records in one transaction — a whole night of a
/// kind at once — with the same transitive master invalidation a single
/// forget performs. The FITS files are never touched.
pub fn forget_frames(
    conn: &mut Connection,
    frame_uuids: &[String],
) -> Result<CalibrationMutationOutcome> {
    if frame_uuids.is_empty() || !schema_exists(conn) {
        return Ok(CalibrationMutationOutcome::default());
    }
    let transaction = conn.transaction()?;
    let uuid_set = frame_uuids
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let any_exists = {
        let mut statement = transaction
            .prepare("SELECT 1 FROM psf_guard_calibration_frame WHERE frame_uuid = ?1")?;
        frame_uuids
            .iter()
            .any(|uuid| statement.exists([uuid]).unwrap_or(false))
    };
    if !any_exists {
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
        .filter(|master| {
            master
                .sources
                .iter()
                .any(|source| uuid_set.contains(source.as_str()))
        })
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
    let mut frames_removed = 0usize;
    {
        let mut statement =
            transaction.prepare("DELETE FROM psf_guard_calibration_frame WHERE frame_uuid = ?1")?;
        for frame_uuid in frame_uuids {
            frames_removed += statement.execute([frame_uuid])?;
        }
    }
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
    // Not `schema_exists`: a catalog whose tables predate a column this query
    // selects has the table but not the column, and asking for it fails the
    // whole stack with a raw SQL error. Opening the catalog upgrades it; one
    // that could not be upgraded reports no calibration instead.
    if !schema_supports_current_reads(conn) {
        return Ok(CalibrationSelection::default());
    }
    let mut statement = conn.prepare(
        r#"
        SELECT id, frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
               captured_at, telescope, camera, width, height, channels,
               binning_x, binning_y, gain, offset, readout_mode, bayer_pattern,
               exposure_s, camera_temp, filter_name, focal_length_mm, rotation,
               valid_direction, readout_mode_name
        FROM psf_guard_calibration_frame
        ORDER BY captured_at DESC, id DESC
        "#,
    )?;
    let frames = statement
        .query_map([], row_to_frame)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut selected = CalibrationSelection::default();
    for candidate in frames {
        // A frame marked usable only forward or backward of its capture —
        // shot around an optics change or cleaning — never serves a light
        // on the other side of that boundary.
        if !validity_admits(&candidate, light.timestamp) {
            continue;
        }
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
            // Dark-flats pair with the flat, but the boundary is judged
            // against the light: the mark protects the imaging on the
            // other side of the change.
            if validity_admits(&candidate, light.timestamp)
                && frame_pair_matches(flat, &candidate)
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

// ---------- Project calibration report ----------

/// How one project's lights are covered by the calibration library: what
/// matches, how old it is, and whether each night has its own flats.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectCalibrationReport {
    /// One entry per night of lights, newest first.
    pub nights: Vec<CalibrationNightReport>,
    /// Library-wide matching summary per kind, across every light
    /// configuration the project uses.
    pub kinds: Vec<CalibrationKindSummary>,
    pub warnings: Vec<String>,
    /// Lights whose file was not found in the image directories; the
    /// report is built from the lights that resolve.
    pub lights_missing_files: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationKindSummary {
    /// `bias`, `dark`, `dark_flat`, or `flat`.
    pub kind: String,
    /// Distinct matching frames across the whole project.
    pub matching_frames: usize,
    /// Distinct capture days those frames span (night-of, newest first).
    pub sessions: Vec<String>,
    pub newest_at: Option<i64>,
    pub oldest_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationNightReport {
    /// The imaging night (the date twelve hours before the exposures, so a
    /// session spanning midnight stays one night).
    pub night: String,
    pub lights: usize,
    pub filters: Vec<CalibrationNightFilter>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationNightFilter {
    pub filter: String,
    pub lights: usize,
    pub bias_frames: usize,
    pub dark_frames: usize,
    /// Nearest matching dark's distance from this night, in days.
    pub dark_age_days: Option<f64>,
    pub dark_flat_frames: usize,
    pub flat_frames: usize,
    /// The capture day of the flat session a master would build from.
    pub flat_session: Option<String>,
    /// That session's distance from this night, in days.
    pub flat_age_days: Option<f64>,
    /// Whether flats were shot within a day of this night's lights.
    pub nightly_flats: bool,
    /// Kinds with no matching frames at all for this configuration.
    pub missing: Vec<String>,
}

/// The imaging night a timestamp belongs to: subtract twelve hours, take
/// the date, so 01:30 belongs to the evening before.
fn night_of(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp - 12 * 3600, 0)
        .map(|when| when.date_naive().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn day_of(timestamp: i64) -> String {
    night_of(timestamp)
}

/// Build the calibration coverage report for one project's lights.
///
/// One representative light per night and filter is read from disk (header
/// only) and matched against the library exactly as a stack build would,
/// so the report describes what calibration WOULD apply — not just what
/// files exist.
pub fn project_calibration_report(
    conn: &Connection,
    project_id: i32,
    directory_tree: &crate::directory_tree::DirectoryTree,
) -> Result<ProjectCalibrationReport> {
    let db = crate::db::Database::new(conn);
    let rows = db
        .query_images_scoped(None, Some(project_id), None, None, 0)
        .context("querying project lights")?;

    // Bucket lights by (night, filter) and pick the median exposure of each
    // bucket as its representative.
    let mut buckets: HashMap<(String, String), Vec<(i64, String)>> = HashMap::new();
    for (image, _project, _target) in &rows {
        let Some(acquired) = image.acquired_date else {
            continue;
        };
        let Some(basename) = crate::utils::extract_filename(&image.metadata) else {
            continue;
        };
        buckets
            .entry((night_of(acquired), image.filter_name.clone()))
            .or_default()
            .push((acquired, basename));
    }

    let mut lights_missing_files = 0usize;
    let mut nights: HashMap<String, CalibrationNightReport> = HashMap::new();
    // Distinct matching frames per kind, across every representative.
    let mut kind_frames: HashMap<CalibrationKind, HashMap<String, Option<i64>>> = HashMap::new();
    let mut nights_without_nightly_flats: Vec<String> = Vec::new();
    let mut worst_flat_age: Option<(String, String, f64)> = None;

    let mut keys: Vec<(String, String)> = buckets.keys().cloned().collect();
    keys.sort();
    for key in keys {
        let mut lights = buckets.remove(&key).unwrap_or_default();
        let (night, filter) = key;
        lights.sort();
        // The median light stands for the bucket; walk outward if its file
        // is missing so one lost file does not blank a night.
        let mut representative = None;
        let middle = lights.len() / 2;
        let mut order: Vec<usize> = (0..lights.len()).collect();
        order.sort_by_key(|index| index.abs_diff(middle));
        for index in order {
            let (acquired, basename) = &lights[index];
            if let Some(path) = directory_tree.find_file_first(basename) {
                representative = Some((*acquired, path.clone()));
                break;
            }
        }
        let Some((light_at, light_path)) = representative else {
            lights_missing_files += lights.len();
            continue;
        };

        let meta = crate::commands::import::headers::read_frame_meta(&light_path);
        let selected = select_for_light(conn, &meta)?;

        let day_seconds = 86_400i64;
        let age_days = |captured: Option<i64>| {
            captured.map(|at| (at.abs_diff(light_at)) as f64 / day_seconds as f64)
        };
        let nearest_age = |frames: &[CalibrationFrame]| {
            frames
                .iter()
                .filter_map(|frame| age_days(frame.captured_at))
                .min_by(f64::total_cmp)
        };

        for (kind, frames) in [
            (CalibrationKind::Bias, &selected.bias),
            (CalibrationKind::Dark, &selected.dark),
            (CalibrationKind::DarkFlat, &selected.dark_flat),
            (CalibrationKind::Flat, &selected.flat),
        ] {
            let entry = kind_frames.entry(kind).or_default();
            for frame in frames {
                entry.insert(frame.frame_uuid.clone(), frame.captured_at);
            }
        }

        let flat_subset = coherent_master_subset(CalibrationKind::Flat, &selected.flat);
        let flat_session_at = if flat_subset.is_empty() {
            None
        } else {
            let mut times: Vec<i64> = flat_subset.iter().filter_map(|f| f.captured_at).collect();
            times.sort();
            times.get(times.len() / 2).copied()
        };
        let nightly_flats = selected.flat.iter().any(|frame| {
            frame
                .captured_at
                .is_some_and(|at| at.abs_diff(light_at) <= day_seconds as u64)
        });
        if !selected.flat.is_empty() && !nightly_flats {
            nights_without_nightly_flats.push(night.clone());
        }
        if let Some(age) = age_days(flat_session_at)
            && worst_flat_age
                .as_ref()
                .is_none_or(|(_, _, worst)| age > *worst)
        {
            worst_flat_age = Some((night.clone(), filter.clone(), age));
        }

        let mut missing = Vec::new();
        if selected.bias.is_empty() {
            missing.push("bias".to_string());
        }
        if selected.dark.is_empty() {
            missing.push("dark".to_string());
        }
        if selected.flat.is_empty() {
            missing.push("flat".to_string());
        }

        let filter_report = CalibrationNightFilter {
            filter,
            lights: lights.len(),
            bias_frames: selected.bias.len(),
            dark_frames: selected.dark.len(),
            dark_age_days: nearest_age(&selected.dark),
            dark_flat_frames: selected.dark_flat.len(),
            flat_frames: selected.flat.len(),
            flat_session: flat_session_at.map(day_of),
            flat_age_days: age_days(flat_session_at),
            nightly_flats,
            missing,
        };
        let entry = nights
            .entry(night.clone())
            .or_insert(CalibrationNightReport {
                night,
                lights: 0,
                filters: Vec::new(),
            });
        entry.lights += filter_report.lights;
        entry.filters.push(filter_report);
    }

    let mut nights: Vec<CalibrationNightReport> = nights.into_values().collect();
    nights.sort_by(|left, right| right.night.cmp(&left.night));

    let mut kinds = Vec::new();
    for kind in [
        CalibrationKind::Bias,
        CalibrationKind::Dark,
        CalibrationKind::DarkFlat,
        CalibrationKind::Flat,
    ] {
        let frames = kind_frames.remove(&kind).unwrap_or_default();
        let mut sessions: Vec<String> = frames
            .values()
            .filter_map(|at| at.map(day_of))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        sessions.sort_by(|left, right| right.cmp(left));
        kinds.push(CalibrationKindSummary {
            kind: kind.as_str().to_string(),
            matching_frames: frames.len(),
            newest_at: frames.values().filter_map(|at| *at).max(),
            oldest_at: frames.values().filter_map(|at| *at).min(),
            sessions,
        });
    }

    let mut warnings = Vec::new();
    for summary in &kinds {
        if summary.matching_frames == 0 && summary.kind != "dark_flat" {
            warnings.push(format!(
                "No {} frames match any of this project's lights",
                summary.kind
            ));
        }
    }
    if !nights_without_nightly_flats.is_empty() {
        nights_without_nightly_flats.sort();
        nights_without_nightly_flats.dedup();
        warnings.push(format!(
            "{} of {} nights have no same-night flats",
            nights_without_nightly_flats.len(),
            nights.len()
        ));
    }
    if let Some((night, filter, age)) = worst_flat_age
        && age > 30.0
    {
        warnings.push(format!(
            "The {filter} flats nearest the {night} lights are {age:.0} days away — dust moves; consider fresh flats"
        ));
    }

    Ok(ProjectCalibrationReport {
        nights,
        kinds,
        warnings,
        lights_missing_files,
    })
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

/// Fit the bias pedestal of a flats-only calibration set from the lights.
///
/// The light's background is `sky * V(x) + pedestal`, with `V` the
/// median-normalized flat response, so a robust line fit of per-tile
/// background against per-tile flat response recovers the pedestal as the
/// intercept. Every guardrail failure returns `None` and the caller falls
/// back to withholding the flat.
fn estimate_flat_pedestal(
    flat_path: &Path,
    light_paths: &[PathBuf],
    hint: Option<f64>,
) -> Option<f32> {
    const MAX_SAMPLE_LIGHTS: usize = 3;
    const MAX_PEDESTAL_ADU: f32 = 13_107.0; // 20% of the 16-bit scale

    let flat = crate::image_io::open_linear_frame(flat_path).ok()?;
    // CFA data interleaves photosite responses that differ per channel;
    // the single-line model only holds for mono frames.
    if flat.bayer.is_some() || flat.image.channels != 1 {
        tracing::debug!("pedestal fit skipped: the flat is not a mono frame");
        return None;
    }

    // Sample lights by sorted path so the estimate does not depend on the
    // caller's frame order — the source-frame search must re-derive the
    // same pedestal the stack build found.
    let mut sorted = light_paths.to_vec();
    sorted.sort();
    sorted.dedup();
    let sample_indices: Vec<usize> = match sorted.len() {
        0 => return None,
        1 => vec![0],
        2 => vec![0, 1],
        len => vec![0, len / 2, len - 1],
    };

    let mut estimates = Vec::new();
    for index in sample_indices {
        let Ok(light) = crate::image_io::open_linear_frame(&sorted[index]) else {
            continue;
        };
        if light.bayer.is_some()
            || light.image.channels != 1
            || light.image.width != flat.image.width
            || light.image.height != flat.image.height
        {
            continue;
        }
        if let Some(pedestal) = fit_pedestal_against_flat(&light.image, &flat.image) {
            estimates.push(pedestal);
        }
        if estimates.len() >= MAX_SAMPLE_LIGHTS {
            break;
        }
    }
    if estimates.is_empty() {
        tracing::debug!("pedestal fit skipped: no light produced a usable fit");
        return None;
    }
    estimates.sort_by(f32::total_cmp);
    let pedestal = estimates[estimates.len() / 2];
    let spread = estimates[estimates.len() - 1] - estimates[0];
    if spread > (pedestal * 0.25).max(64.0) {
        tracing::debug!("pedestal fit skipped: per-light estimates disagree ({estimates:?} ADU)");
        return None;
    }
    if !(0.0..=MAX_PEDESTAL_ADU).contains(&pedestal) {
        tracing::debug!("pedestal fit skipped: {pedestal:.1} ADU is outside the sane range");
        return None;
    }
    // A camera whose driver records its offset gives an independent
    // prediction; a fit that contradicts it by more than 3x either way is
    // more likely a sky-gradient artifact than a pedestal.
    if let Some(hint) = hint {
        let hint = hint as f32;
        if pedestal < hint / 3.0 || pedestal > hint * 3.0 {
            tracing::debug!(
                "pedestal fit skipped: {pedestal:.1} ADU contradicts the camera's \
                 recorded offset (~{hint:.0} ADU)"
            );
            return None;
        }
    }
    Some(pedestal)
}

/// Fit the pedestal a camera added, by regressing each tile's sky against the
/// flat's response there.
///
/// The fit is `seiza-calibration`'s. It declines rather than guesses when the
/// frame cannot support one, and reports a caller mistake — a colour frame, a
/// mismatched size — separately from that, which is why the two outcomes are
/// distinguished here rather than collapsed into "no pedestal".
fn fit_pedestal_against_flat(
    light: &seiza_stacking::LinearImage,
    flat: &seiza_stacking::LinearImage,
) -> Option<f32> {
    fn view(image: &seiza_stacking::LinearImage) -> Option<seiza_calibration::LinearImageRef<'_>> {
        seiza_calibration::LinearImageRef::new(
            &image.data,
            image.width,
            image.height,
            image.channels,
        )
        .map_err(|error| tracing::debug!("pedestal fit skipped: {error}"))
        .ok()
    }
    let (light_view, flat_view) = (view(light)?, view(flat)?);
    match seiza_calibration::fit_flat_pedestal(light_view, flat_view) {
        Ok(pedestal) => pedestal,
        Err(error) => {
            tracing::debug!("pedestal fit skipped: {error}");
            None
        }
    }
}

/// The camera's recorded offset as a pedestal in 16-bit ADU, for camera
/// families whose driver mapping is known. ZWO ASI drivers apply the offset
/// setting in 10-ADU steps at the 16-bit scale. Header metadata is a
/// prediction, not pixel evidence, so this only corroborates a fitted
/// pedestal — it is never applied on its own.
fn header_pedestal_hint(light: &FrameMeta) -> Option<f64> {
    let offset = light.offset.filter(|value| *value > 0)?;
    let camera = light.camera.as_deref()?.to_ascii_uppercase();
    (camera.contains("ZWO") || camera.contains("ASI")).then_some(offset as f64 * 10.0)
}

pub fn resolve_or_build_masters(
    conn: &Connection,
    cache_root: &Path,
    light_paths: &[PathBuf],
    directory_tree: Option<&crate::directory_tree::DirectoryTree>,
    cancel: Option<&AtomicBool>,
    mode: CalibrationMode,
) -> Result<(seiza_stacking::CalibrationMasters, AppliedCalibration)> {
    resolve_or_build_masters_pinned(
        conn,
        cache_root,
        light_paths,
        directory_tree,
        cancel,
        mode,
        None,
    )
}

/// [`resolve_or_build_masters`] with a previously fitted pedestal carried
/// in. The source-frame search reproduces a stack's calibration from a
/// possibly smaller set of lights (rejected frames are not searched), so it
/// must reuse the pedestal the build recorded rather than fit its own from
/// a different sample.
fn resolve_or_build_masters_pinned(
    conn: &Connection,
    cache_root: &Path,
    light_paths: &[PathBuf],
    directory_tree: Option<&crate::directory_tree::DirectoryTree>,
    cancel: Option<&AtomicBool>,
    mode: CalibrationMode,
    pinned_pedestal: Option<f32>,
) -> Result<(seiza_stacking::CalibrationMasters, AppliedCalibration)> {
    if mode == CalibrationMode::Off {
        return Ok(calibration_off());
    }
    let Some(light_path) = light_paths.first() else {
        anyhow::bail!("calibration needs at least one light frame");
    };
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

    // A read-only catalog, or one written by a newer build, can still reuse
    // masters whose row and cache file already exist. Do not turn that read
    // path into a fatal setup write. If a new master is needed,
    // `build_master` reports the blocker and the stack continues without it.
    let master_recording_blocker = if conn.is_readonly(rusqlite::MAIN_DB)? {
        Some("catalog is read-only".to_string())
    } else {
        // Refuse a future schema before `ensure_schema` runs any
        // `CREATE IF NOT EXISTS` statements. Newer catalogs are readable but
        // this build must not change their shape or record new masters.
        match migrate_existing(conn).and_then(|_| ensure_schema(conn)) {
            Ok(()) => None,
            Err(error) => {
                tracing::warn!(
                    "Cannot prepare the calibration catalog to record generated masters: \
                     {error:#}. Existing recorded cached masters remain usable."
                );
                Some(format!(
                    "catalog schema cannot be updated by this build: {error:#}"
                ))
            }
        }
    };
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
    // Frames that clustered but that the integrator would have refused. The
    // master still builds from the rest; this says what was left out, because
    // a dropped frame usually means the library holds something the selection
    // should not have offered.
    let mut set_aside: Vec<(CalibrationKind, usize, String)> = Vec::new();
    let build_or_warn = |kind: CalibrationKind,
                         frames: &[CalibrationFrame],
                         inputs: MasterInputs<'_>,
                         skip_because: Option<&str>,
                         failures: &mut Vec<(CalibrationKind, String)>,
                         set_aside: &mut Vec<(CalibrationKind, usize, String)>|
     -> Option<BuiltMaster> {
        if frames.is_empty() {
            return None;
        }
        let report = master_subset_report(kind, frames);
        if let Some(first) = report.dropped.first() {
            tracing::warn!(
                "{} master: {} frame(s) set aside as incompatible with {}",
                kind.as_str(),
                report.dropped.len(),
                first.source_path.display()
            );
            set_aside.push((
                kind,
                report.dropped.len(),
                first.source_path.display().to_string(),
            ));
        }
        if let Some(failed_dependency) = skip_because {
            let reason = format!("skipped because the {failed_dependency} master failed to build");
            tracing::warn!("{} master {reason}", kind.as_str());
            failures.push((kind, reason));
            return None;
        }
        match build_master(
            conn,
            &master_root,
            kind,
            frames,
            inputs,
            master_recording_blocker.as_deref(),
        ) {
            // The integrator reads the headers, so it catches what selection
            // could not: a catalog holds only what it recorded at import.
            Ok(Some(master)) => {
                if let Some((path, _)) = master.skipped.first() {
                    set_aside.push((kind, master.skipped.len(), path.display().to_string()));
                }
                Some(master)
            }
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
        &mut set_aside,
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
        &mut set_aside,
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
        &mut set_aside,
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
        &mut set_aside,
    );
    // A flat can only be DIVIDED into a light whose pedestal has been
    // removed. With neither a bias nor a dark master, the division
    // amplifies the light's uncorrected pedestal by 1/vignette at the
    // frame edges — the stack comes out with the vignette inverted
    // (bright edges), which is worse than no flat at all. The flat master
    // itself stays cached: once bias or dark frames are imported, the
    // dependency-aware hash rebuilds and applies it properly.
    let mut flat_note = None;
    let mut estimated_pedestal = None;
    let flat = if flat.is_some() && bias.is_none() && dark.is_none() {
        if mode == CalibrationMode::On {
            // The caller forced calibration on, accepting the damage Auto
            // refuses. Say what to expect so a bright-edged result is not
            // mistaken for a broken flat.
            flat_note = Some(
                "Flat applied without a bias or dark master because calibration is forced on; \
                 vignetted edges may brighten (the light's pedestal is amplified by the flat \
                 correction)"
                    .to_string(),
            );
            flat
        } else {
            // The pedestal the division needs removed can be fitted from
            // the lights themselves: background = sky * flat response +
            // pedestal, so the intercept of that line is the pedestal. A
            // pinned value from the stack being reproduced wins over a
            // fresh fit.
            estimated_pedestal = pinned_pedestal.or_else(|| {
                flat.as_ref().and_then(|master| {
                    estimate_flat_pedestal(&master.path, light_paths, header_pedestal_hint(&light))
                })
            });
            if estimated_pedestal.is_some() {
                flat
            } else {
                let reason = "skipped: dividing a flat into a light with no bias or dark master \
                      brightens vignetted edges (the light's pedestal is amplified by \
                      the flat correction), and no reliable pedestal could be fitted from \
                      these lights; import bias or dark frames for this camera, \
                      or force calibration on for this stack";
                tracing::warn!("flat master {reason}");
                build_failures.push((CalibrationKind::Flat, reason.into()));
                None
            }
        }
    } else {
        flat
    };
    applied.flat_master = flat.as_ref().map(|master| master.label());

    let masters = if let (Some(pedestal), Some(flat_master)) = (estimated_pedestal, flat.as_ref()) {
        // Hand seiza a synthesized constant bias so the existing
        // light-minus-bias-then-divide path removes the fitted pedestal.
        // The cached flat master is already normalized, so seiza leaves
        // its values alone.
        let flat_frame = crate::image_io::open_linear_frame(&flat_master.path)
            .context("loading the cached master flat")?;
        let bias_image = seiza_stacking::LinearImage::new(
            flat_frame.image.width,
            flat_frame.image.height,
            flat_frame.image.channels,
            vec![pedestal; flat_frame.image.data.len()],
        )
        .context("synthesizing the estimated-pedestal bias")?;
        let flat_frame = seiza_stacking::MasterFlat::from_fits_frame(flat_frame)
            .context("preparing the master flat")?;
        seiza_stacking::CalibrationMasters::new(Some(bias_image), None, Some(flat_frame))
            .context("preparing estimated-pedestal calibration")?
    } else {
        seiza_stacking::CalibrationMasters::from_fits_paths(
            bias.as_ref().map(|value| value.path.as_path()),
            dark.as_ref().map(|value| value.path.as_path()),
            flat.as_ref().map(|value| value.path.as_path()),
            light.exposure_s,
        )
        .context("loading matched calibration masters")?
    };
    applied.estimated_pedestal_adu = estimated_pedestal;
    if let Some(pedestal) = estimated_pedestal {
        let hint_note = match header_pedestal_hint(&light) {
            Some(hint) => {
                format!(", consistent with the camera's recorded offset (~{hint:.0} ADU)")
            }
            None => String::new(),
        };
        flat_note = Some(format!(
            "No bias or dark master; applied the flat after subtracting a {pedestal:.0} ADU \
             pedestal fitted from the lights{hint_note}. Import bias or dark frames for \
             measured calibration"
        ));
    }
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
    if !set_aside.is_empty() {
        let note = format!(
            "Built without frames the integrator would not accept — {}. Check the library for \
             frames filed under the wrong filter or camera",
            set_aside
                .iter()
                .map(|(kind, count, example)| format!(
                    "{}: {count} frame(s), e.g. {example}",
                    kind.as_str()
                ))
                .collect::<Vec<_>>()
                .join("; ")
        );
        applied.warning = Some(match applied.warning.take() {
            Some(previous) => format!("{previous}. {note}"),
            None => note,
        });
    }
    if let Some(note) = flat_note {
        applied.warning = Some(match applied.warning.take() {
            Some(previous) => format!("{previous}. {note}"),
            None => note,
        });
    }
    applied.masters_signature = masters_signature(&applied);
    applied.sessions = 1;
    applied.session_details = vec![CalibrationSessionDetail {
        fingerprint: applied.fingerprint.clone(),
        masters_signature: applied.masters_signature.clone(),
        estimated_pedestal_adu: applied.estimated_pedestal_adu,
        lights: light_paths.len(),
    }];
    Ok((masters, applied))
}

/// The masters a resolution actually applied, as one comparable string.
/// The selection fingerprint is computed before any build and cannot see a
/// failed or skipped build, so consumers that must not mix calibrations —
/// the stack resume checkpoint and the source-frame search — compare this
/// too.
fn masters_signature(applied: &AppliedCalibration) -> String {
    let label = |master: &Option<String>| master.as_deref().unwrap_or("none").to_string();
    let mut signature = format!(
        "bias={};dark={};dark_flat={};flat={}",
        label(&applied.bias_master),
        label(&applied.dark_master),
        label(&applied.dark_flat_master),
        label(&applied.flat_master),
    );
    // Only estimated-pedestal stacks carry the suffix, so signatures stored
    // before it existed still compare equal.
    if let Some(pedestal) = applied.estimated_pedestal_adu {
        signature.push_str(&format!(";pedestal=estimated-{pedestal:.1}"));
    }
    signature
}

/// One calibration session of a stack group: the masters its lights
/// calibrate with and how they resolved.
pub struct CalibrationSession {
    pub masters: seiza_stacking::CalibrationMasters,
    pub applied: AppliedCalibration,
}

/// How a stack group's lights calibrate: each light is assigned to a
/// session, and each session carries its own masters. A single-night group
/// has one session and behaves exactly like the old single-resolution path.
pub struct CalibrationPlan {
    /// Session index for each input light, in caller order.
    pub assignments: Vec<usize>,
    pub sessions: Vec<CalibrationSession>,
    /// The group-level summary carried on the stack card, folded into the
    /// resume checkpoint key, and compared by the source-frame search. For
    /// one session it is that session's summary verbatim; for several it
    /// composes their identities order-independently.
    pub applied: AppliedCalibration,
}

impl CalibrationPlan {
    /// A plan that calibrates nothing, for an automatic mode falling back
    /// after the real plan could not be built. Distinct from mode Off: the
    /// fingerprint says the fallback happened, so a later run whose plan
    /// builds again does not resume an uncalibrated accumulator.
    pub fn without_calibration(lights: usize) -> Self {
        let (masters, mut applied) = calibration_off();
        applied.mode = CalibrationMode::Auto;
        applied.state = "unavailable".into();
        applied.fingerprint = "auto-fallback-uncalibrated".into();
        Self::single(masters, applied, lights)
    }

    fn single(
        masters: seiza_stacking::CalibrationMasters,
        applied: AppliedCalibration,
        lights: usize,
    ) -> Self {
        Self {
            assignments: vec![0; lights],
            sessions: vec![CalibrationSession {
                masters,
                applied: applied.clone(),
            }],
            applied,
        }
    }
}

/// The master set a light's selection would build, as a comparable key.
/// Two lights with equal keys calibrate identically — same coherent frame
/// subsets per kind, therefore the same content-addressed masters. Keys
/// differ across capture sessions (per-night flats), across a changed
/// library, and across the selection horizon of very large libraries.
fn session_key(selected: &CalibrationSelection) -> String {
    let mut parts = Vec::new();
    for (kind, frames) in [
        (CalibrationKind::Bias, &selected.bias),
        (CalibrationKind::Dark, &selected.dark),
        (CalibrationKind::DarkFlat, &selected.dark_flat),
        (CalibrationKind::Flat, &selected.flat),
    ] {
        let subset = coherent_master_subset(kind, frames);
        let mut values = subset
            .iter()
            .map(|frame| format!("{}:{}", frame.frame_uuid, frame.source_fingerprint))
            .collect::<Vec<_>>();
        values.sort();
        parts.push(format!("{}={}", kind.as_str(), values.join(",")));
    }
    hex_digest(&parts.join("\u{1e}"))
}

/// Resolve every session a group of lights needs, building masters once per
/// session. `pinned` carries per-session details recorded on an existing
/// stack, so a reproduction (the source-frame search) reuses each session's
/// fitted pedestal instead of fitting its own.
pub fn resolve_or_build_master_plan(
    conn: &Connection,
    cache_root: &Path,
    light_paths: &[PathBuf],
    directory_tree: Option<&crate::directory_tree::DirectoryTree>,
    cancel: Option<&AtomicBool>,
    mode: CalibrationMode,
    pinned: &[CalibrationSessionDetail],
) -> Result<CalibrationPlan> {
    if mode == CalibrationMode::Off {
        let (masters, applied) = calibration_off();
        return Ok(CalibrationPlan::single(masters, applied, light_paths.len()));
    }
    if light_paths.is_empty() {
        return Ok(CalibrationPlan::single(
            seiza_stacking::CalibrationMasters::default(),
            AppliedCalibration::default(),
            0,
        ));
    }

    // Partition by the masters each light would build. The key derivation
    // mirrors the build (selection, remap, coherent subset), so every light
    // in a partition resolves to the identical master set whichever of them
    // acts as the reference.
    let mut assignments = Vec::with_capacity(light_paths.len());
    let mut session_lights: Vec<Vec<PathBuf>> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    for path in light_paths {
        let light = crate::commands::import::headers::read_frame_meta(path);
        let mut selected = select_for_light(conn, &light)?;
        remap_missing_sources(&mut selected, directory_tree);
        let key = session_key(&selected);
        let index = match keys.iter().position(|existing| existing == &key) {
            Some(index) => index,
            None => {
                keys.push(key);
                session_lights.push(Vec::new());
                keys.len() - 1
            }
        };
        assignments.push(index);
        session_lights[index].push(path.clone());
    }

    let mut sessions = Vec::with_capacity(session_lights.len());
    for lights in &session_lights {
        stop_requested(cancel)?;
        // A session's pin is looked up by its selection fingerprint, which
        // the resolution below recomputes identically from the same lights.
        let fingerprint = selection_fingerprint(conn, &lights[0], directory_tree)?;
        let pinned_pedestal = pinned
            .iter()
            .find(|detail| detail.fingerprint == fingerprint)
            .and_then(|detail| detail.estimated_pedestal_adu);
        let (masters, applied) = resolve_or_build_masters_pinned(
            conn,
            cache_root,
            lights,
            directory_tree,
            cancel,
            mode,
            pinned_pedestal,
        )?;
        sessions.push(CalibrationSession { masters, applied });
    }

    let applied = if sessions.len() == 1 {
        sessions[0].applied.clone()
    } else {
        compose_group_summary(&sessions, mode)
    };
    Ok(CalibrationPlan {
        assignments,
        sessions,
        applied,
    })
}

/// The group-level summary for a multi-session plan. Identities compose
/// order-independently so a reordering of the group's lights cannot force a
/// restack.
fn compose_group_summary(
    sessions: &[CalibrationSession],
    mode: CalibrationMode,
) -> AppliedCalibration {
    let mut details = sessions
        .iter()
        .map(|session| CalibrationSessionDetail {
            fingerprint: session.applied.fingerprint.clone(),
            masters_signature: session.applied.masters_signature.clone(),
            estimated_pedestal_adu: session.applied.estimated_pedestal_adu,
            lights: session
                .applied
                .session_details
                .first()
                .map(|detail| detail.lights)
                .unwrap_or(0),
        })
        .collect::<Vec<_>>();
    details.sort_by(|left, right| left.fingerprint.cmp(&right.fingerprint));

    let fingerprint = hex_digest(
        &details
            .iter()
            .map(|detail| detail.fingerprint.as_str())
            .collect::<Vec<_>>()
            .join("\u{1e}"),
    );
    let masters_signature = details
        .iter()
        .map(|detail| detail.masters_signature.as_str())
        .collect::<Vec<_>>()
        .join("||");

    let mut warning: Option<String> = None;
    for (index, session) in sessions.iter().enumerate() {
        if let Some(session_warning) = &session.applied.warning {
            let line = format!("Session {}: {session_warning}", index + 1);
            warning = Some(match warning.take() {
                Some(previous) => format!("{previous}. {line}"),
                None => line,
            });
        }
    }

    let any_applied = sessions
        .iter()
        .any(|session| session.applied.state == "applied");
    AppliedCalibration {
        mode,
        state: if any_applied { "applied" } else { "incomplete" }.into(),
        bias_frames: sessions.iter().map(|s| s.applied.bias_frames).sum(),
        dark_frames: sessions.iter().map(|s| s.applied.dark_frames).sum(),
        dark_flat_frames: sessions.iter().map(|s| s.applied.dark_flat_frames).sum(),
        flat_frames: sessions.iter().map(|s| s.applied.flat_frames).sum(),
        bias_master: None,
        dark_master: None,
        dark_flat_master: None,
        flat_master: None,
        warning,
        fingerprint,
        masters_signature,
        estimated_pedestal_adu: None,
        sessions: sessions.len(),
        session_details: details,
    }
}

pub fn resolve_or_build_masters_for_group(
    conn: &Connection,
    cache_root: &Path,
    light_paths: &[PathBuf],
    directory_tree: Option<&crate::directory_tree::DirectoryTree>,
    cancel: Option<&AtomicBool>,
    mode: CalibrationMode,
) -> Result<(seiza_stacking::CalibrationMasters, AppliedCalibration)> {
    let mut plan = resolve_or_build_master_plan(
        conn,
        cache_root,
        light_paths,
        directory_tree,
        cancel,
        mode,
        &[],
    )?;
    let masters = if plan.sessions.len() == 1 {
        plan.sessions.remove(0).masters
    } else {
        // Callers of this single-masters convenience cannot calibrate per
        // session; the plan API is the one that can.
        seiza_stacking::CalibrationMasters::default()
    };
    Ok((masters, plan.applied))
}

struct BuiltMaster {
    path: PathBuf,
    master_uuid: String,
    /// Inputs the integrator refused, which only it can see: it reads the
    /// headers, while selection has only what the catalog recorded.
    skipped: Vec<(PathBuf, String)>,
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
    recording_blocker: Option<&str>,
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
        let recorded_version: Option<i64> = conn
            .query_row(
                "SELECT cache_version FROM psf_guard_calibration_master WHERE cache_path = ?1",
                [path.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .optional()?;
        // The version is what makes the cache honest: a master written by an
        // older generation is not the artifact this build would produce from
        // the same sources, so it is a miss, not a hit. Files predating
        // metadata preservation sat in this cache validating as "nothing
        // recorded, nothing to check" for a week because reuse never asked.
        // The exception is a blocked recorder: it cannot rebuild, and an old
        // master under today's validation still beats no master at all.
        let row_current = recorded_version == Some(i64::from(MASTER_CACHE_VERSION));
        let file_valid = crate::image_io::open_linear_frame(&path)
            .and_then(|frame| frame.validate_master_kind(expected_kind))
            .is_ok();
        if recorded_version.is_some() && file_valid && (row_current || recording_blocker.is_some())
        {
            if !row_current {
                tracing::info!(
                    "serving generation-{} master {} without rebuilding: recording is blocked",
                    recorded_version.unwrap_or_default(),
                    path.display()
                );
            }
            return Ok(Some(BuiltMaster {
                path,
                master_uuid,
                // A cached master is served without rebuilding, so there is
                // no fresh refusal to report.
                skipped: Vec::new(),
            }));
        }
        if let Some(blocker) = recording_blocker {
            anyhow::bail!(
                "{blocker}; no valid recorded cached {} master exists",
                kind.as_str()
            );
        }
        std::fs::remove_file(&path)
            .with_context(|| format!("removing stale master {}", path.display()))?;
    }
    if let Some(blocker) = recording_blocker {
        anyhow::bail!(
            "{blocker}; no recorded cached {} master exists",
            kind.as_str()
        );
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
        // A defective sensor pixel repeats in every flat, so across-frame
        // clipping keeps it and it would divide every light forever. The
        // flat's true response is smooth at pixel scale, so a spatial pass
        // is safe there. Darks and dark-flats must KEEP their hot pixels —
        // they are what subtracts them from the frames they calibrate.
        defect_suppression: (kind == CalibrationKind::Flat)
            .then(seiza_stacking::ImpulseFilterOptions::default),
        ..Default::default()
    };
    let paths = frames
        .iter()
        .map(|frame| frame.source_path.clone())
        .collect::<Vec<_>>();
    let frame = seiza_stacking::build_master_from_fits(&paths, seiza_kind, &options)
        .with_context(|| format!("building master {}", kind.as_str()))?;
    let skipped: Vec<(PathBuf, String)> = frame
        .skipped_inputs
        .iter()
        .map(|skipped| (skipped.path.clone(), skipped.reason.clone()))
        .collect();
    for (path, reason) in &skipped {
        tracing::warn!(
            "master {}: left out {} — {reason}",
            kind.as_str(),
            path.display()
        );
    }
    if frame.defect_pixels_replaced > 0 {
        tracing::info!(
            "master {} suppressed {} defective pixel(s)",
            kind.as_str(),
            frame.defect_pixels_replaced
        );
    }
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
    Ok(Some(BuiltMaster {
        path,
        master_uuid,
        skipped,
    }))
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

/// Seiza's signature plus the one setting it has no slot for: the readout
/// mode's name.
///
/// `seiza-calibration` knows the readout mode as an integer. N.I.N.A. writes
/// it as a display name, so on those rigs the integer is never known and
/// Seiza alone would let a "High Gain Mode" dark serve an "Extend Fullwell"
/// light. The name rides alongside and is compared with Seiza's own rules
/// for a known-or-unknown setting.
struct Signature {
    seiza: seiza_calibration::FrameSignature,
    readout_mode_name: Option<String>,
}

/// Seiza's `sensor_matches`, plus the readout mode's name: a reference that
/// names one is not matched by a candidate that names another or none.
fn signature_sensor_matches(reference: &Signature, candidate: &Signature) -> bool {
    seiza_calibration::sensor_matches(&reference.seiza, &candidate.seiza)
        && match (&reference.readout_mode_name, &candidate.readout_mode_name) {
            (Some(reference), Some(candidate)) => readout_names_equal(reference, candidate),
            (Some(_), None) => false,
            (None, _) => true,
        }
}

/// Seiza's `sensor_consistent`, plus the readout mode's name: two named
/// modes must agree, an unnamed one on either side is tolerated.
fn signature_sensor_consistent(left: &Signature, right: &Signature) -> bool {
    seiza_calibration::sensor_consistent(&left.seiza, &right.seiza)
        && match (&left.readout_mode_name, &right.readout_mode_name) {
            (Some(left), Some(right)) => readout_names_equal(left, right),
            _ => true,
        }
}

fn readout_names_equal(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

/// Everything Seiza's matcher needs from a light frame's headers.
///
/// PSF Guard's own records carry more — catalog identity, paths, grades — and
/// none of it decides whether two frames belong together. That question is
/// `seiza-calibration`'s, and these two adapters are the whole of what it
/// takes to ask it.
fn light_signature(light: &FrameMeta) -> Signature {
    let mut signature = seiza_calibration::FrameSignature::default();
    signature.camera = light.camera.clone();
    signature.telescope = light.telescope.clone();
    signature.bayer_pattern = light.bayer_pattern.clone();
    signature.filter = light.filter.clone();
    signature.width = light.width;
    signature.height = light.height;
    signature.channels = light.channels;
    signature.binning_x = light.binning_x;
    signature.binning_y = light.binning_y;
    signature.gain = light.gain;
    signature.offset = light.offset;
    signature.readout_mode = light.readout_mode;
    signature.focal_length_mm = light.focal_length_mm;
    signature.rotation_deg = light.rotator_position;
    signature.exposure_seconds = light.exposure_s;
    signature.camera_temp_c = light.camera_temp;
    signature.captured_at_unix = light.timestamp;
    Signature {
        seiza: signature,
        readout_mode_name: light.readout_mode_name.clone(),
    }
}

/// The same, for a calibration frame already in the catalog.
fn frame_signature(frame: &CalibrationFrame) -> Signature {
    let mut signature = seiza_calibration::FrameSignature::default();
    signature.camera = frame.camera.clone();
    signature.telescope = frame.telescope.clone();
    signature.bayer_pattern = frame.bayer_pattern.clone();
    signature.filter = frame.filter.clone();
    signature.width = frame.width;
    signature.height = frame.height;
    signature.channels = frame.channels;
    signature.binning_x = frame.binning_x;
    signature.binning_y = frame.binning_y;
    signature.gain = frame.gain;
    signature.offset = frame.offset;
    signature.readout_mode = frame.readout_mode;
    signature.focal_length_mm = frame.focal_length_mm;
    signature.rotation_deg = frame.rotation;
    signature.exposure_seconds = frame.exposure_s;
    signature.camera_temp_c = frame.camera_temp;
    signature.captured_at_unix = frame.captured_at;
    Signature {
        seiza: signature,
        readout_mode_name: frame.readout_mode_name.clone(),
    }
}

/// Configured rotation tolerance in f64 bits; zero means "not configured".
/// Process-wide because the tolerance is a deployment's property, set once
/// from `[calibration]` in the config before any matching runs. Threading it
/// as a parameter would touch every selection, clustering and build path for
/// a value that never changes after startup.
static ROTATION_TOLERANCE_BITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Override the rotator-angle tolerance for every match this process makes.
/// Call once at startup, before any catalog is opened. `None` keeps the
/// library default.
pub fn configure_rotation_tolerance(degrees: Option<f64>) {
    let bits = match degrees {
        Some(value) if value.is_finite() && value >= 0.0 => value.to_bits(),
        Some(other) => {
            tracing::warn!("ignoring rotation tolerance {other}: not a non-negative number");
            return;
        }
        None => 0,
    };
    ROTATION_TOLERANCE_BITS.store(bits, std::sync::atomic::Ordering::Relaxed);
}

fn tolerances() -> seiza_calibration::MatchTolerances {
    let mut tolerances = seiza_calibration::MatchTolerances::default();
    let bits = ROTATION_TOLERANCE_BITS.load(std::sync::atomic::Ordering::Relaxed);
    if bits != 0 {
        tolerances.rotation_deg = f64::from_bits(bits);
    }
    tolerances
}

fn sensor_matches(light: &FrameMeta, candidate: &CalibrationFrame) -> bool {
    signature_sensor_matches(&light_signature(light), &frame_signature(candidate))
}

fn flat_matches(light: &FrameMeta, candidate: &CalibrationFrame) -> bool {
    seiza_calibration::optics_match(
        &light_signature(light).seiza,
        &frame_signature(candidate).seiza,
        &tolerances(),
    )
}

fn frame_pair_matches(left: &CalibrationFrame, right: &CalibrationFrame) -> bool {
    signature_sensor_matches(&frame_signature(left), &frame_signature(right))
}

/// Whether a dark's exposure suits what it would be subtracted from.
///
/// Seiza reads this off a pair of signatures, and the callers here have only
/// the two numbers, so the numbers become signatures. Worth the allocation to
/// keep one answer to "are these the same exposure": the rule is a floor or a
/// proportion of the longer exposure, whichever is larger, and reproducing
/// that here is how the two drifted apart in the first place.
fn exposure_matches(left: Option<f64>, right: Option<f64>) -> bool {
    let signature = |exposure| {
        let mut signature = seiza_calibration::FrameSignature::default();
        signature.exposure_seconds = exposure;
        signature
    };
    seiza_calibration::exposure_matches(&signature(left), &signature(right), &tolerances())
}

fn temperature_matches(left: Option<f64>, right: Option<f64>) -> bool {
    let signature = |temperature| {
        let mut signature = seiza_calibration::FrameSignature::default();
        signature.camera_temp_c = temperature;
        signature
    };
    seiza_calibration::temperature_matches(&signature(left), &signature(right), &tolerances())
}

/// The coherent subset of verified, nearest-first candidates that can
/// actually feed one master, chosen at BUILD time so it never touches the
/// per-light selection fingerprint (per-light trimming split fingerprints
/// across multi-night stack groups, which refuse mixed selections).
///
/// See [`master_subset_report`] for what "coherent" has to mean here.
fn coherent_master_subset(
    kind: CalibrationKind,
    frames: &[CalibrationFrame],
) -> Vec<CalibrationFrame> {
    master_subset_report(kind, frames).kept
}

/// The frames that will feed one master, and how many were set aside because
/// the integrator would have refused them.
struct MasterSubset {
    kept: Vec<CalibrationFrame>,
    /// Frames that clustered by temperature and session but that the master
    /// builder would reject outright, so they were dropped instead.
    dropped: Vec<CalibrationFrame>,
}

/// Reduce candidates to the frames that can actually combine into one master.
///
/// Two rules apply, and only the first used to. `seiza-calibration` anchors a
/// cluster on each candidate in turn and takes the first with enough frames to
/// build, so a stray single flat shot near the lights does not orphan a
/// complete session from a week earlier. That rule knows about temperature,
/// session and angle.
///
/// It does not know about the filter, and neither did this function, so a
/// night's flats — one rotator angle, one temperature, five filters — clustered
/// as one coherent set. The integrator then compared each frame against the
/// first and refused the whole master on the first mismatch, which cost a
/// perfectly good flat master because a single frame from another filter had
/// been shot minutes earlier.
///
/// So candidates now also have to pass the integrator's own admission test,
/// against the same anchor the integrator will use: its first frame. Seiza owns
/// both halves of the rule, and asking it the second question here means an odd
/// frame is set aside rather than taken as grounds to abandon the master.
fn master_subset_report(kind: CalibrationKind, frames: &[CalibrationFrame]) -> MasterSubset {
    let signatures: Vec<_> = frames.iter().map(frame_signature).collect();
    let seiza_signatures: Vec<_> = signatures
        .iter()
        .map(|signature| signature.seiza.clone())
        .collect();
    let role = if kind == CalibrationKind::Flat {
        seiza_calibration::FrameRole::Flat
    } else {
        seiza_calibration::FrameRole::Other
    };
    let clustered = seiza_calibration::coherent_subset_indices(
        &seiza_signatures,
        role,
        MIN_MASTER_FRAMES,
        &tolerances(),
    );

    // The integrator takes its first frame as the reference and measures every
    // later one against it, so anchor on the same frame it will. Candidates
    // arrive nearest-first, which makes that the frame closest to the lights.
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    let mut anchor: Option<usize> = None;
    for index in clustered {
        match anchor {
            None => {
                anchor = Some(index);
                kept.push(frames[index].clone());
            }
            Some(anchor) if integrates_with(kind, &signatures[anchor], &signatures[index]) => {
                kept.push(frames[index].clone());
            }
            Some(_) => dropped.push(frames[index].clone()),
        }
    }
    MasterSubset { kept, dropped }
}

/// Whether the master builder would accept `candidate` into a set anchored on
/// `reference`. This asks Seiza the same questions `seiza-stacking` asks while
/// integrating, so selection and integration cannot disagree about one frame.
fn integrates_with(kind: CalibrationKind, reference: &Signature, candidate: &Signature) -> bool {
    if !signature_sensor_consistent(reference, candidate) {
        return false;
    }
    // Only a flat records an optical path; a bias or a dark does not care
    // which filter was in the way.
    kind != CalibrationKind::Flat
        || seiza_calibration::optics_consistent(&reference.seiza, &candidate.seiza, &tolerances())
}

fn sort_candidates(frames: &mut [CalibrationFrame], reference_at: Option<i64>) {
    frames.sort_by_key(|frame| match (reference_at, frame.captured_at) {
        (Some(reference), Some(captured)) => reference.abs_diff(captured),
        (Some(_), None) => u64::MAX,
        _ => 0,
    });
}

fn query_kind(conn: &Connection, kind: CalibrationKind) -> Result<Vec<CalibrationFrame>> {
    // See `select_for_light`: an un-upgraded catalog has no rotation column.
    if !schema_supports_current_reads(conn) {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        r#"
        SELECT id, frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
               captured_at, telescope, camera, width, height, channels,
               binning_x, binning_y, gain, offset, readout_mode, bayer_pattern,
               exposure_s, camera_temp, filter_name, focal_length_mm, rotation,
               valid_direction, readout_mode_name
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
        rotation: row.get(22)?,
        valid_direction: ValidDirection::from_db(row.get(23)?),
        readout_mode_name: row.get(24)?,
        source_verified: false,
    })
}

/// Whether a candidate's validity boundary admits this light. A frame or
/// light with no recorded time cannot be judged and matches, the same way
/// an unrecorded angle matches any angle.
fn validity_admits(candidate: &CalibrationFrame, light_timestamp: Option<i64>) -> bool {
    let (Some(direction), Some(light), Some(frame)) = (
        candidate.valid_direction,
        light_timestamp,
        candidate.captured_at,
    ) else {
        return true;
    };
    match direction {
        ValidDirection::Forward => light >= frame,
        ValidDirection::Backward => light <= frame,
    }
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
    // The catalog row is the reference and the file on disk the candidate:
    // a file that does not record a setting cannot prove it is this frame.
    let hard_matches = signature_sensor_matches(&frame_signature(frame), &light_signature(&meta));
    if !hard_matches {
        return false;
    }
    match frame.kind {
        CalibrationKind::Bias => true,
        CalibrationKind::Dark | CalibrationKind::DarkFlat => {
            exposure_matches(frame.exposure_s, meta.exposure_s)
                && temperature_matches(frame.camera_temp, meta.camera_temp)
        }
        // Filter, telescope and focal length, as before — and now the
        // rotator angle too, which the written-out version here omitted. A
        // flat shot at a different angle is not the frame this row describes,
        // and an angle neither side recorded still matches.
        CalibrationKind::Flat => seiza_calibration::optics_match(
            &frame_signature(frame).seiza,
            &light_signature(&meta).seiza,
            &tolerances(),
        ),
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

    /// A catalog exactly as an older build left it: the tables, the version
    /// row saying 1, and no `rotation` column.
    fn version_one_catalog() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE psf_guard_calibration_schema (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                version   INTEGER NOT NULL
            );
            INSERT INTO psf_guard_calibration_schema (singleton, version) VALUES (1, 1);
            CREATE TABLE psf_guard_calibration_frame (
                id                 INTEGER PRIMARY KEY AUTOINCREMENT,
                frame_uuid         TEXT NOT NULL UNIQUE,
                rig_uuid           TEXT NOT NULL,
                kind               TEXT NOT NULL,
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
            "#,
        )
        .unwrap();
        conn
    }

    fn columns(conn: &Connection, table: &str) -> Vec<String> {
        table_column_names(conn, table).unwrap()
    }

    #[test]
    fn opening_a_version_one_catalog_upgrades_it_in_place() {
        let conn = version_one_catalog();
        assert!(!columns(&conn, "psf_guard_calibration_frame")
            .iter()
            .any(|name| name == "rotation"));

        assert!(migrate_existing(&conn).unwrap(), "there was work to do");

        assert!(columns(&conn, "psf_guard_calibration_frame")
            .iter()
            .any(|name| name == "rotation"));
        assert!(schema_supports_current_reads(&conn));
        let version: i64 = conn
            .query_row(
                "SELECT version FROM psf_guard_calibration_schema WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, CALIBRATION_SCHEMA_VERSION);
    }

    #[test]
    fn the_upgrade_is_what_lets_a_stack_read_the_library() {
        // The reported failure: `no such column: rotation`, raised from
        // inside a stack build against a catalog an older build wrote.
        let conn = version_one_catalog();
        let light = frame("/lights/one.fits", "LIGHT");
        assert!(
            select_for_light(&conn, &light).is_ok(),
            "an un-upgraded catalog must report no calibration, not fail"
        );
        assert!(select_for_light(&conn, &light).unwrap().bias.is_empty());

        migrate_existing(&conn).unwrap();
        assert!(select_for_light(&conn, &light).is_ok());
        assert!(query_kind(&conn, CalibrationKind::Flat).is_ok());
    }

    #[test]
    fn upgrading_twice_is_a_no_op() {
        let conn = version_one_catalog();
        assert!(migrate_existing(&conn).unwrap());
        assert!(!migrate_existing(&conn).unwrap(), "nothing left to do");
        assert!(!migrate_existing(&conn).unwrap());
    }

    #[test]
    fn a_half_upgraded_catalog_finishes_the_job() {
        // An older build added the column from its own write path without
        // recording a version, so the step must survive being run over it.
        let conn = version_one_catalog();
        conn.execute_batch(
            "ALTER TABLE psf_guard_calibration_frame ADD COLUMN rotation REAL;
             ALTER TABLE psf_guard_calibration_frame ADD COLUMN valid_direction TEXT;
             ALTER TABLE psf_guard_calibration_frame ADD COLUMN readout_mode_name TEXT;",
        )
        .unwrap();
        assert!(
            schema_supports_current_reads(&conn),
            "the physical shape is readable even while its version lags"
        );
        assert!(migrate_existing(&conn).unwrap());
        assert!(schema_supports_current_reads(&conn));
    }

    #[test]
    fn a_current_version_row_does_not_hide_schema_drift() {
        let conn = version_one_catalog();
        conn.execute(
            "UPDATE psf_guard_calibration_schema SET version = ?1 WHERE singleton = 1",
            [CALIBRATION_SCHEMA_VERSION],
        )
        .unwrap();
        assert!(!schema_supports_current_reads(&conn));

        assert!(
            migrate_existing(&conn).unwrap(),
            "the missing column was repaired"
        );
        assert!(schema_supports_current_reads(&conn));
        assert!(
            !migrate_existing(&conn).unwrap(),
            "the repair is idempotent"
        );
    }

    #[test]
    fn a_catalog_with_no_psf_guard_tables_is_left_alone() {
        // Opening someone's scheduler database must not write to it.
        let conn = Connection::open_in_memory().unwrap();
        assert!(!migrate_existing(&conn).unwrap());
        assert!(!schema_exists(&conn));
        assert!(!schema_supports_current_reads(&conn));
    }

    #[test]
    fn a_fresh_catalog_is_created_at_the_current_version() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        assert!(schema_supports_current_reads(&conn));
        assert!(!migrate_existing(&conn).unwrap(), "nothing to upgrade");
    }

    #[test]
    fn every_version_this_build_claims_has_a_step_that_reaches_it() {
        // The constant and the ladder live far apart. Bumping one without the
        // other would log a successful upgrade on every open while the catalog
        // stayed behind, and calibration would quietly vanish.
        assert_eq!(
            MIGRATIONS.last().map(|migration| migration.to_version),
            Some(CALIBRATION_SCHEMA_VERSION),
            "the last rung must reach the version this build claims"
        );
        let mut previous = 1;
        for migration in MIGRATIONS {
            assert!(
                migration.to_version > previous,
                "rungs must ascend; run_migrations walks them in order"
            );
            previous = migration.to_version;
        }
    }

    #[test]
    fn a_stale_migrator_cannot_move_the_version_backward() {
        let conn = version_one_catalog();
        let newer_version = CALIBRATION_SCHEMA_VERSION + 1;
        conn.execute(
            "UPDATE psf_guard_calibration_schema SET version = ?1 WHERE singleton = 1",
            [newer_version],
        )
        .unwrap();

        let (observed, advanced) =
            advance_schema_version(&conn, CALIBRATION_SCHEMA_VERSION).unwrap();
        assert!(!advanced);
        assert_eq!(observed, newer_version);
        assert_eq!(recorded_schema_version(&conn).unwrap(), newer_version);
    }

    #[test]
    fn a_catalog_that_has_the_columns_reads_even_with_a_stale_version() {
        // The state real catalogs are in: an earlier build added the column
        // from its write path and never recorded a version. Gating reads on
        // the version row would report no calibration for a catalog that is
        // perfectly readable, including where it cannot be upgraded at all.
        let conn = version_one_catalog();
        conn.execute_batch(
            "ALTER TABLE psf_guard_calibration_frame ADD COLUMN rotation REAL;
             ALTER TABLE psf_guard_calibration_frame ADD COLUMN valid_direction TEXT;
             ALTER TABLE psf_guard_calibration_frame ADD COLUMN readout_mode_name TEXT;",
        )
        .unwrap();
        assert!(
            schema_supports_current_reads(&conn),
            "the columns are all there"
        );
        let light = frame("/lights/one.fits", "LIGHT");
        assert!(select_for_light(&conn, &light).is_ok());
        assert!(query_kind(&conn, CalibrationKind::Flat).is_ok());
    }

    #[test]
    fn a_catalog_missing_the_columns_reports_absent_rather_than_failing() {
        let conn = version_one_catalog();
        assert!(!schema_supports_current_reads(&conn));
        let light = frame("/lights/one.fits", "LIGHT");
        assert!(select_for_light(&conn, &light).unwrap().bias.is_empty());
    }

    #[test]
    fn two_openers_racing_the_same_upgrade_both_succeed() {
        // Both pass the column check, both try the ALTER, SQLite serializes
        // them and tells the loser the column already exists. That is the
        // state we wanted, so it is not a failure.
        let conn = version_one_catalog();
        add_column_if_missing(&conn, "psf_guard_calibration_frame", "rotation", "REAL").unwrap();
        assert!(
            add_column_if_missing(&conn, "psf_guard_calibration_frame", "rotation", "REAL").is_ok(),
            "the loser of the race must not report an upgrade failure"
        );
    }

    #[test]
    fn a_catalog_from_a_newer_build_is_refused_rather_than_guessed_at() {
        let conn = version_one_catalog();
        // A newer build would have run every step this one knows about, so the
        // columns are there; it simply also knows steps this build does not.
        conn.execute_batch(
            "ALTER TABLE psf_guard_calibration_frame ADD COLUMN rotation REAL;
             ALTER TABLE psf_guard_calibration_frame ADD COLUMN valid_direction TEXT;
             ALTER TABLE psf_guard_calibration_frame ADD COLUMN readout_mode_name TEXT;",
        )
        .unwrap();
        conn.execute(
            "UPDATE psf_guard_calibration_schema SET version = ?1 WHERE singleton = 1",
            [CALIBRATION_SCHEMA_VERSION + 1],
        )
        .unwrap();
        let error = migrate_existing(&conn).unwrap_err().to_string();
        assert!(error.contains("newer than this build"), "{error}");
        // Refusing to upgrade it is not the same as refusing to read it.
        // Steps only add columns, so every column this build names is there.
        assert!(schema_supports_current_reads(&conn));
        let light = frame("/lights/one.fits", "LIGHT");
        assert!(select_for_light(&conn, &light).is_ok());
    }

    #[test]
    fn a_version_row_cannot_hide_a_missing_required_column() {
        let conn = version_one_catalog();
        conn.execute(
            "UPDATE psf_guard_calibration_schema SET version = ?1 WHERE singleton = 1",
            [CALIBRATION_SCHEMA_VERSION + 1],
        )
        .unwrap();

        assert!(!schema_supports_current_reads(&conn));
        let light = frame("/lights/one.fits", "LIGHT");
        let selection = select_for_light(&conn, &light).unwrap();
        assert!(selection.bias.is_empty());
        assert!(query_kind(&conn, CalibrationKind::Flat).unwrap().is_empty());
    }

    fn fits_card(output: &mut Vec<u8>, value: &str) {
        let mut card = value.as_bytes().to_vec();
        card.resize(80, b' ');
        output.extend(card);
    }

    #[test]
    fn upgrading_recovers_the_rotator_angle_the_catalog_never_recorded() {
        // Adding the `rotation` column left every existing row NULL, and NULL
        // reads as "no angle was written down", which matches any angle. A
        // library filled before the column existed therefore looked like one
        // where the rotator had never moved, and flats from nights an angle
        // apart were offered as one set. The files knew all along.
        let temp = tempfile::tempdir().unwrap();
        let flat = temp.path().join("flat.fits");
        let mut header = Vec::new();
        fits_card(&mut header, "SIMPLE  =                    T");
        fits_card(&mut header, "BITPIX  =                   16");
        fits_card(&mut header, "NAXIS   =                    2");
        fits_card(&mut header, "NAXIS1  =                    4");
        fits_card(&mut header, "NAXIS2  =                    4");
        fits_card(&mut header, "IMAGETYP= 'FLAT'");
        fits_card(&mut header, "ROTATANG=      101.98999786377");
        fits_card(&mut header, "END");
        header.resize(header.len().div_ceil(2880) * 2880, b' ');
        let mut payload = vec![0_u8; 2880];
        payload[0] = 1;
        let mut file = std::fs::File::create(&flat).unwrap();
        file.write_all(&header).unwrap();
        file.write_all(&payload).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO psf_guard_calibration_frame
               (frame_uuid, rig_uuid, kind, source_path, source_fingerprint, rotation, added_at, updated_at)
             VALUES ('u', 'r', 'flat', ?1, 'fp', NULL, 0, 0)",
            [flat.to_string_lossy().as_ref()],
        )
        .unwrap();
        // A flat whose file has gone must survive the upgrade, not stop it.
        conn.execute(
            "INSERT INTO psf_guard_calibration_frame
               (frame_uuid, rig_uuid, kind, source_path, source_fingerprint, rotation, added_at, updated_at)
             VALUES ('gone', 'r', 'flat', '/nowhere/missing.fits', 'fp', NULL, 0, 0)",
            [],
        )
        .unwrap();

        // The rung runs only while the catalog still claims a version that
        // predates it: on a current catalog every candidate was already
        // visited once, and re-reading headers on each open buys nothing.
        conn.execute(
            "UPDATE psf_guard_calibration_schema SET version = 2 WHERE singleton = 1",
            [],
        )
        .unwrap();
        assert!(backfill_flat_rotation(&conn).unwrap(), "an angle was found");
        let recovered: Option<f64> = conn
            .query_row(
                "SELECT rotation FROM psf_guard_calibration_frame WHERE frame_uuid = 'u'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            recovered.is_some_and(|angle| (angle - 101.99).abs() < 0.01),
            "the angle comes off the file: {recovered:?}"
        );
        let missing: Option<f64> = conn
            .query_row(
                "SELECT rotation FROM psf_guard_calibration_frame WHERE frame_uuid = 'gone'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(missing, None, "a vanished file keeps its NULL");

        // Once the catalog records version 3 the rung is spent: it does not
        // re-read hundreds of headers on every open to learn nothing.
        conn.execute(
            "UPDATE psf_guard_calibration_schema SET version = 3 WHERE singleton = 1",
            [],
        )
        .unwrap();
        assert!(!backfill_flat_rotation(&conn).unwrap());
    }

    #[test]
    fn upgrading_recovers_the_readout_mode_name_the_catalog_never_recorded() {
        // Every frame from a N.I.N.A. rig sat in the catalog with no readout
        // mode at all, because the header spells it as a name and the
        // integer column had no room for one. The files knew all along.
        let temp = tempfile::tempdir().unwrap();
        let dark = temp.path().join("dark.fits");
        let mut header = Vec::new();
        fits_card(&mut header, "SIMPLE  =                    T");
        fits_card(&mut header, "BITPIX  =                   16");
        fits_card(&mut header, "NAXIS   =                    2");
        fits_card(&mut header, "NAXIS1  =                    4");
        fits_card(&mut header, "NAXIS2  =                    4");
        fits_card(&mut header, "IMAGETYP= 'DARK'");
        fits_card(
            &mut header,
            "READOUTM= 'Extend Fullwell 2CMS' / Sensor readout mode",
        );
        fits_card(&mut header, "END");
        header.resize(header.len().div_ceil(2880) * 2880, b' ');
        let mut payload = vec![0_u8; 2880];
        payload[0] = 1;
        let mut file = std::fs::File::create(&dark).unwrap();
        file.write_all(&header).unwrap();
        file.write_all(&payload).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        for (uuid, path) in [
            ("u", dark.to_string_lossy().into_owned()),
            ("gone", "/nowhere/missing.fits".to_string()),
        ] {
            conn.execute(
                "INSERT INTO psf_guard_calibration_frame
                   (frame_uuid, rig_uuid, kind, source_path, source_fingerprint, added_at, updated_at)
                 VALUES (?1, 'r', 'dark', ?2, 'fp', 0, 0)",
                rusqlite::params![uuid, path],
            )
            .unwrap();
        }
        let name_of = |uuid: &str| -> Option<String> {
            conn.query_row(
                "SELECT readout_mode_name FROM psf_guard_calibration_frame WHERE frame_uuid = ?1",
                [uuid],
                |row| row.get(0),
            )
            .unwrap()
        };

        // The rung runs only while the catalog still claims a version that
        // predates it.
        conn.execute(
            "UPDATE psf_guard_calibration_schema SET version = 5 WHERE singleton = 1",
            [],
        )
        .unwrap();
        assert!(
            backfill_readout_mode_name(&conn).unwrap(),
            "a name was found"
        );
        assert_eq!(name_of("u").as_deref(), Some("Extend Fullwell 2CMS"));
        assert_eq!(name_of("gone"), None, "a vanished file keeps its NULL");

        // Once the catalog records version 6 the rung is spent.
        conn.execute(
            "UPDATE psf_guard_calibration_schema SET version = 6 WHERE singleton = 1",
            [],
        )
        .unwrap();
        assert!(!backfill_readout_mode_name(&conn).unwrap());
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

    fn file_backed_bias_library(temp: &tempfile::TempDir) -> (PathBuf, PathBuf, PathBuf) {
        let mut calibration_meta = Vec::new();
        for index in 0..2 {
            let path = temp.path().join(format!("bias-{index}.fits"));
            write_test_fits(&path, "BIAS", 100 + index);
            calibration_meta.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        let light_path = temp.path().join("light.fits");
        write_test_fits(&light_path, "LIGHT", 1_100);

        let database_path = temp.path().join("catalog.sqlite");
        let mut conn = Connection::open(&database_path).unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &calibration_meta, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        drop(conn);

        (database_path, temp.path().join("cache"), light_path)
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
            kind_from_meta(&frame("/flat-dark.fits", "FLAT DARK")),
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
    fn a_long_dark_matches_within_a_proportion_of_its_exposure() {
        // The rule is a floor or a proportion of the longer exposure,
        // whichever is larger. A flat 0.05 s is a six-thousandth of a
        // five-minute sub — tighter than a shutter, and tighter than the rule
        // the master builder applies to the same frames.
        assert!(exposure_matches(Some(300.0), Some(300.25)));
        assert!(!exposure_matches(Some(300.0), Some(301.0)));
        // Below a minute the floor still decides, because a tenth of a
        // percent of half a second is nothing any header records.
        assert!(exposure_matches(Some(0.5), Some(0.52)));
        assert!(!exposure_matches(Some(0.5), Some(0.7)));
    }

    #[test]
    fn a_header_that_reads_as_not_a_number_recorded_nothing() {
        // A NaN temperature is not a temperature. Left in place it would read
        // as "unknown, accepts anything" and pair a light of no known
        // temperature with a dark of any temperature at all — a +20 °C dark
        // under a -10 °C light, silently, which is worse than no dark.
        let mut file = Vec::new();
        fits_card(&mut file, "SIMPLE  =                    T");
        fits_card(&mut file, "BITPIX  =                   16");
        fits_card(&mut file, "NAXIS   =                    2");
        fits_card(&mut file, "NAXIS1  =                  100");
        fits_card(&mut file, "NAXIS2  =                  100");
        fits_card(&mut file, "IMAGETYP= 'LIGHT'");
        fits_card(&mut file, "CCD-TEMP=                  nan");
        fits_card(&mut file, "ROTATANG=                  nan");
        fits_card(&mut file, "END");
        file.resize(file.len().div_ceil(2880) * 2880, b' ');
        file.resize(file.len() + 2880 * 4, 0);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nan.fits");
        std::fs::write(&path, &file).unwrap();
        let meta = crate::commands::import::headers::read_frame_meta(&path);
        assert!(meta.readable, "the frame parses");
        assert_eq!(meta.camera_temp, None, "NaN is absent, not a reading");
        assert_eq!(meta.rotator_position, None);
    }

    #[test]
    fn configured_rotation_tolerance_reaches_every_match() {
        // The knob is process-wide, and these tests share the process, so the
        // probe value differs from the default only by epsilon: enough to
        // prove the configured number is the one used, small enough that no
        // concurrent test's angle comparison can flip on it.
        let probe = seiza_calibration::MatchTolerances::default().rotation_deg + 1.0e-7;
        configure_rotation_tolerance(Some(probe));
        assert_eq!(tolerances().rotation_deg.to_bits(), probe.to_bits());

        // Nonsense is refused and the previous setting stands.
        configure_rotation_tolerance(Some(f64::NAN));
        assert_eq!(tolerances().rotation_deg.to_bits(), probe.to_bits());
        configure_rotation_tolerance(Some(-1.0));
        assert_eq!(tolerances().rotation_deg.to_bits(), probe.to_bits());

        // Unsetting restores the library default.
        configure_rotation_tolerance(None);
        assert_eq!(
            tolerances().rotation_deg,
            seiza_calibration::MatchTolerances::default().rotation_deg
        );
    }

    #[test]
    fn one_odd_filter_does_not_cost_the_whole_flat_master() {
        // Taken from a real night: C925, 2026-08-11. Forty-one flats, one
        // rotator angle, one focal length, one temperature band — and five
        // filters, because the whole wheel was run back to back. A single
        // OIII frame was shot four minutes before the R set.
        //
        // Every one of those frames clusters: same angle, same session, same
        // temperature. The integrator then measured each against its first
        // frame, hit the filter change, and refused the master outright, so a
        // good R flat was lost to one frame from another filter.
        let flat = |id: i64, filter: &str, captured_at: i64| CalibrationFrame {
            id,
            frame_uuid: format!("f{id}"),
            rig_uuid: "r".into(),
            kind: CalibrationKind::Flat,
            source_path: format!("/flat-{id}-{filter}.fits").into(),
            source_fingerprint: "x".into(),
            captured_at: Some(captured_at),
            telescope: Some("C925".into()),
            camera: Some("ZWO ASI2600MM Pro".into()),
            width: Some(6248),
            height: Some(4176),
            channels: Some(1),
            binning_x: Some(1),
            binning_y: Some(1),
            gain: Some(100),
            offset: Some(30),
            readout_mode: None,
            readout_mode_name: None,
            bayer_pattern: None,
            exposure_s: Some(5.0),
            camera_temp: Some(-9.8),
            filter: Some(filter.into()),
            focal_length_mm: Some(2350.0),
            rotation: Some(101.99),
            valid_direction: None,
            source_verified: false,
        };
        // Nearest-first, as selection delivers them: the R set, with the one
        // OIII frame trailing it.
        let mut frames: Vec<CalibrationFrame> = (0..10)
            .map(|id| flat(id, "R", 1_000_000_000 + id))
            .collect();
        frames.push(flat(10, "OIII", 999_999_787));

        let report = master_subset_report(CalibrationKind::Flat, &frames);
        assert_eq!(report.kept.len(), 10, "the ten R flats still make a master");
        assert!(
            report
                .kept
                .iter()
                .all(|frame| frame.filter.as_deref() == Some("R")),
            "a master must not average two filters"
        );
        assert_eq!(
            report.dropped.len(),
            1,
            "the odd frame is set aside, not fatal"
        );
        assert_eq!(report.dropped[0].filter.as_deref(), Some("OIII"));
    }

    #[test]
    fn a_bias_master_does_not_care_which_filter_was_in_the_way() {
        // The optical test is for flats alone. Biases carry a filter name
        // only because the wheel happened to be somewhere; refusing to
        // combine them on that basis would leave a rig with no bias at all.
        let bias = |id: i64, filter: &str| CalibrationFrame {
            id,
            frame_uuid: format!("b{id}"),
            rig_uuid: "r".into(),
            kind: CalibrationKind::Bias,
            source_path: format!("/bias-{id}.fits").into(),
            source_fingerprint: "x".into(),
            captured_at: Some(1_000_000_000 + id),
            telescope: Some("C925".into()),
            camera: Some("ZWO ASI2600MM Pro".into()),
            width: Some(6248),
            height: Some(4176),
            channels: Some(1),
            binning_x: Some(1),
            binning_y: Some(1),
            gain: Some(100),
            offset: Some(30),
            readout_mode: None,
            readout_mode_name: None,
            bayer_pattern: None,
            exposure_s: Some(0.0),
            camera_temp: Some(-10.0),
            filter: Some(filter.into()),
            focal_length_mm: Some(2350.0),
            rotation: Some(101.99),
            valid_direction: None,
            source_verified: false,
        };
        let frames = vec![bias(0, "R"), bias(1, "OIII"), bias(2, "L")];
        let report = master_subset_report(CalibrationKind::Bias, &frames);
        assert_eq!(report.kept.len(), 3);
        assert!(report.dropped.is_empty());
    }

    #[test]
    fn a_frame_from_another_camera_never_joins_a_master() {
        // Sensor disagreement is not tolerated for any kind: averaging two
        // cameras produces a master that describes neither.
        let frame = |id: i64, camera: &str, gain: i64| CalibrationFrame {
            id,
            frame_uuid: format!("d{id}"),
            rig_uuid: "r".into(),
            kind: CalibrationKind::Dark,
            source_path: format!("/dark-{id}.fits").into(),
            source_fingerprint: "x".into(),
            captured_at: Some(1_000_000_000 + id),
            telescope: Some("C925".into()),
            camera: Some(camera.into()),
            width: Some(6248),
            height: Some(4176),
            channels: Some(1),
            binning_x: Some(1),
            binning_y: Some(1),
            gain: Some(gain),
            offset: Some(30),
            readout_mode: None,
            readout_mode_name: None,
            bayer_pattern: None,
            exposure_s: Some(300.0),
            camera_temp: Some(-10.0),
            filter: None,
            focal_length_mm: Some(2350.0),
            rotation: None,
            valid_direction: None,
            source_verified: false,
        };
        let frames = vec![
            frame(0, "ZWO ASI2600MM Pro", 100),
            frame(1, "ZWO ASI2600MM Pro", 100),
            frame(2, "ZWO ASI6200MM Pro", 100),
            frame(3, "ZWO ASI2600MM Pro", 200),
        ];
        let report = master_subset_report(CalibrationKind::Dark, &frames);
        assert_eq!(report.kept.len(), 2, "only the matching pair combines");
        assert_eq!(
            report.dropped.len(),
            2,
            "other camera and other gain are set aside"
        );
    }

    #[test]
    fn flat_master_never_mixes_rotator_angles() {
        let flat = |id: i64, rotation: Option<f64>| CalibrationFrame {
            id,
            frame_uuid: format!("f{id}"),
            rig_uuid: "r".into(),
            kind: CalibrationKind::Flat,
            source_path: format!("/flat-{id}.fits").into(),
            source_fingerprint: "x".into(),
            captured_at: Some(1_000_000_000),
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
            readout_mode_name: None,
            bayer_pattern: None,
            exposure_s: Some(3.0),
            camera_temp: Some(-10.0),
            filter: Some("Ha".into()),
            focal_length_mm: None,
            rotation,
            valid_direction: None,
            source_verified: false,
        };
        // Five flats at 30° and two strays at 120°: the master takes only
        // the coherent angle, because integrating both would average two
        // different vignette orientations into one wrong correction.
        let frames: Vec<CalibrationFrame> = (0..5)
            .map(|id| flat(id, Some(30.0 + 0.1 * id as f64)))
            .chain((5..7).map(|id| flat(id, Some(120.0))))
            .collect();
        let subset = coherent_master_subset(CalibrationKind::Flat, &frames);
        assert_eq!(subset.len(), 5);
        assert!(subset.iter().all(|frame| frame.rotation.unwrap() < 40.0));
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
            readout_mode_name: None,
            bayer_pattern: None,
            exposure_s: Some(300.0),
            camera_temp: Some(0.0),
            filter: None,
            focal_length_mm: None,
            rotation: None,
            valid_direction: None,
            source_verified: false,
        };
        assert!(!sensor_matches(&light, &candidate));
        assert!(!temperature_matches(
            light.camera_temp,
            candidate.camera_temp
        ));
    }

    #[test]
    fn a_named_readout_mode_separates_the_modes_of_one_camera() {
        // N.I.N.A. writes READOUTM as a name, so the integer is unknown on
        // both sides and Seiza alone calls every mode of the camera the same.
        // The name round-trips through import and decides the match.
        let temp = tempfile::tempdir().unwrap();
        let mut darks = Vec::new();
        for (file, name) in [
            ("extend.fits", Some("Extend Fullwell 2CMS")),
            ("highgain.fits", Some("High Gain Mode")),
            ("unnamed.fits", None),
        ] {
            let path = temp.path().join(file);
            std::fs::write(&path, b"dark").unwrap();
            let mut dark = frame(path.to_str().unwrap(), "DARK");
            dark.readout_mode_name = name.map(str::to_string);
            darks.push(dark);
        }
        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &darks, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        let names = |light: &FrameMeta| {
            let mut names: Vec<Option<String>> = select_for_light(&conn, light)
                .unwrap()
                .dark
                .into_iter()
                .map(|dark| dark.readout_mode_name)
                .collect();
            names.sort();
            names
        };

        // A light that names its mode takes only that mode. A dark that
        // recorded no name cannot prove it is the same mode, so it is
        // refused too — Seiza's rule for every other known setting.
        let mut light = frame("/light.fits", "LIGHT");
        light.readout_mode_name = Some("extend fullwell 2cms".into());
        assert_eq!(names(&light), vec![Some("Extend Fullwell 2CMS".into())]);

        // A light that names no mode cannot rule any dark out.
        light.readout_mode_name = None;
        assert_eq!(names(&light).len(), 3);

        // The listing shows the name so the operator can see why.
        let details = library_details(&conn).unwrap();
        assert!(details
            .frames
            .iter()
            .any(|frame| frame.readout_mode_name.as_deref() == Some("High Gain Mode")));
    }

    #[test]
    fn a_master_never_mixes_readout_mode_names() {
        let dark = |id: i64, name: Option<&str>| CalibrationFrame {
            id,
            frame_uuid: format!("d{id}"),
            rig_uuid: "r".into(),
            kind: CalibrationKind::Dark,
            source_path: format!("/dark-{id}.fits").into(),
            source_fingerprint: "x".into(),
            captured_at: Some(1_000_000_000 + id),
            telescope: None,
            camera: Some("Camera".into()),
            width: Some(3000),
            height: Some(2000),
            channels: Some(1),
            binning_x: Some(1),
            binning_y: Some(1),
            gain: Some(100),
            offset: Some(20),
            readout_mode: None,
            readout_mode_name: name.map(str::to_string),
            bayer_pattern: None,
            exposure_s: Some(300.0),
            camera_temp: Some(-10.0),
            filter: None,
            focal_length_mm: None,
            rotation: None,
            valid_direction: None,
            source_verified: false,
        };
        // Five darks in one mode and two strays in another: the two strays
        // are set aside rather than averaged into a master whose read noise
        // and bias level describe neither mode.
        let frames: Vec<CalibrationFrame> = (0..5)
            .map(|id| dark(id, Some("Extend Fullwell 2CMS")))
            .chain((5..7).map(|id| dark(id, Some("High Gain Mode"))))
            .collect();
        let subset = coherent_master_subset(CalibrationKind::Dark, &frames);
        assert_eq!(subset.len(), 5);
        assert!(subset
            .iter()
            .all(|frame| frame.readout_mode_name.as_deref() == Some("Extend Fullwell 2CMS")));

        // A frame that never named its mode is tolerated in either set: two
        // calibration frames are only apart when both say something different.
        let frames: Vec<CalibrationFrame> = (0..4)
            .map(|id| dark(id, Some("Extend Fullwell 2CMS")))
            .chain(std::iter::once(dark(4, None)))
            .collect();
        assert_eq!(
            coherent_master_subset(CalibrationKind::Dark, &frames).len(),
            5
        );
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
        let details = library_details(&conn).unwrap();
        assert_eq!(details.frames.len(), 1);
        assert_eq!(details.frames[0].kind, CalibrationKind::Dark);
    }

    #[test]
    fn a_validity_boundary_is_never_crossed() {
        // A dark shot after a sensor cleaning must never serve a light from
        // before it once marked forward, and the reverse for backward. Both
        // directions return when the mark is cleared.
        let temp = tempfile::tempdir().unwrap();
        let dark_path = temp.path().join("dark.fits");
        std::fs::write(&dark_path, b"dark").unwrap();
        let mut dark = frame(dark_path.to_str().unwrap(), "DARK");
        dark.timestamp = Some(1_000);

        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &[dark], Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        let uuid: String = conn
            .query_row(
                "SELECT frame_uuid FROM psf_guard_calibration_frame",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut older_light = frame("/light.fits", "LIGHT");
        older_light.timestamp = Some(900);
        let mut newer_light = frame("/light.fits", "LIGHT");
        newer_light.timestamp = Some(1_100);

        let selects = |conn: &Connection, light: &FrameMeta| {
            select_for_light(conn, light).unwrap().dark.len() == 1
        };
        assert!(selects(&conn, &older_light));
        assert!(selects(&conn, &newer_light));

        assert_eq!(
            set_frames_validity(
                &conn,
                std::slice::from_ref(&uuid),
                Some(ValidDirection::Forward)
            )
            .unwrap(),
            1
        );
        assert!(!selects(&conn, &older_light), "forward excludes the past");
        assert!(selects(&conn, &newer_light));

        set_frames_validity(
            &conn,
            std::slice::from_ref(&uuid),
            Some(ValidDirection::Backward),
        )
        .unwrap();
        assert!(selects(&conn, &older_light));
        assert!(
            !selects(&conn, &newer_light),
            "backward excludes the future"
        );

        // A light captured exactly at the boundary is admitted either way.
        let mut boundary_light = frame("/light.fits", "LIGHT");
        boundary_light.timestamp = Some(1_000);
        assert!(selects(&conn, &boundary_light));

        set_frames_validity(&conn, std::slice::from_ref(&uuid), None).unwrap();
        assert!(selects(&conn, &older_light));
        assert!(selects(&conn, &newer_light));

        // The listing carries the mark for the UI.
        set_frames_validity(&conn, &[uuid], Some(ValidDirection::Forward)).unwrap();
        let details = library_details(&conn).unwrap();
        assert_eq!(
            details.frames[0].valid_direction,
            Some(ValidDirection::Forward)
        );
    }

    #[test]
    fn forgetting_a_whole_night_removes_every_named_frame_at_once() {
        let temp = tempfile::tempdir().unwrap();
        let mut meta = Vec::new();
        for index in 0..3 {
            let path = temp.path().join(format!("dark-{index}.fits"));
            std::fs::write(&path, format!("dark {index}")).unwrap();
            let mut dark = frame(path.to_str().unwrap(), "DARK");
            dark.timestamp = Some(1_000 + index);
            meta.push(dark);
        }
        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &meta, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        let uuids: Vec<String> = conn
            .prepare("SELECT frame_uuid FROM psf_guard_calibration_frame")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(uuids.len(), 3);

        // Unknown ids remove nothing rather than failing the batch.
        let outcome = forget_frames(
            &mut conn,
            &["missing".to_string(), uuids[0].clone(), uuids[1].clone()],
        )
        .unwrap();
        assert_eq!(outcome.frames_removed, 2);
        assert_eq!(library_summary(&conn).unwrap().frame_count, 1);

        assert_eq!(
            forget_frames(&mut conn, &[]).unwrap().frames_removed,
            0,
            "an empty batch is a no-op"
        );
    }

    #[test]
    fn a_reimport_does_not_clear_a_validity_mark() {
        // Re-scanning a folder updates a row's header-derived columns; the
        // user's mark is not header-derived and must survive.
        let temp = tempfile::tempdir().unwrap();
        let dark_path = temp.path().join("dark.fits");
        std::fs::write(&dark_path, b"dark").unwrap();
        let mut dark = frame(dark_path.to_str().unwrap(), "DARK");
        dark.timestamp = Some(1_000);

        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &[dark.clone()], Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        let uuid: String = conn
            .query_row(
                "SELECT frame_uuid FROM psf_guard_calibration_frame",
                [],
                |row| row.get(0),
            )
            .unwrap();
        set_frames_validity(&conn, &[uuid], Some(ValidDirection::Backward)).unwrap();

        // A changed file takes the upsert's update path.
        std::fs::write(&dark_path, b"dark v2 with more bytes").unwrap();
        dark.timestamp = Some(1_001);
        {
            let tx = conn.transaction().unwrap();
            let outcome = import_calibration_frames(&tx, &[dark], Some("profile")).unwrap();
            assert_eq!(outcome.updated, 1);
            tx.commit().unwrap();
        }
        let details = library_details(&conn).unwrap();
        assert_eq!(
            details.frames[0].valid_direction,
            Some(ValidDirection::Backward)
        );
    }

    #[test]
    fn marking_validity_needs_an_upgraded_catalog() {
        let conn = version_one_catalog();
        let error = set_frames_validity(&conn, &["x".to_string()], None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("has not been upgraded"), "{error}");

        migrate_existing(&conn).unwrap();
        // Upgraded, an unknown frame is simply zero rows changed.
        assert_eq!(
            set_frames_validity(&conn, &["x".to_string()], None).unwrap(),
            0
        );
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
            readout_mode_name: None,
            bayer_pattern: None,
            exposure_s: Some(3.0),
            camera_temp: temp,
            filter: Some("Ha".into()),
            focal_length_mm: None,
            rotation: None,
            valid_direction: None,
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
            std::slice::from_ref(&light_path),
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        assert!(first.flat_master.is_some());

        let (_, second) = resolve_or_build_masters(
            &conn,
            &temp.path().join("cache-b"),
            std::slice::from_ref(&light_path),
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
            std::slice::from_ref(&light_path),
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

    /// A frame with real dimensions and per-pixel values, for tests that
    /// exercise the pedestal fit. BITPIX 16, so values must stay below
    /// `i16::MAX`.
    fn write_gradient_fits(
        path: &Path,
        kind: &str,
        width: usize,
        height: usize,
        camera: &str,
        offset: i64,
        pixel: impl Fn(usize, usize) -> f64,
    ) {
        let mut header = Vec::new();
        fits_card(&mut header, "SIMPLE  =                    T");
        fits_card(&mut header, "BITPIX  =                   16");
        fits_card(&mut header, "NAXIS   =                    2");
        fits_card(&mut header, &format!("NAXIS1  = {width:20}"));
        fits_card(&mut header, &format!("NAXIS2  = {height:20}"));
        fits_card(&mut header, &format!("IMAGETYP= '{kind}'"));
        fits_card(&mut header, "FILTER  = 'Ha'");
        fits_card(&mut header, "EXPTIME =                300.0");
        fits_card(&mut header, "GAIN    =                  100");
        fits_card(&mut header, &format!("OFFSET  = {offset:20}"));
        fits_card(&mut header, "XBINNING=                    1");
        fits_card(&mut header, "YBINNING=                    1");
        fits_card(&mut header, "CCD-TEMP=                -10.0");
        fits_card(&mut header, "TELESCOP= 'Scope'");
        fits_card(&mut header, &format!("INSTRUME= '{camera}'"));
        fits_card(&mut header, "END");
        header.resize(header.len().div_ceil(2880) * 2880, b' ');
        let mut payload = Vec::new();
        for y in 0..height {
            for x in 0..width {
                payload.extend((pixel(x, y) as i16).to_be_bytes());
            }
        }
        payload.resize(payload.len().div_ceil(2880) * 2880, 0);
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(&header).unwrap();
        file.write_all(&payload).unwrap();
    }

    /// A flats-only library with a strong vignette and lights whose
    /// background is `sky * vignette + pedestal`. Returns the connection,
    /// cache directory, and light paths, ready for `resolve_or_build_masters`.
    #[test]
    fn project_report_says_which_nights_have_their_own_flats() {
        // Two nights of lights; flats exist only for the first. The report
        // must mark the first night as covered by same-night flats, the
        // second as using flats from days away, and warn about the missing
        // kinds and the uncovered night.
        let temp = tempfile::tempdir().unwrap();
        let night_one = 1_750_000_000i64; // an evening
        let night_two = night_one + 5 * 86_400;

        let mut calibration_meta = Vec::new();
        for index in 0..2 {
            let path = temp.path().join(format!("flat-{index}.fits"));
            write_test_fits(&path, "FLAT", 1_000 + index);
            let mut meta = crate::commands::import::headers::read_frame_meta(&path);
            meta.timestamp = Some(night_one + 3_600 + index as i64);
            calibration_meta.push(meta);
        }

        let mut lights = Vec::new();
        for (index, timestamp) in [(0, night_one), (1, night_two)] {
            let path = temp.path().join(format!("light-{index}.fits"));
            write_test_fits(&path, "LIGHT", 1_100);
            let mut meta = crate::commands::import::headers::read_frame_meta(&path);
            meta.timestamp = Some(timestamp);
            meta.object = Some("M 31".into());
            meta.ra_deg = Some(10.68);
            meta.dec_deg = Some(41.27);
            lights.push(meta);
        }

        let mut conn = Connection::open_in_memory().unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &calibration_meta, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        let outcome = crate::commands::import::import_frames(
            &mut conn,
            lights,
            &crate::commands::import::ImportOptions::default(),
        )
        .unwrap();
        assert_eq!(outcome.imported, 2);

        let project_id: i32 = conn
            .query_row("SELECT id FROM project LIMIT 1", [], |row| row.get(0))
            .unwrap();
        let tree = crate::directory_tree::DirectoryTree::build_multiple(&[temp.path()]).unwrap();
        let report = project_calibration_report(&conn, project_id, &tree).unwrap();

        assert_eq!(report.nights.len(), 2);
        // Newest night first; it has no same-night flats.
        let newest = &report.nights[0];
        let oldest = &report.nights[1];
        assert!(newest.night > oldest.night);
        let newest_filter = &newest.filters[0];
        assert!(!newest_filter.nightly_flats);
        assert_eq!(newest_filter.flat_frames, 2);
        assert!(newest_filter.flat_age_days.is_some_and(|age| age > 4.0));
        let oldest_filter = &oldest.filters[0];
        assert!(oldest_filter.nightly_flats);
        assert!(oldest_filter.flat_age_days.is_some_and(|age| age < 1.0));
        assert!(oldest_filter.missing.contains(&"bias".to_string()));

        let flat_summary = report
            .kinds
            .iter()
            .find(|summary| summary.kind == "flat")
            .unwrap();
        assert_eq!(flat_summary.matching_frames, 2);
        assert_eq!(flat_summary.sessions.len(), 1);
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("No bias frames")));
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("1 of 2 nights have no same-night flats")));
        assert_eq!(report.lights_missing_files, 0);
    }

    fn vignetted_flat_only_library(
        temp: &tempfile::TempDir,
        camera: &str,
        offset: i64,
        pedestal: f64,
    ) -> (Connection, PathBuf, Vec<PathBuf>) {
        const SIZE: usize = 256;
        let vignette = |x: usize| 0.7 + 0.6 * x as f64 / (SIZE - 1) as f64;
        let mut calibration_meta = Vec::new();
        for index in 0..2 {
            let path = temp.path().join(format!("flat-{index}.fits"));
            write_gradient_fits(&path, "FLAT", SIZE, SIZE, camera, offset, |x, _| {
                20_000.0 * vignette(x) + index as f64
            });
            calibration_meta.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        let mut light_paths = Vec::new();
        for index in 0..2 {
            let path = temp.path().join(format!("light-{index}.fits"));
            write_gradient_fits(&path, "LIGHT", SIZE, SIZE, camera, offset, |x, _| {
                pedestal + 100.0 * vignette(x) + index as f64
            });
            light_paths.push(path);
        }
        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &calibration_meta, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        (conn, temp.path().join("cache"), light_paths)
    }

    #[test]
    fn flat_masters_suppress_a_defective_pixel_the_flats_share() {
        // A hot pixel repeats identically in every flat, survives the
        // across-frame clipping, and would divide every light forever. The
        // spatial pass in the master build must take it out.
        const SIZE: usize = 64;
        let temp = tempfile::tempdir().unwrap();
        let hot = 31 * SIZE + 17;
        let vignette = |x: usize| 0.7 + 0.6 * x as f64 / (SIZE - 1) as f64;
        let mut calibration_meta = Vec::new();
        for index in 0..2 {
            let path = temp.path().join(format!("bias-{index}.fits"));
            write_gradient_fits(&path, "BIAS", SIZE, SIZE, "Camera", 30, |x, y| {
                100.0 + ((x * 31 + y * 17 + index) % 13) as f64 / 13.0
            });
            calibration_meta.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        for index in 0..2 {
            let path = temp.path().join(format!("flat-{index}.fits"));
            write_gradient_fits(&path, "FLAT", SIZE, SIZE, "Camera", 30, move |x, y| {
                if y * SIZE + x == hot {
                    30_000.0
                } else {
                    100.0 + 20_000.0 * vignette(x) + ((x * 7 + y * 3 + index) % 11) as f64
                }
            });
            calibration_meta.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        let light_path = temp.path().join("light.fits");
        write_gradient_fits(&light_path, "LIGHT", SIZE, SIZE, "Camera", 30, |x, _| {
            400.0 + 100.0 * vignette(x)
        });

        let mut conn = Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &calibration_meta, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        let cache = temp.path().join("cache");
        let (_, applied) = resolve_or_build_masters(
            &conn,
            &cache,
            std::slice::from_ref(&light_path),
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        let label = applied.flat_master.expect("a flat master");
        let master =
            crate::image_io::open_linear_frame(cache.join("calibration-masters").join(label))
                .unwrap();
        let value = master.image.data[hot];
        let neighbor = master.image.data[hot + 2];
        assert!(
            (value - neighbor).abs() < 0.05,
            "the defect must sit on the smooth response: {value} vs neighbor {neighbor}"
        );
    }

    #[test]
    fn auto_flat_only_fits_the_pedestal_and_applies_the_flat() {
        // The lights carry a 300 ADU pedestal over a vignetted sky. The fit
        // must recover it, the flat must apply through a synthesized bias,
        // and the signature must record the estimate. The ZWO offset header
        // (30 -> ~300 ADU) corroborates.
        let temp = tempfile::tempdir().unwrap();
        let (conn, cache, lights) =
            vignetted_flat_only_library(&temp, "ZWO ASI2600MM Pro", 30, 300.0);
        let (masters, applied) =
            resolve_or_build_masters(&conn, &cache, &lights, None, None, CalibrationMode::Auto)
                .unwrap();

        assert!(applied.flat_master.is_some(), "{:?}", applied.warning);
        assert!(!masters.is_empty());
        assert_eq!(applied.state, "applied");
        let pedestal = applied.estimated_pedestal_adu.expect("a fitted pedestal");
        assert!(
            (pedestal - 300.0).abs() < 15.0,
            "fitted {pedestal} ADU, expected ~300"
        );
        assert!(
            applied.masters_signature.contains(";pedestal=estimated-"),
            "signature must record the estimate: {}",
            applied.masters_signature
        );
        let warning = applied.warning.as_deref().unwrap_or_default();
        assert!(
            warning.contains("fitted from the lights")
                && warning.contains("consistent with the camera's recorded offset"),
            "warning must explain the estimate: {warning}"
        );
    }

    #[test]
    fn pedestal_fit_that_contradicts_the_camera_offset_withholds_the_flat() {
        // Same pixels, but the header claims an offset near 20000 ADU. A fit
        // of ~300 contradicting the driver's own record by this much is more
        // likely a gradient artifact, so the flat must stay withheld.
        let temp = tempfile::tempdir().unwrap();
        let (conn, cache, lights) =
            vignetted_flat_only_library(&temp, "ZWO ASI2600MM Pro", 2_000, 300.0);
        let (masters, applied) =
            resolve_or_build_masters(&conn, &cache, &lights, None, None, CalibrationMode::Auto)
                .unwrap();

        assert!(applied.flat_master.is_none());
        assert!(masters.is_empty());
        assert!(applied.estimated_pedestal_adu.is_none());
        let warning = applied.warning.as_deref().unwrap_or_default();
        assert!(
            warning.contains("no reliable pedestal could be fitted"),
            "warning must say why the flat stayed withheld: {warning}"
        );
    }

    #[test]
    fn pedestal_fit_works_without_a_camera_offset_mapping() {
        // An unknown camera family has no header hint; the fit stands on
        // its own and the warning does not claim corroboration.
        let temp = tempfile::tempdir().unwrap();
        let (conn, cache, lights) = vignetted_flat_only_library(&temp, "Camera", 30, 300.0);
        let (_, applied) =
            resolve_or_build_masters(&conn, &cache, &lights, None, None, CalibrationMode::Auto)
                .unwrap();

        assert!(applied.flat_master.is_some(), "{:?}", applied.warning);
        let pedestal = applied.estimated_pedestal_adu.expect("a fitted pedestal");
        assert!((pedestal - 300.0).abs() < 15.0);
        let warning = applied.warning.as_deref().unwrap_or_default();
        assert!(
            !warning.contains("consistent with the camera's recorded offset"),
            "no hint exists, so the warning must not claim one: {warning}"
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
        let (masters, applied) = resolve_or_build_masters(
            &conn,
            &cache,
            std::slice::from_ref(&light_path),
            None,
            None,
            CalibrationMode::On,
        )
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
    fn the_auto_fallback_plan_cannot_be_mistaken_for_off_or_resumed_into() {
        // When the real plan cannot be built, auto mode stacks raw rather
        // than failing — the edict is that calibration problems warn, never
        // kill. The fallback must still be honest about what it is: mode
        // auto (the user did not choose off), a state that says calibration
        // was unavailable, and a fingerprint distinct from both "off" and
        // any real selection, so a later run whose plan builds again does
        // not resume an accumulator whose frames went in uncalibrated.
        let plan = CalibrationPlan::without_calibration(3);
        assert_eq!(plan.assignments, vec![0, 0, 0]);
        assert_eq!(plan.applied.mode, CalibrationMode::Auto);
        assert_eq!(plan.applied.state, "unavailable");
        assert_ne!(plan.applied.fingerprint, "off");
        assert_ne!(plan.applied.fingerprint, "none");
        assert!(plan.sessions[0].masters.is_empty());
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
            std::slice::from_ref(&light_path),
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
            std::slice::from_ref(&light_path),
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
            std::slice::from_ref(&light_path),
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
    fn read_only_catalog_reuses_a_recorded_cached_master() {
        let temp = tempfile::tempdir().unwrap();
        let (database_path, cache, light_path) = file_backed_bias_library(&temp);

        let writable = Connection::open(&database_path).unwrap();
        let (built, first) = resolve_or_build_masters(
            &writable,
            &cache,
            std::slice::from_ref(&light_path),
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        assert!(!built.is_empty());
        assert!(first.bias_master.is_some());
        drop(writable);

        let read_only =
            Connection::open_with_flags(&database_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert!(read_only.is_readonly(rusqlite::MAIN_DB).unwrap());
        let (reused, applied) = resolve_or_build_masters(
            &read_only,
            &cache,
            std::slice::from_ref(&light_path),
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();

        assert!(!reused.is_empty());
        assert_eq!(applied.state, "applied");
        assert_eq!(applied.bias_master, first.bias_master);
    }

    #[test]
    fn read_only_catalog_skips_an_uncached_master_without_writing() {
        let temp = tempfile::tempdir().unwrap();
        let (database_path, cache, light_path) = file_backed_bias_library(&temp);
        let read_only =
            Connection::open_with_flags(&database_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();

        let (masters, applied) = resolve_or_build_masters(
            &read_only,
            &cache,
            std::slice::from_ref(&light_path),
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();

        assert!(masters.is_empty());
        assert!(applied.bias_master.is_none());
        assert_eq!(applied.state, "incomplete");
        let warning = applied.warning.as_deref().unwrap_or_default();
        assert!(
            warning.contains("catalog is read-only; no recorded cached bias master exists"),
            "{warning}"
        );
        let master_root = cache.join("calibration-masters");
        assert!(master_root.is_dir());
        assert!(
            std::fs::read_dir(master_root).unwrap().next().is_none(),
            "a read-only catalog must not leave an unrecorded cache file"
        );
    }

    #[test]
    fn newer_catalog_reuses_a_recorded_cached_master() {
        let temp = tempfile::tempdir().unwrap();
        let (database_path, cache, light_path) = file_backed_bias_library(&temp);
        let conn = Connection::open(&database_path).unwrap();
        let (_, first) = resolve_or_build_masters(
            &conn,
            &cache,
            std::slice::from_ref(&light_path),
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        conn.execute(
            "UPDATE psf_guard_calibration_schema SET version = ?1 WHERE singleton = 1",
            [CALIBRATION_SCHEMA_VERSION + 1],
        )
        .unwrap();

        let (reused, applied) = resolve_or_build_masters(
            &conn,
            &cache,
            std::slice::from_ref(&light_path),
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();

        assert!(!reused.is_empty());
        assert_eq!(applied.state, "applied");
        assert_eq!(applied.bias_master, first.bias_master);
    }

    #[test]
    fn a_master_from_an_older_generation_is_a_miss_not_a_hit() {
        // What let masters without optics metadata survive for a week: reuse
        // compared only the path and the file's kind, so bumping
        // MASTER_CACHE_VERSION invalidated nothing. The version is part of
        // what the cache promises — an older generation's file is not the
        // artifact this build would produce from the same sources.
        let temp = tempfile::tempdir().unwrap();
        let (database_path, cache, light_path) = file_backed_bias_library(&temp);
        let conn = Connection::open(&database_path).unwrap();
        resolve_or_build_masters(
            &conn,
            &cache,
            std::slice::from_ref(&light_path),
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        conn.execute(
            "UPDATE psf_guard_calibration_master SET cache_version = ?1",
            [i64::from(MASTER_CACHE_VERSION) - 1],
        )
        .unwrap();

        let (rebuilt, applied) = resolve_or_build_masters(
            &conn,
            &cache,
            std::slice::from_ref(&light_path),
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        assert!(!rebuilt.is_empty());
        assert_eq!(applied.state, "applied");
        let version: i64 = conn
            .query_row(
                "SELECT cache_version FROM psf_guard_calibration_master LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            version,
            i64::from(MASTER_CACHE_VERSION),
            "the stale master was rebuilt at the current generation"
        );
    }

    #[test]
    fn a_blocked_recorder_still_serves_an_older_generation_master() {
        // A read-only flow cannot rebuild, and an old master under today's
        // validation still beats no master at all.
        let temp = tempfile::tempdir().unwrap();
        let (database_path, cache, light_path) = file_backed_bias_library(&temp);
        let conn = Connection::open(&database_path).unwrap();
        let (_, first) = resolve_or_build_masters(
            &conn,
            &cache,
            std::slice::from_ref(&light_path),
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        conn.execute(
            "UPDATE psf_guard_calibration_master SET cache_version = ?1",
            [i64::from(MASTER_CACHE_VERSION) - 1],
        )
        .unwrap();
        drop(conn);

        let read_only =
            Connection::open_with_flags(&database_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let (masters, applied) = resolve_or_build_masters(
            &read_only,
            &cache,
            std::slice::from_ref(&light_path),
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        assert!(!masters.is_empty());
        assert_eq!(applied.bias_master, first.bias_master);
    }

    #[test]
    fn newer_catalog_is_not_changed_to_record_an_uncached_master() {
        let temp = tempfile::tempdir().unwrap();
        let (database_path, cache, light_path) = file_backed_bias_library(&temp);
        let conn = Connection::open(&database_path).unwrap();
        conn.execute_batch(&format!(
            "DROP TABLE psf_guard_calibration_master;
             UPDATE psf_guard_calibration_schema
             SET version = {} WHERE singleton = 1;",
            CALIBRATION_SCHEMA_VERSION + 1
        ))
        .unwrap();

        let (masters, applied) = resolve_or_build_masters(
            &conn,
            &cache,
            std::slice::from_ref(&light_path),
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();

        assert!(masters.is_empty());
        assert_eq!(applied.state, "incomplete");
        let warning = applied.warning.as_deref().unwrap_or_default();
        assert!(warning.contains("newer than this build"), "{warning}");
        let master_table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'psf_guard_calibration_master'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(master_table_count, 0, "a newer schema must not be changed");
        let master_root = cache.join("calibration-masters");
        assert!(master_root.is_dir());
        assert!(
            std::fs::read_dir(master_root).unwrap().next().is_none(),
            "a future schema must not leave an unrecorded cache file"
        );
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
            std::slice::from_ref(&light_path),
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
            std::slice::from_ref(&light_path),
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
    fn group_with_different_calibration_sets_partitions_into_sessions() {
        // Two lights whose selections differ (different gains here; per-night
        // flats in the field) used to force the whole group to stack
        // uncalibrated. They now partition: each light calibrates in its own
        // session with its own masters, and the group summary composes both.
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
        let plan = resolve_or_build_master_plan(
            &conn,
            &temp.path().join("cache"),
            &[first.clone(), second.clone()],
            None,
            None,
            CalibrationMode::Auto,
            &[],
        )
        .unwrap();
        assert_eq!(plan.sessions.len(), 2);
        assert_eq!(plan.assignments, vec![0, 1]);
        for session in &plan.sessions {
            assert_eq!(session.applied.state, "applied");
            assert!(session.applied.bias_master.is_some());
            assert!(!session.masters.is_empty());
        }
        assert_ne!(
            plan.sessions[0].applied.fingerprint,
            plan.sessions[1].applied.fingerprint
        );
        assert_eq!(plan.applied.sessions, 2);
        assert_eq!(plan.applied.state, "applied");
        assert_eq!(plan.applied.session_details.len(), 2);
        assert!(plan.applied.masters_signature.contains("||"));

        // The composite identity must not depend on the caller's light
        // order, or a reorder would force a restack.
        let reversed = resolve_or_build_master_plan(
            &conn,
            &temp.path().join("cache"),
            &[second, first],
            None,
            None,
            CalibrationMode::Auto,
            &[],
        )
        .unwrap();
        assert_eq!(reversed.applied.fingerprint, plan.applied.fingerprint);
        assert_eq!(
            reversed.applied.masters_signature,
            plan.applied.masters_signature
        );
        assert_eq!(reversed.assignments, vec![0, 1]);
    }

    #[test]
    fn a_pinned_session_pedestal_is_reused_instead_of_refitted() {
        // The source-frame search reproduces a stack's calibration from the
        // accepted frames only. With a flats-only library the pedestal was
        // fitted from the build's lights; the search must reuse the recorded
        // value, not fit its own from a different sample.
        let temp = tempfile::tempdir().unwrap();
        let (conn, cache, lights) =
            vignetted_flat_only_library(&temp, "ZWO ASI2600MM Pro", 30, 300.0);
        let plan = resolve_or_build_master_plan(
            &conn,
            &cache,
            &lights,
            None,
            None,
            CalibrationMode::Auto,
            &[],
        )
        .unwrap();
        let fitted = plan
            .applied
            .estimated_pedestal_adu
            .expect("a fitted pedestal");
        assert_eq!(plan.applied.sessions, 1);

        // Reproduce from a subset with an artificial pin: the pinned value
        // must land verbatim, so the signature matches the stack's.
        let mut pinned = plan.applied.session_details.clone();
        pinned[0].estimated_pedestal_adu = Some(123.0);
        let reproduced = resolve_or_build_master_plan(
            &conn,
            &cache,
            std::slice::from_ref(&lights[0]),
            None,
            None,
            CalibrationMode::Auto,
            &pinned,
        )
        .unwrap();
        assert_eq!(reproduced.applied.estimated_pedestal_adu, Some(123.0));
        assert!((fitted - 300.0).abs() < 15.0);
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
