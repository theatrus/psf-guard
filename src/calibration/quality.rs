//! Grades for the raw bias, dark, dark-flat and flat frames of a library.
//!
//! Seiza measures one frame at a time and judges a set of frames that should
//! agree. This module keeps each frame's measurement beside it, sorts frames
//! into the sets Seiza judges, and stores each frame's grade: ok, suspect or
//! poor. A poor frame, or one a person keeps out, stays out of masters and
//! exports; a person can also put a poor frame back. A suspect frame stays
//! in.
//!
//! A frame is read once. Later passes read only frames that are new, whose
//! file changed, or that an older algorithm measured. Grading needs no
//! pixels, so it runs again after every pass.

use super::{
    catalog_night_boundary, exposure_matches, frame_select, night_of, row_to_frame,
    schema_supports_current_reads, signature_sensor_consistent, signature_sensor_matches,
    temperature_matches, tolerances, CalibrationFrame, CalibrationKind, CalibrationSyncCounts,
};
use anyhow::{Context, Result};
use rusqlite::types::Value;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use seiza_calibration::{
    BiasScreening, DarkMeasureOptions, DarkMeasurement, DarkScreening, Finding, FindingCode,
    FlatMeasureOptions, FlatMeasurement, FlatScreening, FrameVerdict, Grade, LinearImageRef,
    FRAME_QUALITY_ALGORITHM_VERSION,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// One row per frame that was measured, graded or given an override.
pub(super) const QUALITY_TABLE: &str = "psf_guard_calibration_frame_quality";

/// Measurement, grade and override columns. Keyed by frame UUID rather than
/// row id, so a rescan, which keeps the UUID, keeps the row, and sync can
/// carry it to another catalog.
pub(super) const QUALITY_TABLE_SQL: &str = r#"
    CREATE TABLE IF NOT EXISTS psf_guard_calibration_frame_quality (
        frame_uuid          TEXT PRIMARY KEY,
        source_fingerprint  TEXT,
        algorithm_version   INTEGER,
        measurement_json    TEXT,
        measure_error       TEXT,
        measured_at         INTEGER,
        grade               TEXT,
        grade_code          TEXT,
        reason              TEXT,
        findings_json       TEXT,
        graded_at           INTEGER,
        override_choice     TEXT,
        override_at         INTEGER,
        updated_at          INTEGER NOT NULL
    );
"#;

/// A dark or bias master built from fewer frames than this puts its own
/// read noise into every light; its record says so.
pub const THIN_MASTER_FRAMES: usize = 5;

/// Frames measured between commits. Each is a full read of a raw frame.
const MEASURE_CHUNK: usize = 10;

/// Most flat features kept per frame. A clean flat has a few dozen, a frame
/// covered in droplets hundreds; past this it is poor anyway, and the stored
/// measurement stays small. The deepest are kept, and Seiza counts clear
/// features from the deepest down.
const MAX_STORED_FEATURES: usize = 2_000;

/// The grade table, created by schema rung 9 with no version bump. Safe to
/// run twice; adds only. A column added later goes in with
/// `add_column_if_missing` here, never by changing this statement alone.
pub(super) fn add_quality_table(conn: &Connection) -> Result<bool> {
    // Every open runs this rung, so it must cost nothing once the table is
    // there. A catalog that cannot be written reads as ungraded, which
    // every reader allows, so there is nothing to report.
    if table_exists(conn) || conn.is_readonly(rusqlite::MAIN_DB).unwrap_or(true) {
        return Ok(false);
    }
    conn.execute_batch(QUALITY_TABLE_SQL)
        .context("creating the calibration frame grade table")?;
    Ok(true)
}

/// The frames graded suspect, each with its grade's reason.
pub(super) fn suspect_reasons(conn: &Connection) -> Result<HashMap<String, String>> {
    if !table_exists(conn) {
        return Ok(HashMap::new());
    }
    let mut statement = conn.prepare(&format!(
        "SELECT frame_uuid, COALESCE(reason, '') FROM {QUALITY_TABLE} WHERE grade = 'suspect'"
    ))?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<HashMap<String, String>>>()?;
    Ok(rows)
}

/// Whether this catalog has the grade table. One this build could not
/// upgrade has none, and reads as if nothing were graded.
pub(super) fn table_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [QUALITY_TABLE],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .unwrap_or(false)
}

/// A person's choice about one frame, which outranks its grade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityOverride {
    /// Use the frame whatever its grade.
    Use,
    /// Keep the frame out of masters and exports whatever its grade.
    Exclude,
}

impl QualityOverride {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Use => "use",
            Self::Exclude => "exclude",
        }
    }

    pub(super) fn from_db(value: Option<&str>) -> Option<Self> {
        match value {
            Some("use") => Some(Self::Use),
            Some("exclude") => Some(Self::Exclude),
            _ => None,
        }
    }
}

/// Why a frame stays out of masters and exports, or `None` when it serves.
/// A person's choice comes first; then the grade; a frame not graded yet
/// keeps the stray-light mark an older build may have left on it.
pub(super) fn kept_out_reason(
    stray_light: Option<String>,
    grade: Option<&str>,
    reason: Option<String>,
    choice: Option<QualityOverride>,
) -> Option<String> {
    match choice {
        Some(QualityOverride::Use) => None,
        Some(QualityOverride::Exclude) => Some("by hand".into()),
        None => match grade {
            Some("poor") => Some(reason.unwrap_or_else(|| "graded poor".into())),
            Some(_) => None,
            None => stray_light,
        },
    }
}

/// Whether every frame carries a person's choice or a grade this build made
/// from its current file. Such a set needs no screen of its own: the grades
/// already keep the poor frames out, and a choice must stand.
pub(super) fn all_judged(conn: &Connection, frames: &[CalibrationFrame]) -> Result<bool> {
    if !table_exists(conn) {
        return Ok(false);
    }
    let mut statement = conn.prepare(
        "SELECT 1 FROM psf_guard_calibration_frame_quality
         WHERE frame_uuid = ?1
           AND (override_choice IS NOT NULL
                OR (grade IS NOT NULL AND algorithm_version = ?2
                    AND source_fingerprint IS ?3))",
    )?;
    for frame in frames {
        let judged = statement
            .query_row(
                params![
                    frame.frame_uuid,
                    FRAME_QUALITY_ALGORITHM_VERSION,
                    frame.source_fingerprint
                ],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !judged {
            return Ok(false);
        }
    }
    Ok(true)
}

/// What was measured on one frame, as stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StoredMeasurement {
    /// A bias, dark or dark-flat.
    Dark(DarkMeasurement),
    /// A flat.
    Flat(FlatMeasurement),
}

/// A raw frame waiting to be measured.
#[derive(Debug, Clone)]
pub struct PendingFrame {
    pub frame_uuid: String,
    pub kind: CalibrationKind,
    pub source_path: PathBuf,
    pub source_fingerprint: String,
}

/// What reading one frame came to.
#[derive(Debug, Clone)]
pub enum FrameMeasurement {
    Measured(Box<StoredMeasurement>),
    /// Read, but Seiza could not measure it (too small, say). Recorded, so
    /// it is not read again until its file changes.
    Refused(String),
    /// The file could not be read this time, as on an unmounted share. Not
    /// recorded; the next pass tries again.
    Unreadable(String),
}

/// What one measuring pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct MeasureOutcome {
    /// Frames that needed measuring when the pass began.
    pub pending: usize,
    pub measured: usize,
    pub refused: usize,
    pub unreadable: usize,
}

/// What one grading pass found.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct GradeOutcome {
    pub graded: usize,
    pub ok: usize,
    pub suspect: usize,
    pub poor: usize,
    /// Rows whose grade changed this pass.
    pub changed: usize,
}

/// Whether this build may measure and grade here: the catalog has the
/// grade table and no newer build has taken it past this build's schema.
fn grades_here(conn: &Connection) -> bool {
    schema_supports_current_reads(conn)
        && table_exists(conn)
        && super::recorded_schema_version(conn)
            .is_ok_and(|version| version <= super::CALIBRATION_SCHEMA_VERSION)
}

/// Raw frames with no measurement from this algorithm of their current file.
pub fn pending_measurements(conn: &Connection) -> Result<Vec<PendingFrame>> {
    if !grades_here(conn) {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT f.frame_uuid, f.kind, f.source_path, f.source_fingerprint
         FROM psf_guard_calibration_frame f
         LEFT JOIN psf_guard_calibration_frame_quality q ON q.frame_uuid = f.frame_uuid
         WHERE f.is_master = 0
           AND (q.frame_uuid IS NULL
                OR q.algorithm_version IS NULL
                OR q.algorithm_version <> ?1
                OR q.source_fingerprint IS NOT f.source_fingerprint)
         ORDER BY f.id",
    )?;
    let rows = statement
        .query_map([FRAME_QUALITY_ALGORITHM_VERSION], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(frame_uuid, kind, source_path, source_fingerprint)| {
            Some(PendingFrame {
                frame_uuid,
                kind: CalibrationKind::from_db(&kind)?,
                source_path: PathBuf::from(source_path),
                source_fingerprint,
            })
        })
        .collect())
}

/// Read one frame through [`crate::image_io`] and measure it. Seiza's own
/// threads run in the pool of the calling thread.
pub fn measure_frame(frame: &PendingFrame) -> FrameMeasurement {
    let opened = match crate::image_io::open_linear_frame(&frame.source_path) {
        Ok(opened) => opened,
        Err(error) => return FrameMeasurement::Unreadable(error.to_string()),
    };
    let image = &opened.image;
    let view = match LinearImageRef::new(&image.data, image.width, image.height, image.channels) {
        Ok(view) => view,
        Err(error) => return FrameMeasurement::Refused(error.to_string()),
    };
    let measured = match frame.kind {
        CalibrationKind::Flat => {
            let cfa = opened.bayer.is_some() && image.channels == 1;
            seiza_calibration::measure_flat(view, cfa, &FlatMeasureOptions::default()).map(
                |mut measurement| {
                    measurement.features.truncate(MAX_STORED_FEATURES);
                    StoredMeasurement::Flat(measurement)
                },
            )
        }
        CalibrationKind::Bias | CalibrationKind::Dark | CalibrationKind::DarkFlat => {
            seiza_calibration::measure_dark(view, &DarkMeasureOptions::default())
                .map(StoredMeasurement::Dark)
        }
    };
    match measured {
        Ok(measurement) => FrameMeasurement::Measured(Box::new(measurement)),
        Err(error) => FrameMeasurement::Refused(error.to_string()),
    }
}

/// Measure `pending` a few frames at a time with `measure`, which returns
/// one result per frame it is given, in order. Each batch is committed, so
/// a restart or a lock held by the capture software loses little.
/// `progress` hears how many of them are done, and `keep_going` is asked
/// before each batch, so a server stop ends the pass at a commit.
pub fn measure_frames(
    conn: &Connection,
    pending: &[PendingFrame],
    mut measure: impl FnMut(&[PendingFrame]) -> Vec<FrameMeasurement>,
    keep_going: impl Fn() -> bool,
    mut progress: impl FnMut(usize, usize),
) -> Result<MeasureOutcome> {
    let mut outcome = MeasureOutcome {
        pending: pending.len(),
        ..Default::default()
    };
    let mut done = 0;
    for chunk in pending.chunks(MEASURE_CHUNK) {
        if !keep_going() {
            break;
        }
        let measured = measure(chunk);
        anyhow::ensure!(
            measured.len() == chunk.len(),
            "measured {} of {} frames",
            measured.len(),
            chunk.len()
        );
        let results: Vec<(&PendingFrame, FrameMeasurement)> = chunk.iter().zip(measured).collect();
        record_measurements(conn, &results, &mut outcome)?;
        done += chunk.len();
        progress(done, pending.len());
    }
    Ok(outcome)
}

fn record_measurements(
    conn: &Connection,
    results: &[(&PendingFrame, FrameMeasurement)],
    outcome: &mut MeasureOutcome,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let tx = conn.unchecked_transaction()?;
    {
        let mut upsert = tx.prepare(
            "INSERT INTO psf_guard_calibration_frame_quality
                (frame_uuid, source_fingerprint, algorithm_version, measurement_json,
                 measure_error, measured_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(frame_uuid) DO UPDATE SET
                source_fingerprint = excluded.source_fingerprint,
                algorithm_version = excluded.algorithm_version,
                measurement_json = excluded.measurement_json,
                measure_error = excluded.measure_error,
                measured_at = excluded.measured_at,
                updated_at = excluded.updated_at",
        )?;
        let mut level = tx.prepare(
            "UPDATE psf_guard_calibration_frame
             SET dark_level = ?1, dark_level_sigma = ?2
             WHERE frame_uuid = ?3",
        )?;
        for (frame, result) in results {
            let (json, error) = match result {
                FrameMeasurement::Measured(measurement) => {
                    outcome.measured += 1;
                    // The library shows a dark's level, as it did before
                    // frames were graded.
                    if let StoredMeasurement::Dark(dark) = measurement.as_ref()
                        && matches!(
                            frame.kind,
                            CalibrationKind::Dark | CalibrationKind::DarkFlat
                        )
                    {
                        level.execute(params![
                            f64::from(dark.level),
                            f64::from(dark.sigma),
                            frame.frame_uuid
                        ])?;
                    }
                    (Some(serde_json::to_string(measurement.as_ref())?), None)
                }
                FrameMeasurement::Refused(error) => {
                    outcome.refused += 1;
                    tracing::debug!(
                        "calibration grade: cannot measure {}: {error}",
                        frame.source_path.display()
                    );
                    (None, Some(error.clone()))
                }
                FrameMeasurement::Unreadable(error) => {
                    outcome.unreadable += 1;
                    tracing::debug!(
                        "calibration grade: cannot read {}: {error}",
                        frame.source_path.display()
                    );
                    continue;
                }
            };
            upsert.execute(params![
                frame.frame_uuid,
                frame.source_fingerprint,
                FRAME_QUALITY_ALGORITHM_VERSION,
                json,
                error,
                now,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// One measured frame, ready to judge.
struct Measured {
    frame: CalibrationFrame,
    signature: super::Signature,
    measurement: StoredMeasurement,
    /// The stray-light mark as stored, which the grade keeps in step.
    stray_light: Option<String>,
}

/// A frame's grade, and for a dark the stray-light mark older builds read.
#[derive(Debug, Clone, PartialEq)]
struct Judged {
    grade: Grade,
    code: Option<FindingCode>,
    reason: Option<String>,
    findings: Vec<Finding>,
    stray_light: Option<String>,
}

/// Judge every measured raw frame against its peers and store each grade.
///
/// Peers are the frames that should agree. A bias or dark is judged against
/// every frame that could stand in for it: same kind, rig and sensor
/// settings, matching exposure and temperature, from any night, so a morning
/// whose darks all caught light is still caught by a clean set from another
/// night. Flats are judged within their session: one night, rig, filter,
/// rotation and sensor mode.
pub fn grade_library(conn: &Connection) -> Result<GradeOutcome> {
    if !grades_here(conn) {
        return Ok(GradeOutcome::default());
    }
    let measured = load_measured(conn)?;
    let mut judged: HashMap<String, Judged> = HashMap::new();
    judge_darks(&measured, &mut judged);
    judge_flats(&measured, catalog_night_boundary(conn), &mut judged);
    store_grades(conn, &measured, &judged)
}

fn load_measured(conn: &Connection) -> Result<Vec<Measured>> {
    let mut statement = conn.prepare(&format!(
        "{} WHERE f.is_master = 0 AND q.measurement_json IS NOT NULL
             AND q.algorithm_version = ?1 AND q.source_fingerprint IS f.source_fingerprint
         ORDER BY f.captured_at, f.id",
        frame_select(conn, &["q.measurement_json", "f.stray_light"])
    ))?;
    let rows = statement
        .query_map([FRAME_QUALITY_ALGORITHM_VERSION], |row| {
            Ok((
                row_to_frame(row)?,
                row.get::<_, String>(30)?,
                row.get::<_, Option<String>>(31)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(frame, json, stray_light)| {
            let measurement = match serde_json::from_str::<StoredMeasurement>(&json) {
                Ok(measurement) => measurement,
                Err(error) => {
                    tracing::debug!(
                        "calibration grade: unreadable measurement for {}: {error}",
                        frame.frame_uuid
                    );
                    return None;
                }
            };
            Some(Measured {
                signature: super::frame_signature(&frame),
                frame,
                measurement,
                stray_light,
            })
        })
        .collect())
}

/// Bias, darks and dark-flats, each against the frames that could stand in
/// for it, the way the dark-level check grouped them.
fn judge_darks(measured: &[Measured], judged: &mut HashMap<String, Judged>) {
    let mut buckets: HashMap<(CalibrationKind, &str), Vec<(&Measured, &DarkMeasurement)>> =
        HashMap::new();
    for entry in measured {
        if let StoredMeasurement::Dark(dark) = &entry.measurement
            && entry.frame.kind != CalibrationKind::Flat
        {
            buckets
                .entry((entry.frame.kind, entry.frame.rig_uuid.as_str()))
                .or_default()
                .push((entry, dark));
        }
    }
    let (dark_screening, bias_screening) = (DarkScreening::default(), BiasScreening::default());
    for ((kind, _), frames) in buckets {
        for (entry, _) in &frames {
            let peers: Vec<&(&Measured, &DarkMeasurement)> = frames
                .iter()
                .filter(|(other, _)| {
                    signature_sensor_matches(&entry.signature, &other.signature)
                        && exposure_matches(entry.frame.exposure_s, other.frame.exposure_s)
                        && temperature_matches(entry.frame.camera_temp, other.frame.camera_temp)
                })
                .collect();
            let Some(index) = peers
                .iter()
                .position(|(other, _)| other.frame.frame_uuid == entry.frame.frame_uuid)
            else {
                continue;
            };
            let set: Vec<DarkMeasurement> = peers.iter().map(|(_, dark)| (*dark).clone()).collect();
            let verdicts = if kind == CalibrationKind::Bias {
                seiza_calibration::screen_bias(&set, &bias_screening)
            } else {
                seiza_calibration::screen_darks(&set, &dark_screening)
            };
            let Some(verdict) = verdicts.into_iter().nth(index) else {
                continue;
            };
            let stray_light = (kind != CalibrationKind::Bias)
                .then(|| stray_light_text(&verdict, &set, index))
                .flatten();
            judged.insert(
                entry.frame.frame_uuid.clone(),
                judge(verdict, stray_light, kind),
            );
        }
    }
}

/// The library's stray-light words for a dark whose level is poor: how far
/// above the quietest of its peers it sits.
fn stray_light_text(
    verdict: &FrameVerdict,
    set: &[DarkMeasurement],
    index: usize,
) -> Option<String> {
    let finding = verdict
        .findings
        .iter()
        .find(|finding| finding.code == FindingCode::LevelExcess && finding.grade == Grade::Poor)?;
    let quietest = set.iter().min_by(|a, b| a.level.total_cmp(&b.level))?;
    let frame = set.get(index)?;
    Some(format!(
        "stray light: level {:.1} sits {:.1} above the quietest dark, {:.1} times its pixel \
         spread of {:.1}",
        frame.level,
        frame.level - quietest.level,
        finding.value,
        quietest.sigma
    ))
}

/// The key flats of one session share. Filters compare without case.
#[derive(Clone, PartialEq, Eq, Hash)]
struct FlatSessionKey {
    rig: String,
    night: String,
    filter: Option<String>,
    width: usize,
    height: usize,
}

/// Flats by session, each session screened as one set.
fn judge_flats(measured: &[Measured], boundary: i64, judged: &mut HashMap<String, Judged>) {
    for set in flat_sessions(measured, boundary) {
        let measurements: Vec<FlatMeasurement> = set
            .iter()
            .filter_map(|entry| match &entry.measurement {
                StoredMeasurement::Flat(flat) => Some(flat.clone()),
                StoredMeasurement::Dark(_) => None,
            })
            .collect();
        let report = match seiza_calibration::screen_flats(&measurements, &FlatScreening::default())
        {
            Ok(report) => report,
            Err(error) => {
                tracing::warn!("calibration grade: a flat session was not screened: {error}");
                continue;
            }
        };
        for (entry, verdict) in set.iter().zip(report.frames) {
            judged.insert(
                entry.frame.frame_uuid.clone(),
                judge(verdict, None, CalibrationKind::Flat),
            );
        }
    }
}

/// Measured flats split into sessions: one night, rig and filter, one frame
/// size, and sensor and optical settings that do not contradict the first
/// frame of the session. Frames arrive in capture order.
fn flat_sessions(measured: &[Measured], boundary: i64) -> Vec<Vec<&Measured>> {
    let mut sessions: Vec<(FlatSessionKey, Vec<&Measured>)> = Vec::new();
    for entry in measured {
        let StoredMeasurement::Flat(flat) = &entry.measurement else {
            continue;
        };
        if entry.frame.kind != CalibrationKind::Flat {
            continue;
        }
        let key = FlatSessionKey {
            rig: entry.frame.rig_uuid.clone(),
            night: entry
                .frame
                .captured_at
                .map(|at| night_of(at, boundary))
                .unwrap_or_else(|| "unknown".into()),
            filter: entry
                .frame
                .filter
                .as_deref()
                .map(|filter| filter.trim().to_ascii_lowercase()),
            width: flat.width,
            height: flat.height,
        };
        let joins = |(session_key, frames): &&mut (FlatSessionKey, Vec<&Measured>)| {
            *session_key == key
                && frames.first().is_some_and(|first| {
                    signature_sensor_consistent(&first.signature, &entry.signature)
                        && seiza_calibration::optics_consistent(
                            &first.signature.seiza,
                            &entry.signature.seiza,
                            &tolerances(),
                        )
                })
        };
        match sessions.iter_mut().find(|session| joins(session)) {
            Some((_, frames)) => frames.push(entry),
            None => sessions.push((key, vec![entry])),
        }
    }
    sessions.into_iter().map(|(_, frames)| frames).collect()
}

fn judge(verdict: FrameVerdict, stray_light: Option<String>, kind: CalibrationKind) -> Judged {
    // The worst finding speaks for the frame; the first of equal grade.
    let main = verdict
        .findings
        .iter()
        .find(|finding| finding.grade == verdict.grade)
        .filter(|_| verdict.grade != Grade::Ok);
    let reason = main.map(|finding| match (&stray_light, finding.code) {
        (Some(text), FindingCode::LevelExcess) if finding.grade == Grade::Poor => text.clone(),
        _ => describe_finding(finding, kind),
    });
    Judged {
        grade: verdict.grade,
        code: main.map(|finding| finding.code),
        reason,
        findings: verdict.findings,
        stray_light,
    }
}

/// One finding in plain words, with its number.
pub fn describe_finding(finding: &Finding, kind: CalibrationKind) -> String {
    let value = finding.value;
    let peers = if kind == CalibrationKind::Flat {
        "its session"
    } else {
        "its peers"
    };
    match finding.code {
        FindingCode::Clipped => format!("{:.2}% of samples clipped", value * 100.0),
        FindingCode::Bright => format!("brightest channel at {:.0}% of full scale", value * 100.0),
        FindingCode::Dim => format!("dimmest channel at {:.1}% of full scale", value * 100.0),
        FindingCode::Illumination => {
            format!(
                "light falls {:.1}% off the pattern of {peers}",
                value * 100.0
            )
        }
        FindingCode::Features => format!("{value:.0} clear spots"),
        FindingCode::ChangingFeatures => format!("{value:.0} spots that change across {peers}"),
        FindingCode::CarriedFeatures => format!("{value:.0} changing spots in most frames"),
        FindingCode::Level => format!("level {value:.1} times the noise from {peers}"),
        FindingCode::LevelExcess => {
            format!("level {value:.2} times its spread above the quietest dark")
        }
        FindingCode::Noise => format!("noise {value:.2} times that of {peers}"),
        FindingCode::RowBanding => format!("row banding {value:.2} of the noise"),
        FindingCode::ColumnBanding => format!("column banding {value:.2} of the noise"),
        FindingCode::HotPixels => format!("{value:.1} times the hot pixels of {peers}"),
        code => format!("{} {value:.3}", code.as_str()),
    }
}

fn store_grades(
    conn: &Connection,
    measured: &[Measured],
    judged: &HashMap<String, Judged>,
) -> Result<GradeOutcome> {
    let mut outcome = GradeOutcome::default();
    let stored: HashMap<String, (Option<String>, Option<String>)> = conn
        .prepare(
            "SELECT frame_uuid, grade, findings_json FROM psf_guard_calibration_frame_quality",
        )?
        .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))?
        .collect::<rusqlite::Result<_>>()?;
    let now = chrono::Utc::now().timestamp();
    let tx = conn.unchecked_transaction()?;
    {
        let mut update = tx.prepare(
            "UPDATE psf_guard_calibration_frame_quality
             SET grade = ?1, grade_code = ?2, reason = ?3, findings_json = ?4,
                 graded_at = ?5, updated_at = ?5
             WHERE frame_uuid = ?6",
        )?;
        let mut mark = tx.prepare(
            "UPDATE psf_guard_calibration_frame SET stray_light = ?1, updated_at = ?2
             WHERE frame_uuid = ?3",
        )?;
        for entry in measured {
            let Some(result) = judged.get(&entry.frame.frame_uuid) else {
                continue;
            };
            outcome.graded += 1;
            match result.grade {
                Grade::Ok => outcome.ok += 1,
                Grade::Suspect => outcome.suspect += 1,
                Grade::Poor => outcome.poor += 1,
            }
            let findings = serde_json::to_string(&result.findings)?;
            let grade = result.grade.as_str();
            let unchanged =
                stored
                    .get(&entry.frame.frame_uuid)
                    .is_some_and(|(old_grade, old_findings)| {
                        old_grade.as_deref() == Some(grade)
                            && old_findings.as_deref() == Some(findings.as_str())
                    });
            if !unchanged {
                outcome.changed += 1;
                update.execute(params![
                    grade,
                    result.code.map(FindingCode::as_str),
                    result.reason,
                    findings,
                    now,
                    entry.frame.frame_uuid,
                ])?;
            }
            // Older builds read only this mark, so it follows the grade.
            if matches!(
                entry.frame.kind,
                CalibrationKind::Dark | CalibrationKind::DarkFlat
            ) && entry.stray_light != result.stray_light
            {
                mark.execute(params![result.stray_light, now, entry.frame.frame_uuid])?;
            }
        }
        // Rows of frames the library no longer holds.
        tx.execute(
            "DELETE FROM psf_guard_calibration_frame_quality
             WHERE frame_uuid NOT IN (SELECT frame_uuid FROM psf_guard_calibration_frame)",
            [],
        )?;
    }
    tx.commit()?;
    Ok(outcome)
}

/// Measure what is pending, then grade the library: one whole pass with
/// Seiza on the calling thread's pool.
pub fn measure_and_grade(conn: &Connection) -> Result<(MeasureOutcome, GradeOutcome)> {
    let pending = pending_measurements(conn)?;
    let measured = measure_frames(
        conn,
        &pending,
        |frames| frames.iter().map(measure_frame).collect(),
        || true,
        |_, _| {},
    )?;
    Ok((measured, grade_library(conn)?))
}

/// Set or clear a person's choice on frames. Returns how many of the given
/// frames the library holds. The choice outranks any grade, survives
/// regrading and rescans, and travels with the frame when a catalog syncs.
pub fn set_overrides(
    conn: &Connection,
    frame_uuids: &[String],
    choice: Option<QualityOverride>,
) -> Result<usize> {
    if frame_uuids.is_empty() || !super::schema_exists(conn) {
        return Ok(0);
    }
    if !table_exists(conn) {
        anyhow::bail!(
            "this catalog has not been upgraded to record calibration grades; \
             open it with write access once and retry"
        );
    }
    let now = chrono::Utc::now().timestamp();
    let mut statement = conn.prepare(
        "INSERT INTO psf_guard_calibration_frame_quality
            (frame_uuid, override_choice, override_at, updated_at)
         SELECT frame_uuid, ?1, ?2, ?2 FROM psf_guard_calibration_frame WHERE frame_uuid = ?3
         ON CONFLICT(frame_uuid) DO UPDATE SET
            override_choice = excluded.override_choice,
            override_at = excluded.override_at,
            updated_at = excluded.updated_at",
    )?;
    let mut changed = 0;
    for frame_uuid in frame_uuids {
        changed += statement.execute(params![
            choice.map(QualityOverride::as_str),
            now,
            frame_uuid
        ])?;
    }
    Ok(changed)
}

/// Drop the grade rows of forgotten frames.
pub(super) fn forget(conn: &Transaction<'_>, frame_uuids: &[String]) -> Result<()> {
    if !table_exists(conn) {
        return Ok(());
    }
    let mut statement =
        conn.prepare("DELETE FROM psf_guard_calibration_frame_quality WHERE frame_uuid = ?1")?;
    for frame_uuid in frame_uuids {
        statement.execute([frame_uuid])?;
    }
    Ok(())
}

/// Copy grade rows to another catalog with their frames. Each part takes the
/// newer side: the measurement by when it was taken, the grade by when it
/// was given, and a person's choice by when it was made, so a choice made
/// on either machine, a cleared one included, survives the round trip.
pub(super) fn sync_quality(
    source: &Connection,
    destination: &Transaction<'_>,
) -> Result<CalibrationSyncCounts> {
    let mut counts = CalibrationSyncCounts::default();
    if !table_exists(source) {
        return Ok(counts);
    }
    const MEASUREMENT: [&str; 6] = [
        "source_fingerprint",
        "algorithm_version",
        "measurement_json",
        "measure_error",
        "measured_at",
        "updated_at",
    ];
    const GRADE: [&str; 5] = [
        "grade",
        "grade_code",
        "reason",
        "findings_json",
        "graded_at",
    ];
    const CHOICE: [&str; 2] = ["override_choice", "override_at"];
    let columns: Vec<&str> = std::iter::once("frame_uuid")
        .chain(MEASUREMENT)
        .chain(GRADE)
        .chain(CHOICE)
        .collect();
    let select = format!("SELECT {} FROM {QUALITY_TABLE}", columns.join(", "));
    let row_values = |row: &rusqlite::Row<'_>| {
        (0..columns.len())
            .map(|index| row.get::<_, Value>(index))
            .collect::<rusqlite::Result<Vec<_>>>()
    };
    let position = |name: &str| columns.iter().position(|column| *column == name).unwrap();
    let time = |values: &[Value], name: &str| match values[position(name)] {
        Value::Integer(at) => Some(at),
        _ => None,
    };
    let newer = |source: Option<i64>, destination: Option<i64>| match (source, destination) {
        (Some(source), Some(destination)) => source > destination,
        (Some(_), None) => true,
        _ => false,
    };
    let mut statement = source.prepare(&select)?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let values = row_values(row)?;
        let Value::Text(frame_uuid) = &values[0] else {
            continue;
        };
        let known: bool = destination
            .query_row(
                "SELECT 1 FROM psf_guard_calibration_frame WHERE frame_uuid = ?1",
                [frame_uuid],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !known {
            continue;
        }
        let existing = destination
            .query_row(
                &format!("{select} WHERE frame_uuid = ?1"),
                [frame_uuid],
                row_values,
            )
            .optional()?;
        let Some(existing) = existing else {
            let placeholders = (1..=columns.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            destination.execute(
                &format!(
                    "INSERT INTO {QUALITY_TABLE} ({}) VALUES ({placeholders})",
                    columns.join(", ")
                ),
                rusqlite::params_from_iter(values.iter()),
            )?;
            counts.inserted += 1;
            continue;
        };
        let mut merged = existing.clone();
        for (part, at) in [
            (&MEASUREMENT[..], "measured_at"),
            (&GRADE[..], "graded_at"),
            (&CHOICE[..], "override_at"),
        ] {
            if newer(time(&values, at), time(&existing, at)) {
                for column in part {
                    merged[position(column)] = values[position(column)].clone();
                }
            }
        }
        if merged == existing {
            counts.unchanged += 1;
            continue;
        }
        let assignments = columns[1..]
            .iter()
            .enumerate()
            .map(|(index, column)| format!("{column} = ?{}", index + 1))
            .collect::<Vec<_>>()
            .join(", ");
        destination.execute(
            &format!(
                "UPDATE {QUALITY_TABLE} SET {assignments} WHERE frame_uuid = ?{}",
                columns.len()
            ),
            rusqlite::params_from_iter(merged[1..].iter().chain(std::iter::once(&merged[0]))),
        )?;
        counts.updated += 1;
    }
    Ok(counts)
}

/// How a frame came out, for the library listing.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct FrameGrade {
    /// `ok`, `suspect` or `poor`; absent until graded.
    pub grade: Option<String>,
    /// The main finding's code, such as `level_excess` or `noise`.
    pub grade_code: Option<String>,
    /// The main finding in plain words, with its number.
    pub grade_reason: Option<String>,
    /// A person's choice: `use` or `exclude`.
    pub quality_override: Option<QualityOverride>,
}

/// Library-wide counts for the job's route.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct QualityCounts {
    /// Raw frames in the library.
    pub frames: usize,
    /// Raw frames with a current measurement.
    pub measured: usize,
    pub ok: usize,
    pub suspect: usize,
    pub poor: usize,
    /// Frames a person chose to keep out.
    pub kept_out_by_hand: usize,
    /// Poor frames a person chose to use anyway.
    pub used_anyway: usize,
}

pub fn quality_counts(conn: &Connection) -> Result<QualityCounts> {
    if !schema_supports_current_reads(conn) {
        return Ok(QualityCounts::default());
    }
    let frames = conn.query_row(
        "SELECT COUNT(*) FROM psf_guard_calibration_frame WHERE is_master = 0",
        [],
        |row| row.get::<_, i64>(0),
    )? as usize;
    if !table_exists(conn) {
        return Ok(QualityCounts {
            frames,
            ..Default::default()
        });
    }
    let (measured, ok, suspect, poor, kept_out, used) = conn.query_row(
        "SELECT
            SUM(q.measurement_json IS NOT NULL AND q.algorithm_version = ?1
                AND q.source_fingerprint IS f.source_fingerprint),
            SUM(q.grade = 'ok'), SUM(q.grade = 'suspect'), SUM(q.grade = 'poor'),
            SUM(q.override_choice = 'exclude'),
            SUM(q.override_choice = 'use' AND q.grade = 'poor')
         FROM psf_guard_calibration_frame f
         JOIN psf_guard_calibration_frame_quality q ON q.frame_uuid = f.frame_uuid",
        [FRAME_QUALITY_ALGORITHM_VERSION],
        |row| {
            let count = |index| {
                row.get::<_, Option<i64>>(index)
                    .map(|value| value.unwrap_or(0) as usize)
            };
            Ok((
                count(0)?,
                count(1)?,
                count(2)?,
                count(3)?,
                count(4)?,
                count(5)?,
            ))
        },
    )?;
    Ok(QualityCounts {
        frames,
        measured,
        ok,
        suspect,
        poor,
        kept_out_by_hand: kept_out,
        used_anyway: used,
    })
}

/// What a set of flats would carry into a master built from them: Seiza's
/// master verdict on their stored measurements, for the master's record.
/// `None` when fewer than three of them are measured, since the set check
/// needs peers.
pub(super) fn flat_set_verdict(
    conn: &Connection,
    frames: &[CalibrationFrame],
) -> Option<serde_json::Value> {
    if !table_exists(conn) {
        return None;
    }
    let mut statement = conn
        .prepare(
            "SELECT q.measurement_json FROM psf_guard_calibration_frame_quality q
             JOIN psf_guard_calibration_frame f ON f.frame_uuid = q.frame_uuid
             WHERE q.frame_uuid = ?1 AND q.algorithm_version = ?2
               AND q.source_fingerprint IS f.source_fingerprint",
        )
        .ok()?;
    let measurements: Vec<FlatMeasurement> = frames
        .iter()
        .filter_map(|frame| {
            let json: String = statement
                .query_row(
                    params![frame.frame_uuid, FRAME_QUALITY_ALGORITHM_VERSION],
                    |row| row.get(0),
                )
                .ok()?;
            match serde_json::from_str(&json).ok()? {
                StoredMeasurement::Flat(flat) => Some(flat),
                StoredMeasurement::Dark(_) => None,
            }
        })
        .collect();
    let mut value = serde_json::json!({
        "measured_frames": measurements.len(),
        "frames": frames.len(),
    });
    if measurements.len() < 3 {
        return Some(value);
    }
    let report = seiza_calibration::screen_flats(&measurements, &FlatScreening::default()).ok()?;
    value["master"] = serde_json::to_value(&report.master).ok()?;
    value["steady_features"] = report.steady_features.into();
    value["changing_features"] = report.changing_features.into();
    value["carried_features"] = report.carried_features.into();
    Some(value)
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::{
        explain_calibration, export_destinations, forget_frames, import_calibration_frames,
        library_details, migrate_existing, resolve_or_build_masters, select_for_light,
        sync_library, CalibrationMode,
    };
    use super::*;
    use crate::commands::import::headers::{read_frame_meta, FrameMeta};
    use std::io::Write as _;
    use std::path::Path;

    /// A deterministic, roughly normal number for pixel `(x, y)` of frame
    /// `seed`: unit spread, never beyond about 3.5.
    pub(crate) fn noise(x: usize, y: usize, seed: u64) -> f64 {
        let mut state = seed
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add(((y as u64) << 32) | x as u64);
        let mut sum = 0.0;
        for _ in 0..4 {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            sum += (z >> 11) as f64 / (1u64 << 53) as f64;
        }
        (sum - 2.0) * 3f64.sqrt()
    }

    /// An unsigned 16-bit FITS frame from one rig: camera `Camera` on
    /// `Scope`, gain 100, offset 20, -10 °C, filter Ha. Darks and lights are
    /// 300 s, flats 2 s, bias 0 s.
    pub(crate) fn write_frame(
        path: &Path,
        kind: &str,
        (width, height): (usize, usize),
        captured: &str,
        pixel: impl Fn(usize, usize) -> f64,
    ) {
        let exposure = match kind {
            "BIAS" => 0.0,
            "FLAT" | "DARKFLAT" => 2.0,
            _ => 300.0,
        };
        let cards = [
            "SIMPLE  =                    T".to_string(),
            "BITPIX  =                   16".to_string(),
            "NAXIS   =                    2".to_string(),
            format!("NAXIS1  = {width:20}"),
            format!("NAXIS2  = {height:20}"),
            "BZERO   =                32768".to_string(),
            "BSCALE  =                    1".to_string(),
            format!("IMAGETYP= '{kind}'"),
            format!("DATE-OBS= '{captured}'"),
            "FILTER  = 'Ha'".to_string(),
            format!("EXPTIME = {exposure:20.1}"),
            "GAIN    =                  100".to_string(),
            "OFFSET  =                   20".to_string(),
            "XBINNING=                    1".to_string(),
            "YBINNING=                    1".to_string(),
            "CCD-TEMP=                -10.0".to_string(),
            "TELESCOP= 'Scope'".to_string(),
            "INSTRUME= 'Camera'".to_string(),
            "END".to_string(),
        ];
        let mut header = Vec::new();
        for card in cards {
            let mut bytes = card.into_bytes();
            bytes.resize(80, b' ');
            header.extend(bytes);
        }
        header.resize(header.len().div_ceil(2880) * 2880, b' ');
        let mut payload = Vec::with_capacity(width * height * 2);
        for y in 0..height {
            for x in 0..width {
                let value = pixel(x, y).round().clamp(0.0, 65_535.0) as u16;
                payload.extend(((i32::from(value) - 32_768) as i16).to_be_bytes());
            }
        }
        payload.resize(payload.len().div_ceil(2880) * 2880, 0);
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(&header).unwrap();
        file.write_all(&payload).unwrap();
    }

    const DARK_SIZE: (usize, usize) = (64, 64);
    const FLAT_SIZE: (usize, usize) = (128, 128);

    /// A dark at `level` with five counts of noise.
    fn dark(path: &Path, kind: &str, level: f64, seed: u64) -> FrameMeta {
        write_frame(path, kind, DARK_SIZE, "2026-09-10T10:00:00", |x, y| {
            level + 5.0 * noise(x, y, seed)
        });
        read_frame_meta(path)
    }

    /// A flat at 30,000 counts with a little noise and, when `spots` is set,
    /// thirty-six dips 8% deep that no other frame has.
    fn flat(path: &Path, seed: u64, spots: bool) -> FrameMeta {
        write_frame(path, "FLAT", FLAT_SIZE, "2026-09-10T02:00:00", |x, y| {
            let spot = spots
                && (16..112).contains(&x)
                && (16..112).contains(&y)
                && (x - 16) % 18 < 4
                && (y - 16) % 18 < 4;
            30_000.0 * if spot { 0.92 } else { 1.0 } + 20.0 * noise(x, y, seed)
        });
        read_frame_meta(path)
    }

    fn catalog(path: &Path, frames: &[FrameMeta]) -> Connection {
        let mut conn = Connection::open(path).unwrap();
        let tx = conn.transaction().unwrap();
        import_calibration_frames(&tx, frames, Some("profile")).unwrap();
        tx.commit().unwrap();
        conn
    }

    fn uuid_of(conn: &Connection, name: &str) -> String {
        conn.query_row(
            "SELECT frame_uuid FROM psf_guard_calibration_frame WHERE source_path LIKE ?1",
            [format!("%{name}")],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn named(frames: &[CalibrationFrame]) -> Vec<String> {
        let mut names: Vec<String> = frames
            .iter()
            .map(|frame| {
                frame
                    .source_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    /// Three steady darks, one that caught light, and three bias frames.
    fn dark_library(root: &Path) -> (Connection, FrameMeta) {
        let mut frames = Vec::new();
        for (index, level) in [503.0, 503.0, 503.0, 900.0].into_iter().enumerate() {
            frames.push(dark(
                &root.join(format!("dark-{index}.fits")),
                "DARK",
                level,
                index as u64,
            ));
        }
        for index in 0..3 {
            frames.push(dark(
                &root.join(format!("bias-{index}.fits")),
                "BIAS",
                500.0,
                10 + index,
            ));
        }
        let light = root.join("light.fits");
        write_frame(&light, "LIGHT", DARK_SIZE, "2026-09-11T03:00:00", |_, _| {
            1_100.0
        });
        (
            catalog(&root.join("catalog.sqlite"), &frames),
            read_frame_meta(&light),
        )
    }

    #[test]
    fn a_dark_that_caught_light_is_poor_and_leaves_masters_and_exports() {
        let temp = tempfile::tempdir().unwrap();
        let (conn, light) = dark_library(temp.path());
        assert_eq!(select_for_light(&conn, &light).unwrap().dark.len(), 4);

        let (measured, graded) = measure_and_grade(&conn).unwrap();
        assert_eq!(measured.pending, 7, "every raw bias and dark");
        assert_eq!(measured.measured, 7);
        assert_eq!((graded.graded, graded.poor), (7, 1), "{graded:?}");
        assert!(pending_measurements(&conn).unwrap().is_empty(), "read once");

        let selected = select_for_light(&conn, &light).unwrap();
        assert_eq!(
            named(&selected.dark),
            ["dark-0.fits", "dark-1.fits", "dark-2.fits"]
        );
        assert_eq!(selected.bias.len(), 3);
        assert_eq!(named(&selected.kept_out), ["dark-3.fits"]);
        let why = selected.kept_out[0].kept_out.as_deref().unwrap();
        assert!(why.starts_with("stray light: level 9"), "{why}");

        // The stray-light mark that older builds read follows the grade.
        let leaked = uuid_of(&conn, "dark-3.fits");
        let mark: Option<String> = conn
            .query_row(
                "SELECT stray_light FROM psf_guard_calibration_frame WHERE frame_uuid = ?1",
                [&leaked],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mark.as_deref(), Some(why));

        let export = export_destinations(
            &conn,
            &light,
            "Target",
            None,
            crate::commands::export::ExportLayout::Wbpp,
            12 * 3600,
        )
        .unwrap();
        assert_eq!(export.kept_out.len(), 1);
        assert!(export
            .items
            .iter()
            .all(|(_, frame, _)| !frame.source_path.ends_with("dark-3.fits")));

        // The explanation gives the frame and says why.
        let explained = explain_calibration(&conn, &light).unwrap();
        let darks = explained
            .kinds
            .iter()
            .find(|kind| kind.kind == "dark")
            .unwrap();
        assert_eq!(darks.kept_out.len(), 1);
        assert_eq!(darks.kept_out[0].file, "dark-3.fits");
        assert_eq!(darks.kept_out[0].reason, why);

        // The library listing carries the grade.
        let details = library_details(&conn).unwrap();
        let listed = details
            .frames
            .iter()
            .find(|frame| frame.frame_uuid == leaked)
            .unwrap();
        assert_eq!(listed.quality.grade.as_deref(), Some("poor"));
        assert_eq!(listed.quality.grade_code.as_deref(), Some("level_excess"));
        assert_eq!(listed.kept_out.as_deref(), Some(why));
        assert!(listed
            .dark_level
            .is_some_and(|level| (level - 900.0).abs() < 2.0));
        assert!(details
            .frames
            .iter()
            .filter(|frame| frame.frame_uuid != leaked)
            .all(|frame| frame.quality.grade.as_deref() == Some("ok") && frame.kept_out.is_none()));
    }

    #[test]
    fn a_person_can_use_a_poor_frame_or_keep_out_a_good_one() {
        let temp = tempfile::tempdir().unwrap();
        let (conn, light) = dark_library(temp.path());
        measure_and_grade(&conn).unwrap();
        let (leaked, quiet) = (uuid_of(&conn, "dark-3.fits"), uuid_of(&conn, "dark-0.fits"));
        let darks = |conn: &Connection| named(&select_for_light(conn, &light).unwrap().dark);

        set_overrides(
            &conn,
            std::slice::from_ref(&leaked),
            Some(QualityOverride::Use),
        )
        .unwrap();
        assert_eq!(darks(&conn).len(), 4, "used anyway");
        set_overrides(
            &conn,
            std::slice::from_ref(&quiet),
            Some(QualityOverride::Exclude),
        )
        .unwrap();
        let selected = select_for_light(&conn, &light).unwrap();
        assert_eq!(
            named(&selected.dark),
            ["dark-1.fits", "dark-2.fits", "dark-3.fits"]
        );
        assert_eq!(named(&selected.kept_out), ["dark-0.fits"]);
        assert_eq!(selected.kept_out[0].kept_out.as_deref(), Some("by hand"));

        // Choices outlast a regrade and a rescan of the same files.
        grade_library(&conn).unwrap();
        let rescan: Vec<FrameMeta> = (0..4)
            .map(|index| read_frame_meta(&temp.path().join(format!("dark-{index}.fits"))))
            .collect();
        let mut conn = conn;
        {
            let tx = conn.transaction().unwrap();
            import_calibration_frames(&tx, &rescan, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        measure_and_grade(&conn).unwrap();
        assert_eq!(darks(&conn), ["dark-1.fits", "dark-2.fits", "dark-3.fits"]);

        // Clearing hands both back to their grades.
        set_overrides(&conn, &[leaked, quiet], None).unwrap();
        assert_eq!(darks(&conn), ["dark-0.fits", "dark-1.fits", "dark-2.fits"]);
        assert_eq!(
            set_overrides(&conn, &["no-such-frame".into()], None).unwrap(),
            0
        );
    }

    #[test]
    fn a_lone_dark_left_by_a_forget_is_admitted_again() {
        let temp = tempfile::tempdir().unwrap();
        let (mut conn, light) = dark_library(temp.path());
        measure_and_grade(&conn).unwrap();
        let quiet: Vec<String> = (0..3)
            .map(|index| uuid_of(&conn, &format!("dark-{index}.fits")))
            .collect();
        forget_frames(&mut conn, &quiet).unwrap();
        let rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM psf_guard_calibration_frame_quality",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rows, 4, "forgotten frames take their grades with them");
        let graded = grade_library(&conn).unwrap();
        assert_eq!(graded.poor, 0, "nothing left to judge it against");
        assert_eq!(select_for_light(&conn, &light).unwrap().dark.len(), 1);
    }

    #[test]
    fn a_flat_with_spots_the_others_lack_is_poor() {
        let temp = tempfile::tempdir().unwrap();
        let mut frames = Vec::new();
        for index in 0..4 {
            frames.push(flat(
                &temp.path().join(format!("flat-{index}.fits")),
                20 + index,
                index == 3,
            ));
        }
        let conn = catalog(&temp.path().join("catalog.sqlite"), &frames);
        let (measured, graded) = measure_and_grade(&conn).unwrap();
        assert_eq!((measured.measured, graded.graded), (4, 4));
        assert_eq!((graded.ok, graded.poor), (3, 1), "{graded:?}");
        let details = library_details(&conn).unwrap();
        let spotted = details
            .frames
            .iter()
            .find(|frame| frame.source_path.ends_with("flat-3.fits"))
            .unwrap();
        assert_eq!(
            spotted.quality.grade_code.as_deref(),
            Some("changing_features")
        );
        assert_eq!(
            spotted.kept_out.as_deref(),
            Some("36 spots that change across its session")
        );

        let light = temp.path().join("light.fits");
        write_frame(&light, "LIGHT", FLAT_SIZE, "2026-09-10T05:00:00", |_, _| {
            1_000.0
        });
        let selected = select_for_light(&conn, &read_frame_meta(&light)).unwrap();
        assert_eq!(selected.flat.len(), 3);
        assert_eq!(named(&selected.kept_out), ["flat-3.fits"]);
    }

    #[test]
    fn a_light_whose_only_flats_are_poor_cannot_be_calibrated() {
        let temp = tempfile::tempdir().unwrap();
        let mut frames = Vec::new();
        for index in 0..3 {
            let path = temp.path().join(format!("flat-{index}.fits"));
            // Past 92% of full scale the sensor has left its linear range.
            write_frame(&path, "FLAT", FLAT_SIZE, "2026-09-10T02:00:00", |x, y| {
                62_000.0 + 20.0 * noise(x, y, index)
            });
            frames.push(read_frame_meta(&path));
        }
        let conn = catalog(&temp.path().join("catalog.sqlite"), &frames);
        let graded = measure_and_grade(&conn).unwrap().1;
        assert_eq!(graded.poor, 3, "{graded:?}");
        let light = temp.path().join("light.fits");
        write_frame(&light, "LIGHT", FLAT_SIZE, "2026-09-10T05:00:00", |_, _| {
            1_000.0
        });
        let light = read_frame_meta(&light);
        let selected = select_for_light(&conn, &light).unwrap();
        assert!(selected.flat.is_empty());
        assert!(selected.library.flat, "the library holds flats for it");
        assert_eq!(
            super::super::calibration_gap(&conn, &light, &selected, 12 * 3600)
                .unwrap()
                .as_deref(),
            Some("No matching flat")
        );
    }

    #[test]
    fn a_frame_seiza_cannot_measure_is_recorded_once() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("flat.fits");
        write_frame(&path, "FLAT", (8, 8), "2026-09-10T02:00:00", |_, _| {
            30_000.0
        });
        let conn = catalog(
            &temp.path().join("catalog.sqlite"),
            &[read_frame_meta(&path)],
        );
        let (measured, graded) = measure_and_grade(&conn).unwrap();
        assert_eq!((measured.refused, graded.graded), (1, 0));
        assert!(pending_measurements(&conn).unwrap().is_empty());
        let summary = &library_details(&conn).unwrap().frames[0];
        assert_eq!(summary.quality.grade, None);
        assert_eq!(summary.kept_out, None, "an ungraded frame serves");
    }

    #[test]
    fn a_missing_file_waits_for_the_next_pass() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("dark.fits");
        let meta = dark(&path, "DARK", 500.0, 1);
        let conn = catalog(&temp.path().join("catalog.sqlite"), &[meta]);
        std::fs::rename(&path, temp.path().join("elsewhere.fits")).unwrap();
        let (measured, _) = measure_and_grade(&conn).unwrap();
        assert_eq!((measured.pending, measured.unreadable), (1, 1));
        assert_eq!(pending_measurements(&conn).unwrap().len(), 1);
    }

    fn recorded_version(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT version FROM psf_guard_calibration_schema",
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn a_catalog_from_0_11_gains_the_grade_table_and_keeps_its_marks() {
        let temp = tempfile::tempdir().unwrap();
        let (conn, light) = dark_library(temp.path());
        // As 0.11 left it: no grade table, one dark marked for stray light.
        conn.execute_batch(
            "DROP TABLE psf_guard_calibration_frame_quality;
             UPDATE psf_guard_calibration_frame SET stray_light = 'stray light: by 0.11'
             WHERE source_path LIKE '%dark-3.fits';",
        )
        .unwrap();
        // Without the table it still keeps the marked dark out.
        let selected = select_for_light(&conn, &light).unwrap();
        assert_eq!(named(&selected.kept_out), ["dark-3.fits"]);
        // Opened read-only, it is left as it is and nothing complains.
        let read_only = Connection::open_with_flags(
            temp.path().join("catalog.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert!(!migrate_existing(&read_only).unwrap());
        assert!(!table_exists(&read_only));
        drop(read_only);

        assert!(migrate_existing(&conn).unwrap(), "the table is added");
        assert!(table_exists(&conn));
        assert_eq!(recorded_version(&conn), 9, "and the version stays");
        assert_eq!(
            named(&select_for_light(&conn, &light).unwrap().kept_out),
            ["dark-3.fits"],
            "an ungraded frame keeps its mark"
        );
        assert!(
            !migrate_existing(&conn).unwrap(),
            "a second open does nothing"
        );
    }

    #[test]
    fn a_graded_catalog_stays_at_version_nine_for_older_readers() {
        // 0.11.0 refuses to import into, sync into or record masters in a
        // catalog above version 9, its own. A graded catalog must stay a
        // version 9 catalog that its ensure_schema accepts.
        assert_eq!(
            super::super::CALIBRATION_SCHEMA_VERSION,
            9,
            "the grade table must not raise the version older builds check"
        );
        let temp = tempfile::tempdir().unwrap();
        let (mut conn, _) = dark_library(temp.path());
        let frame_columns = |conn: &Connection| {
            super::super::table_column_names(conn, "psf_guard_calibration_frame")
        };
        let before = frame_columns(&conn).unwrap();
        measure_and_grade(&conn).unwrap();
        assert!(table_exists(&conn));
        assert_eq!(recorded_version(&conn), 9);
        super::super::ensure_schema(&conn).unwrap();
        assert_eq!(recorded_version(&conn), 9);

        // What an older reader sees, the frame table alone, keeps its shape,
        // and its stray-light mark is all the grade tells it: the leaked
        // dark is marked, nothing else is.
        assert_eq!(frame_columns(&conn).unwrap(), before);
        assert!(super::super::schema_supports_current_reads(&conn));
        let marked: Vec<(String, Option<String>)> = conn
            .prepare(
                "SELECT source_path, stray_light FROM psf_guard_calibration_frame
                 ORDER BY source_path",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(marked.len(), 7);
        for (path, mark) in &marked {
            assert_eq!(
                mark.is_some(),
                path.ends_with("dark-3.fits"),
                "{path}: {mark:?}"
            );
        }
        // The write path every import takes runs ensure_schema too.
        let path = temp.path().join("late-dark.fits");
        let late = dark(&path, "DARK", 503.0, 99);
        let tx = conn.transaction().unwrap();
        import_calibration_frames(&tx, &[late], Some("profile")).unwrap();
        tx.commit().unwrap();
        assert_eq!(recorded_version(&conn), 9);
        assert_eq!(pending_measurements(&conn).unwrap().len(), 1);
    }

    #[test]
    fn a_catalog_a_newer_build_wrote_is_left_ungraded() {
        let temp = tempfile::tempdir().unwrap();
        let (conn, _) = dark_library(temp.path());
        conn.execute(
            "UPDATE psf_guard_calibration_schema SET version = ?1",
            [super::super::CALIBRATION_SCHEMA_VERSION + 1],
        )
        .unwrap();
        assert!(pending_measurements(&conn).unwrap().is_empty());
        assert_eq!(grade_library(&conn).unwrap(), GradeOutcome::default());
    }

    #[test]
    fn sync_carries_grades_and_the_newer_choice() {
        let temp = tempfile::tempdir().unwrap();
        let (source, light) = dark_library(temp.path());
        measure_and_grade(&source).unwrap();
        let quiet = uuid_of(&source, "dark-0.fits");
        set_overrides(
            &source,
            std::slice::from_ref(&quiet),
            Some(QualityOverride::Exclude),
        )
        .unwrap();

        let mut destination = Connection::open(temp.path().join("copy.sqlite")).unwrap();
        let sync = |destination: &mut Connection| {
            let tx = destination.transaction().unwrap();
            let outcome = sync_library(&source, &tx).unwrap();
            tx.commit().unwrap();
            outcome.quality
        };
        assert_eq!(sync(&mut destination).inserted, 7);
        let selected = select_for_light(&destination, &light).unwrap();
        assert_eq!(named(&selected.kept_out), ["dark-0.fits", "dark-3.fits"]);
        assert!(pending_measurements(&destination).unwrap().is_empty());
        assert_eq!(sync(&mut destination).unchanged, 7);

        // A choice cleared later on the copy outlives the older one.
        destination
            .execute(
                "UPDATE psf_guard_calibration_frame_quality
                 SET override_choice = NULL, override_at = override_at + 10
                 WHERE frame_uuid = ?1",
                [&quiet],
            )
            .unwrap();
        assert_eq!(sync(&mut destination).unchanged, 7);
        let selected = select_for_light(&destination, &light).unwrap();
        assert_eq!(named(&selected.kept_out), ["dark-3.fits"]);

        // A newer choice on the source comes across.
        source
            .execute(
                "UPDATE psf_guard_calibration_frame_quality
                 SET override_choice = 'use', override_at = ?2
                 WHERE frame_uuid = ?1",
                params![
                    uuid_of(&source, "dark-3.fits"),
                    chrono::Utc::now().timestamp() + 100
                ],
            )
            .unwrap();
        assert_eq!(sync(&mut destination).updated, 1);
        assert!(select_for_light(&destination, &light)
            .unwrap()
            .kept_out
            .is_empty());
    }

    #[test]
    fn masters_record_their_inputs_and_whether_they_are_thin() {
        let temp = tempfile::tempdir().unwrap();
        let mut frames = Vec::new();
        for index in 0..4 {
            frames.push(flat(
                &temp.path().join(format!("flat-{index}.fits")),
                30 + index,
                false,
            ));
        }
        for index in 0..3 {
            let path = temp.path().join(format!("bias-{index}.fits"));
            write_frame(&path, "BIAS", FLAT_SIZE, "2026-09-10T10:00:00", |x, y| {
                500.0 + 5.0 * noise(x, y, 40 + index)
            });
            frames.push(read_frame_meta(&path));
        }
        let conn = catalog(&temp.path().join("catalog.sqlite"), &frames);
        measure_and_grade(&conn).unwrap();
        let light = temp.path().join("flat-light.fits");
        write_frame(&light, "LIGHT", FLAT_SIZE, "2026-09-10T05:00:00", |_, _| {
            1_000.0
        });
        let (_, applied) = resolve_or_build_masters(
            &conn,
            &temp.path().join("cache"),
            std::slice::from_ref(&light),
            None,
            None,
            CalibrationMode::Auto,
        )
        .unwrap();
        assert!(applied.flat_master.is_some(), "{applied:?}");
        assert!(applied.bias_master.is_some(), "{applied:?}");
        let statistics = |kind: &str| -> serde_json::Value {
            let json: String = conn
                .query_row(
                    "SELECT statistics_json FROM psf_guard_calibration_master WHERE kind = ?1",
                    [kind],
                    |row| row.get(0),
                )
                .unwrap();
            serde_json::from_str(&json).unwrap()
        };
        let bias = statistics("bias");
        assert_eq!(bias["thin"], true, "three frames: {bias}");
        assert_eq!(bias["thin_below"], THIN_MASTER_FRAMES);
        assert_eq!(bias["input_statistics"].as_array().unwrap().len(), 3);
        assert!(bias["input_statistics"][0]["frame_uuid"].is_string());
        let flat = statistics("flat");
        assert!(flat["defect_pixels_replaced"].is_u64(), "{flat}");
        assert!(
            flat.get("thin").is_none(),
            "only bias and dark masters are thin"
        );
        assert_eq!(flat["frame_quality"]["measured_frames"], 4);
        assert_eq!(flat["frame_quality"]["master"]["grade"], "ok", "{flat}");
        assert_eq!(flat["frame_quality"]["carried_features"], 0);
    }

    #[test]
    fn a_poor_frame_changes_the_master_it_would_have_joined() {
        let temp = tempfile::tempdir().unwrap();
        let mut frames = Vec::new();
        for (index, level) in [503.0, 503.0, 503.0, 900.0].into_iter().enumerate() {
            frames.push(dark(
                &temp.path().join(format!("dark-{index}.fits")),
                "DARK",
                level,
                index as u64,
            ));
        }
        let conn = catalog(&temp.path().join("catalog.sqlite"), &frames);
        let light = temp.path().join("light.fits");
        write_frame(&light, "LIGHT", DARK_SIZE, "2026-09-11T03:00:00", |_, _| {
            1_100.0
        });
        let fingerprint =
            |conn: &Connection| super::super::selection_fingerprint(conn, &light, None).unwrap();
        let before = fingerprint(&conn);
        measure_and_grade(&conn).unwrap();
        let graded = fingerprint(&conn);
        assert_ne!(before, graded, "a stack built before the grade goes stale");
        let leaked = uuid_of(&conn, "dark-3.fits");
        set_overrides(
            &conn,
            std::slice::from_ref(&leaked),
            Some(QualityOverride::Use),
        )
        .unwrap();
        assert_eq!(
            fingerprint(&conn),
            before,
            "using it anyway restores the set"
        );
    }
}
