//! Browsing the library: frames a page at a time, the sessions they were
//! shot in, one frame's measurement beside its session's, and the masters
//! built from them.
//!
//! A session is the frames of one kind shot one night with one set of
//! capture settings: the sensor mode, size and binning, then for flats the
//! filter and rotator angle, for darks and dark-flats the exposure and
//! sensor temperature, for bias the exposure. A temperature or an angle
//! drifts, so frames join a session while each sits within a small step of
//! the next. Its id is a digest of those settings, so it stays the same
//! while frames are graded or filtered out, and a link to it keeps working.

use super::quality::{self, StoredMeasurement};
use super::{
    catalog_night_boundary, frame_summaries, night_of, schema_exists, CalibrationFrameSummary,
    CalibrationKind, FrameSpan,
};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use seiza_calibration::Finding;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Rows per page when a caller does not say.
pub const DEFAULT_PAGE_SIZE: usize = 100;
/// The most rows one page holds.
pub const MAX_PAGE_SIZE: usize = 500;
/// The flats a session needs before one can be compared with the rest.
pub const MIN_COMPARE_FLATS: usize = 3;
/// The deepest flat features a frame's detail carries, for the overlay.
const DETAIL_FEATURES: usize = 400;
/// Exposures within this many seconds are one exposure, as for matching.
const EXPOSURE_TOLERANCE_S: f64 = 0.05;

/// What a list keeps: every field is optional, and each one given narrows
/// the list further.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FrameFilter {
    pub rig: Option<String>,
    pub kind: Option<CalibrationKind>,
    /// A filter name, matched without case.
    pub filter: Option<String>,
    /// Seconds, matched within [`EXPOSURE_TOLERANCE_S`].
    pub exposure: Option<f64>,
    pub temp_min: Option<f64>,
    pub temp_max: Option<f64>,
    /// First and last night kept, as `YYYY-MM-DD`.
    pub night_from: Option<String>,
    pub night_to: Option<String>,
    pub grade: Option<GradeFilter>,
    /// `true` keeps the frames left out of masters, `false` the rest.
    pub kept_out: Option<bool>,
    pub session: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GradeFilter {
    Ok,
    Suspect,
    Poor,
    Ungraded,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameSort {
    #[default]
    Newest,
    Oldest,
    File,
    /// Poor first, then suspect, ok and ungraded; newest first within each.
    Grade,
}

/// One frame of a list, with the night and session it belongs to.
#[derive(Debug, Clone, Serialize)]
pub struct BrowsedFrame {
    #[serde(flatten)]
    pub frame: CalibrationFrameSummary,
    /// `YYYY-MM-DD`; absent when the frame has no capture time.
    pub night: Option<String>,
    pub session: String,
}

/// One page of frames.
#[derive(Debug, Clone, Serialize)]
pub struct FramePage {
    pub frames: Vec<BrowsedFrame>,
    /// Frames the filters keep, across every page.
    pub total: usize,
    pub page: usize,
    pub per_page: usize,
}

/// Frames of one kind from one night and one set of capture settings.
#[derive(Debug, Clone, Serialize)]
pub struct CalibrationSession {
    pub id: String,
    pub kind: CalibrationKind,
    pub night: Option<String>,
    pub rig_uuid: String,
    pub rig_name: Option<String>,
    pub is_master: bool,
    pub filter: Option<String>,
    pub exposure_min_s: Option<f64>,
    pub exposure_max_s: Option<f64>,
    pub temp_min_c: Option<f64>,
    pub temp_max_c: Option<f64>,
    pub gain: Option<i64>,
    pub offset: Option<i64>,
    pub readout: Option<String>,
    pub binning: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub rotation: Option<f64>,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    /// Frames the filters keep; the counts below are of these.
    pub frames: usize,
    pub ok: usize,
    pub suspect: usize,
    pub poor: usize,
    pub ungraded: usize,
    pub kept_out: usize,
}

/// Values the filters can take, from the whole library.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LibraryFacets {
    pub filters: Vec<String>,
    pub exposures: Vec<f64>,
    pub first_night: Option<String>,
    pub last_night: Option<String>,
    pub temp_min_c: Option<f64>,
    pub temp_max_c: Option<f64>,
}

/// One page of sessions, newest night first.
#[derive(Debug, Clone, Serialize)]
pub struct SessionPage {
    pub sessions: Vec<CalibrationSession>,
    pub total: usize,
    pub page: usize,
    pub per_page: usize,
    /// Frames the filters keep, across every session.
    pub frames: usize,
    pub facets: LibraryFacets,
    pub night_boundary_seconds: i64,
}

/// One number a frame was measured by, beside its session's.
#[derive(Debug, Clone, Serialize)]
pub struct FrameMetric {
    pub key: &'static str,
    pub value: f64,
    /// The median among the other measured frames of its session.
    pub session: Option<f64>,
}

/// A limit the frame crossed, with the numbers behind it.
#[derive(Debug, Clone, Serialize)]
pub struct FindingDetail {
    #[serde(flatten)]
    pub finding: Finding,
    /// In plain words, with its number.
    pub text: String,
}

/// Everything the browser shows about one frame.
#[derive(Debug, Clone, Serialize)]
pub struct FrameDetail {
    #[serde(flatten)]
    pub frame: BrowsedFrame,
    /// What was measured, as stored; a flat keeps its deepest features.
    pub measurement: Option<serde_json::Value>,
    /// Why the frame could not be measured, when it could not.
    pub measure_error: Option<String>,
    pub findings: Vec<FindingDetail>,
    pub metrics: Vec<FrameMetric>,
    /// Frames in its session, and how many of them were measured.
    pub session_frames: usize,
    pub session_measured: usize,
    /// Whether the frame can be shown against its session: a flat in a
    /// session of at least [`MIN_COMPARE_FLATS`].
    pub compare: bool,
    /// Recorded masters built from this frame.
    pub masters: usize,
}

/// Every frame of the library, with its night and session, oldest catalog
/// version tolerated.
fn browsed(conn: &Connection) -> Result<(Vec<BrowsedFrame>, i64)> {
    let boundary = catalog_night_boundary(conn);
    Ok((
        assign_sessions(frame_summaries(conn, "", &[])?, boundary),
        boundary,
    ))
}

/// The frames of one rig and kind, each with its session: all a session's
/// members, read without the rest of the library.
fn browsed_like(
    conn: &Connection,
    rig_uuid: &str,
    kind: CalibrationKind,
) -> Result<Vec<BrowsedFrame>> {
    let kind = kind.as_str();
    let rows = frame_summaries(
        conn,
        "WHERE f.rig_uuid = ?1 AND f.kind = ?2",
        &[&rig_uuid, &kind],
    )?;
    Ok(assign_sessions(rows, catalog_night_boundary(conn)))
}

/// A temperature or an angle that drifts while a set is shot: frames join
/// one session while each is within `step` of the next.
fn drift(frame: &CalibrationFrameSummary) -> Option<(f64, f64)> {
    match frame.kind {
        // A flat shot warm is a good flat; the angle is what ties a set.
        CalibrationKind::Flat => frame.rotation.map(|angle| (angle, 1.0)),
        CalibrationKind::Dark | CalibrationKind::DarkFlat => {
            frame.camera_temp.map(|temp| (temp, 1.5))
        }
        // Bias barely changes with temperature, and is often shot while
        // the sensor cools.
        CalibrationKind::Bias => None,
    }
    .filter(|(value, _)| value.is_finite())
}

/// The settings every frame of a session shares, before any drift.
fn session_base(frame: &CalibrationFrameSummary, night: Option<&str>) -> String {
    let text = |value: Option<i64>| value.map(|value| value.to_string()).unwrap_or_default();
    let mut parts = vec![
        frame.rig_uuid.clone(),
        frame.kind.as_str().to_string(),
        night.unwrap_or("unknown").to_string(),
        frame.is_master.to_string(),
        text(frame.width),
        text(frame.height),
        text(frame.binning_x),
        text(frame.binning_y),
        text(frame.gain),
        text(frame.offset),
        readout(frame).unwrap_or_default().to_ascii_lowercase(),
    ];
    if frame.kind == CalibrationKind::Flat {
        // Flats of one set vary their exposure to reach a level.
        parts.push(filter_key(frame.filter.as_deref()).unwrap_or_default());
    } else {
        parts.push(
            frame
                .exposure_s
                .filter(|value| value.is_finite())
                .map(|value| ((value * 100.0).round() as i64).to_string())
                .unwrap_or_default(),
        );
    }
    parts.join("|")
}

fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Give each frame its night and session.
fn assign_sessions(rows: Vec<CalibrationFrameSummary>, boundary: i64) -> Vec<BrowsedFrame> {
    let nights: Vec<Option<String>> = rows
        .iter()
        .map(|frame| frame.captured_at.map(|at| night_of(at, boundary)))
        .collect();
    let bases: Vec<String> = rows
        .iter()
        .zip(&nights)
        .map(|(frame, night)| session_base(frame, night.as_deref()))
        .collect();
    // Within each base, frames sorted by their drift; a gap wider than the
    // step starts another session, named by where it starts.
    let mut groups: HashMap<&str, Vec<(f64, f64, usize)>> = HashMap::new();
    for (index, frame) in rows.iter().enumerate() {
        if let Some((value, step)) = drift(frame) {
            groups
                .entry(bases[index].as_str())
                .or_default()
                .push((value, step, index));
        }
    }
    let mut tags: Vec<String> = vec![String::new(); rows.len()];
    for members in groups.values_mut() {
        members.sort_by(|left, right| left.0.total_cmp(&right.0));
        let mut start = f64::NAN;
        let mut previous = f64::NAN;
        for &(value, step, index) in members.iter() {
            let joins = previous.is_finite() && value - previous <= step;
            if !joins {
                start = value;
            }
            previous = value;
            tags[index] = format!("{:.1}", start);
        }
    }
    rows.into_iter()
        .zip(nights)
        .enumerate()
        .map(|(index, (frame, night))| BrowsedFrame {
            session: digest(&format!("{}|{}", bases[index], tags[index])),
            night,
            frame,
        })
        .collect()
}

fn filter_key(filter: Option<&str>) -> Option<String> {
    filter
        .map(|filter| filter.trim().to_ascii_lowercase())
        .filter(|filter| !filter.is_empty())
}

fn readout(frame: &CalibrationFrameSummary) -> Option<String> {
    frame
        .readout_mode_name
        .clone()
        .or_else(|| frame.readout_mode.map(|mode| mode.to_string()))
}

impl FrameFilter {
    fn keeps(&self, row: &BrowsedFrame) -> bool {
        let frame = &row.frame;
        if self.rig.as_deref().is_some_and(|rig| rig != frame.rig_uuid)
            || self.kind.is_some_and(|kind| kind != frame.kind)
            || self.session.as_deref().is_some_and(|id| id != row.session)
        {
            return false;
        }
        if let Some(wanted) = filter_key(self.filter.as_deref())
            && filter_key(frame.filter.as_deref()).as_deref() != Some(wanted.as_str())
        {
            return false;
        }
        if let Some(wanted) = self.exposure
            && !frame
                .exposure_s
                .is_some_and(|exposure| (exposure - wanted).abs() <= EXPOSURE_TOLERANCE_S)
        {
            return false;
        }
        if self.temp_min.is_some() || self.temp_max.is_some() {
            let Some(temp) = frame.camera_temp else {
                return false;
            };
            if self.temp_min.is_some_and(|min| temp < min)
                || self.temp_max.is_some_and(|max| temp > max)
            {
                return false;
            }
        }
        if self.night_from.is_some() || self.night_to.is_some() {
            let Some(night) = row.night.as_deref() else {
                return false;
            };
            if self.night_from.as_deref().is_some_and(|from| night < from)
                || self.night_to.as_deref().is_some_and(|to| night > to)
            {
                return false;
            }
        }
        if let Some(grade) = self.grade {
            let actual = frame.quality.grade.as_deref();
            let keeps = match grade {
                GradeFilter::Ok => actual == Some("ok"),
                GradeFilter::Suspect => actual == Some("suspect"),
                GradeFilter::Poor => actual == Some("poor"),
                GradeFilter::Ungraded => actual.is_none(),
            };
            if !keeps {
                return false;
            }
        }
        if let Some(kept_out) = self.kept_out
            && frame.kept_out.is_some() != kept_out
        {
            return false;
        }
        true
    }
}

fn grade_rank(frame: &CalibrationFrameSummary) -> u8 {
    match frame.quality.grade.as_deref() {
        Some("poor") => 0,
        Some("suspect") => 1,
        Some("ok") => 2,
        _ => 3,
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn sort_frames(frames: &mut [BrowsedFrame], sort: FrameSort) {
    let newest = |left: &BrowsedFrame, right: &BrowsedFrame| {
        right
            .frame
            .captured_at
            .cmp(&left.frame.captured_at)
            .then_with(|| left.frame.source_path.cmp(&right.frame.source_path))
    };
    match sort {
        FrameSort::Newest => frames.sort_by(newest),
        FrameSort::Oldest => frames.sort_by(|left, right| newest(right, left)),
        FrameSort::File => frames.sort_by(|left, right| {
            file_name(&left.frame.source_path)
                .to_lowercase()
                .cmp(&file_name(&right.frame.source_path).to_lowercase())
                .then_with(|| newest(left, right))
        }),
        FrameSort::Grade => frames.sort_by(|left, right| {
            grade_rank(&left.frame)
                .cmp(&grade_rank(&right.frame))
                .then_with(|| newest(left, right))
        }),
    }
}

fn page_bounds(total: usize, page: usize, per_page: Option<usize>) -> (usize, usize, usize) {
    let per_page = per_page
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let start = page.saturating_mul(per_page).min(total);
    (per_page, start, (start + per_page).min(total))
}

/// The library's frames in one read, so the lists can be narrowed (by
/// whether a file is there, say) without holding the database.
pub struct LibraryFrames {
    pub frames: Vec<BrowsedFrame>,
    rigs: HashMap<String, String>,
    pub night_boundary_seconds: i64,
}

/// Read every frame of the library, with its night and session.
pub fn library_frames(conn: &Connection) -> Result<LibraryFrames> {
    let (frames, night_boundary_seconds) = browsed(conn)?;
    Ok(LibraryFrames {
        frames,
        rigs: rig_names(conn)?,
        night_boundary_seconds,
    })
}

impl LibraryFrames {
    /// The frames `filter` keeps, in library order.
    pub fn kept<'a>(&'a self, filter: &'a FrameFilter) -> impl Iterator<Item = &'a BrowsedFrame> {
        self.frames.iter().filter(move |row| filter.keeps(row))
    }

    /// One page of the frames `filter` and `also` keep, in `sort` order.
    pub fn frames_page(
        self,
        filter: &FrameFilter,
        also: impl Fn(&BrowsedFrame) -> bool,
        sort: FrameSort,
        page: usize,
        per_page: Option<usize>,
    ) -> FramePage {
        let mut kept: Vec<BrowsedFrame> = self
            .frames
            .into_iter()
            .filter(|row| filter.keeps(row) && also(row))
            .collect();
        sort_frames(&mut kept, sort);
        let total = kept.len();
        let (per_page, start, end) = page_bounds(total, page, per_page);
        FramePage {
            frames: kept.drain(start..end).collect(),
            total,
            page,
            per_page,
        }
    }

    /// One page of the sessions holding frames `filter` and `also` keep,
    /// newest night first, with what the filters can choose from.
    pub fn sessions_page(
        &self,
        filter: &FrameFilter,
        also: impl Fn(&BrowsedFrame) -> bool,
        page: usize,
        per_page: Option<usize>,
    ) -> SessionPage {
        let mut sessions = sessions_of(
            self.frames
                .iter()
                .filter(|row| filter.keeps(row) && also(row)),
            &self.rigs,
        );
        let frames = sessions.iter().map(|session| session.frames).sum();
        let total = sessions.len();
        let (per_page, start, end) = page_bounds(total, page, per_page);
        SessionPage {
            sessions: sessions.drain(start..end).collect(),
            total,
            page,
            per_page,
            frames,
            facets: facets(&self.frames),
            night_boundary_seconds: self.night_boundary_seconds,
        }
    }
}

fn kind_order(kind: CalibrationKind) -> u8 {
    match kind {
        CalibrationKind::Flat => 0,
        CalibrationKind::DarkFlat => 1,
        CalibrationKind::Dark => 2,
        CalibrationKind::Bias => 3,
    }
}

fn widen(range: &mut (Option<f64>, Option<f64>), value: Option<f64>) {
    if let Some(value) = value.filter(|value| value.is_finite()) {
        range.0 = Some(range.0.map_or(value, |low: f64| low.min(value)));
        range.1 = Some(range.1.map_or(value, |high: f64| high.max(value)));
    }
}

/// Group frames into sessions, newest night first.
fn sessions_of<'a>(
    rows: impl Iterator<Item = &'a BrowsedFrame>,
    rigs: &HashMap<String, String>,
) -> Vec<CalibrationSession> {
    let mut sessions: BTreeMap<String, CalibrationSession> = BTreeMap::new();
    let mut exposures: HashMap<String, (Option<f64>, Option<f64>)> = HashMap::new();
    let mut temps: HashMap<String, (Option<f64>, Option<f64>)> = HashMap::new();
    for row in rows {
        let frame = &row.frame;
        let session = sessions
            .entry(row.session.clone())
            .or_insert_with(|| CalibrationSession {
                id: row.session.clone(),
                kind: frame.kind,
                night: row.night.clone(),
                rig_uuid: frame.rig_uuid.clone(),
                rig_name: rigs.get(&frame.rig_uuid).cloned(),
                is_master: frame.is_master,
                filter: frame.filter.clone(),
                exposure_min_s: None,
                exposure_max_s: None,
                temp_min_c: None,
                temp_max_c: None,
                gain: frame.gain,
                offset: frame.offset,
                readout: readout(frame),
                binning: frame
                    .binning_x
                    .map(|x| format!("{x}x{}", frame.binning_y.unwrap_or(x))),
                width: frame.width,
                height: frame.height,
                rotation: frame.rotation,
                first_at: None,
                last_at: None,
                frames: 0,
                ok: 0,
                suspect: 0,
                poor: 0,
                ungraded: 0,
                kept_out: 0,
            });
        session.frames += 1;
        match frame.quality.grade.as_deref() {
            Some("ok") => session.ok += 1,
            Some("suspect") => session.suspect += 1,
            Some("poor") => session.poor += 1,
            _ => session.ungraded += 1,
        }
        if frame.kept_out.is_some() {
            session.kept_out += 1;
        }
        if let Some(at) = frame.captured_at {
            session.first_at = Some(session.first_at.map_or(at, |first| first.min(at)));
            session.last_at = Some(session.last_at.map_or(at, |last| last.max(at)));
        }
        widen(
            exposures.entry(row.session.clone()).or_default(),
            frame.exposure_s,
        );
        widen(
            temps.entry(row.session.clone()).or_default(),
            frame.camera_temp,
        );
    }
    let mut sessions: Vec<CalibrationSession> = sessions
        .into_values()
        .map(|mut session| {
            (session.exposure_min_s, session.exposure_max_s) =
                exposures.remove(&session.id).unwrap_or_default();
            (session.temp_min_c, session.temp_max_c) =
                temps.remove(&session.id).unwrap_or_default();
            session
        })
        .collect();
    sessions.sort_by(|left, right| {
        // Nights without a date go last.
        let night = |session: &CalibrationSession| session.night.clone().unwrap_or_default();
        night(right)
            .cmp(&night(left))
            .then_with(|| kind_order(left.kind).cmp(&kind_order(right.kind)))
            .then_with(|| left.filter.cmp(&right.filter))
            .then_with(|| compare_option(left.exposure_min_s, right.exposure_min_s))
            .then_with(|| left.first_at.cmp(&right.first_at))
            .then_with(|| left.id.cmp(&right.id))
    });
    sessions
}

fn compare_option(left: Option<f64>, right: Option<f64>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.total_cmp(&right),
        (left, right) => left.is_some().cmp(&right.is_some()),
    }
}

fn facets(frames: &[BrowsedFrame]) -> LibraryFacets {
    let mut filters = BTreeMap::new();
    let mut exposures = BTreeSet::new();
    let mut nights = BTreeSet::new();
    let mut temps = (None, None);
    for row in frames {
        if let Some(filter) = row.frame.filter.as_deref().map(str::trim)
            && !filter.is_empty()
        {
            filters
                .entry(filter.to_ascii_lowercase())
                .or_insert_with(|| filter.to_string());
        }
        if let Some(exposure) = row.frame.exposure_s.filter(|value| value.is_finite()) {
            // Hundredths, so a list of flats' exposures stays short.
            exposures.insert((exposure * 100.0).round() as i64);
        }
        nights.extend(row.night.clone());
        widen(&mut temps, row.frame.camera_temp);
    }
    LibraryFacets {
        filters: filters.into_values().collect(),
        exposures: exposures
            .into_iter()
            .map(|hundredths| hundredths as f64 / 100.0)
            .collect(),
        first_night: nights.first().cloned(),
        last_night: nights.last().cloned(),
        temp_min_c: temps.0,
        temp_max_c: temps.1,
    }
}

fn rig_names(conn: &Connection) -> Result<HashMap<String, String>> {
    if !schema_exists(conn) {
        return Ok(HashMap::new());
    }
    let mut statement = conn.prepare("SELECT rig_uuid, name FROM psf_guard_rig")?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// One frame's own row, without its session: one query, for a caller that
/// only reads its file.
pub fn frame_row(conn: &Connection, frame_uuid: &str) -> Result<Option<CalibrationFrameSummary>> {
    Ok(
        frame_summaries(conn, "WHERE f.frame_uuid = ?1", &[&frame_uuid])?
            .into_iter()
            .next(),
    )
}

/// One frame and every frame of its session, it included, oldest first.
pub fn frame_with_session(
    conn: &Connection,
    frame_uuid: &str,
) -> Result<Option<(BrowsedFrame, Vec<BrowsedFrame>)>> {
    let Some(row) = frame_row(conn, frame_uuid)? else {
        return Ok(None);
    };
    let like = browsed_like(conn, &row.rig_uuid, row.kind)?;
    let Some(frame) = like
        .iter()
        .find(|browsed| browsed.frame.frame_uuid == frame_uuid)
        .cloned()
    else {
        return Ok(None);
    };
    let mut members: Vec<BrowsedFrame> = like
        .into_iter()
        .filter(|member| member.session == frame.session)
        .collect();
    sort_frames(&mut members, FrameSort::Oldest);
    Ok(Some((frame, members)))
}

/// The stored measurement, findings and error of some frames.
struct StoredQuality {
    measurement: Option<StoredMeasurement>,
    findings: Vec<Finding>,
    error: Option<String>,
}

fn stored_quality(
    conn: &Connection,
    frame_uuids: &[&str],
) -> Result<HashMap<String, StoredQuality>> {
    let mut stored = HashMap::new();
    if !quality::table_exists(conn) {
        return Ok(stored);
    }
    // Only a measurement of the file as it is now counts, as for grading.
    let mut statement = conn.prepare(
        "SELECT q.measurement_json, q.findings_json, q.measure_error
         FROM psf_guard_calibration_frame_quality q
         JOIN psf_guard_calibration_frame f ON f.frame_uuid = q.frame_uuid
         WHERE q.frame_uuid = ?1 AND q.source_fingerprint IS f.source_fingerprint",
    )?;
    for frame_uuid in frame_uuids {
        let row = statement
            .query_row([frame_uuid], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .optional()?;
        let Some((measurement, findings, error)) = row else {
            continue;
        };
        stored.insert(
            frame_uuid.to_string(),
            StoredQuality {
                measurement: measurement.and_then(|json| serde_json::from_str(&json).ok()),
                findings: findings
                    .and_then(|json| serde_json::from_str(&json).ok())
                    .unwrap_or_default(),
                error,
            },
        );
    }
    Ok(stored)
}

/// The numbers the panel lists for one measurement, by key.
fn measured_numbers(measurement: &StoredMeasurement) -> Vec<(&'static str, f64)> {
    match measurement {
        StoredMeasurement::Dark(dark) => vec![
            ("level", f64::from(dark.level)),
            ("sigma", f64::from(dark.sigma)),
            ("noise", f64::from(dark.noise)),
            ("row_banding", f64::from(dark.row_banding)),
            ("column_banding", f64::from(dark.column_banding)),
            ("hot_pixels", dark.hot_pixels as f64),
        ],
        StoredMeasurement::Flat(flat) => {
            let full_scale = f64::from(flat.full_scale).max(f64::MIN_POSITIVE);
            vec![
                ("level_fraction", f64::from(flat.level) / full_scale),
                ("clipped_fraction", flat.clipped_fraction),
                ("noise_fraction", f64::from(flat.noise)),
                ("features", flat.feature_count as f64),
            ]
        }
    }
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    values.retain(|value| value.is_finite());
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}

/// One frame, its measurement and findings, and the numbers of the other
/// frames of its session.
pub fn frame_detail(conn: &Connection, frame_uuid: &str) -> Result<Option<FrameDetail>> {
    let Some((frame, members)) = frame_with_session(conn, frame_uuid)? else {
        return Ok(None);
    };
    let uuids: Vec<&str> = members
        .iter()
        .map(|member| member.frame.frame_uuid.as_str())
        .collect();
    let stored = stored_quality(conn, &uuids)?;
    let own = stored.get(frame_uuid);
    let numbers = own
        .and_then(|own| own.measurement.as_ref())
        .map(measured_numbers)
        .unwrap_or_default();
    let peers: Vec<Vec<(&'static str, f64)>> = members
        .iter()
        .filter(|member| member.frame.frame_uuid != frame_uuid)
        .filter_map(|member| stored.get(&member.frame.frame_uuid))
        .filter_map(|peer| peer.measurement.as_ref())
        .map(measured_numbers)
        .collect();
    let metrics = numbers
        .iter()
        .map(|(key, value)| FrameMetric {
            key,
            value: *value,
            session: median(
                peers
                    .iter()
                    .filter_map(|numbers| {
                        numbers
                            .iter()
                            .find(|(other, _)| other == key)
                            .map(|(_, value)| *value)
                    })
                    .collect(),
            ),
        })
        .collect();
    let measurement = own
        .and_then(|own| own.measurement.as_ref())
        .and_then(|measurement| {
            let mut measurement = measurement.clone();
            if let StoredMeasurement::Flat(flat) = &mut measurement {
                flat.features.truncate(DETAIL_FEATURES);
            }
            serde_json::to_value(measurement).ok()
        });
    let findings = own
        .map(|own| {
            own.findings
                .iter()
                .map(|finding| FindingDetail {
                    text: quality::describe_finding(finding, frame.frame.kind),
                    finding: finding.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    let masters = masters_using(conn, frame_uuid)?;
    let session_measured = members
        .iter()
        .filter(|member| {
            stored
                .get(&member.frame.frame_uuid)
                .is_some_and(|stored| stored.measurement.is_some())
        })
        .count();
    let compare = frame.frame.kind == CalibrationKind::Flat
        && !frame.frame.is_master
        && members.len() >= MIN_COMPARE_FLATS;
    Ok(Some(FrameDetail {
        measure_error: own.and_then(|own| own.error.clone()),
        measurement,
        findings,
        metrics,
        session_frames: members.len(),
        session_measured,
        compare,
        masters,
        frame,
    }))
}

fn masters_using(conn: &Connection, frame_uuid: &str) -> Result<usize> {
    if !schema_exists(conn) {
        return Ok(0);
    }
    Ok(conn
        .query_row(
            "SELECT COUNT(*) FROM psf_guard_calibration_master m, json_each(m.source_frame_uuids) s
             WHERE s.value = ?1",
            [frame_uuid],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0) as usize)
}

/// A master the library recorded, with what its catalog row says.
#[derive(Debug, Clone, Serialize)]
pub struct RecordedLibraryMaster {
    pub master_uuid: String,
    pub kind: CalibrationKind,
    pub rig_uuid: String,
    pub rig_name: Option<String>,
    pub cache_path: std::path::PathBuf,
    pub created_at: i64,
    pub source_count: usize,
    pub exposure_s: Option<f64>,
    pub filter: Option<String>,
    pub camera_temp: Option<f64>,
    /// The source frames still in the library.
    pub sources: Option<FrameSpan>,
    pub statistics: serde_json::Value,
}

/// Every master the library recorded, newest first.
pub fn recorded_masters(conn: &Connection) -> Result<Vec<RecordedLibraryMaster>> {
    if !schema_exists(conn) {
        return Ok(Vec::new());
    }
    let boundary = catalog_night_boundary(conn);
    let rigs = rig_names(conn)?;
    let frames: HashMap<String, (Option<f64>, Option<i64>)> = conn
        .prepare("SELECT frame_uuid, camera_temp, captured_at FROM psf_guard_calibration_frame")?
        .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))?
        .collect::<rusqlite::Result<_>>()?;
    let mut statement = conn.prepare(
        "SELECT master_uuid, kind, rig_uuid, cache_path, created_at, source_count,
                source_frame_uuids, exposure_s, filter_name, camera_temp, statistics_json
         FROM psf_guard_calibration_master
         ORDER BY created_at DESC, master_uuid",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<f64>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<f64>>(9)?,
                row.get::<_, String>(10)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter_map(
            |(
                master_uuid,
                kind,
                rig_uuid,
                cache_path,
                created_at,
                source_count,
                sources,
                exposure_s,
                filter,
                camera_temp,
                statistics,
            )| {
                let kind = CalibrationKind::from_db(&kind)?;
                let sources: Vec<String> = serde_json::from_str(&sources).unwrap_or_default();
                let known: Vec<_> = sources
                    .iter()
                    .filter_map(|uuid| frames.get(uuid))
                    .map(|(temp, at)| (*temp, at.map(|at| night_of(at, boundary))))
                    .collect();
                Some(RecordedLibraryMaster {
                    rig_name: rigs.get(&rig_uuid).cloned(),
                    master_uuid,
                    kind,
                    rig_uuid,
                    cache_path: std::path::PathBuf::from(cache_path),
                    created_at,
                    source_count: usize::try_from(source_count).unwrap_or(0),
                    exposure_s,
                    filter,
                    camera_temp,
                    sources: (!known.is_empty()).then(|| FrameSpan::of(known)),
                    statistics: serde_json::from_str(&statistics)
                        .unwrap_or(serde_json::Value::Null),
                })
            },
        )
        .collect())
}

/// One recorded master by its UUID.
pub fn recorded_master(
    conn: &Connection,
    master_uuid: &str,
) -> Result<Option<RecordedLibraryMaster>> {
    Ok(recorded_masters(conn)?
        .into_iter()
        .find(|master| master.master_uuid == master_uuid))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(kind: CalibrationKind) -> CalibrationFrameSummary {
        CalibrationFrameSummary {
            frame_uuid: "f".into(),
            rig_uuid: "rig".into(),
            kind,
            source_path: "/frames/a.fits".into(),
            source_exists: false,
            captured_at: Some(1_789_077_600),
            camera: None,
            width: Some(64),
            height: Some(64),
            binning_x: Some(1),
            binning_y: Some(1),
            gain: Some(100),
            offset: Some(20),
            readout_mode: None,
            readout_mode_name: None,
            is_master: false,
            bayer_pattern: None,
            exposure_s: Some(2.5),
            camera_temp: Some(-10.0),
            filter: Some("Ha".into()),
            focal_length_mm: None,
            rotation: Some(92.4),
            valid_direction: None,
            dark_level: None,
            dark_level_sigma: None,
            stray_light: None,
            quality: Default::default(),
            kept_out: None,
        }
    }

    /// The session of each frame, in order.
    fn sessions(rows: Vec<CalibrationFrameSummary>) -> Vec<String> {
        assign_sessions(rows, 12 * 3600)
            .into_iter()
            .map(|row| row.session)
            .collect()
    }

    #[test]
    fn flats_of_one_set_share_a_session_whatever_their_exposure() {
        let first = summary(CalibrationKind::Flat);
        let mut second = first.clone();
        second.exposure_s = Some(2.9);
        second.camera_temp = Some(14.0);
        second.filter = Some(" ha ".into());
        second.rotation = Some(92.6);
        let mut turned = first.clone();
        turned.rotation = Some(181.0);
        let mut next_night = first.clone();
        next_night.captured_at = first.captured_at.map(|at| at + 86_400);
        let ids = sessions(vec![first, second, turned, next_night]);
        assert_eq!(ids[0], ids[1], "a drifting angle and exposure stay one set");
        assert_ne!(ids[0], ids[2]);
        assert_ne!(ids[0], ids[3]);
    }

    #[test]
    fn darks_split_by_exposure_and_temperature_and_bias_by_neither_drift() {
        let dark = summary(CalibrationKind::Dark);
        let mut near = dark.clone();
        near.camera_temp = Some(-9.4);
        near.filter = Some("L".into());
        let mut warm = dark.clone();
        warm.camera_temp = Some(3.0);
        let mut longer = dark.clone();
        longer.exposure_s = Some(300.0);
        let ids = sessions(vec![dark, near, warm, longer]);
        assert_eq!(ids[0], ids[1]);
        assert_ne!(ids[0], ids[2], "a warm dark is another session");
        assert_ne!(ids[0], ids[3]);

        // Bias shot while the sensor cools is one set.
        let bias: Vec<CalibrationFrameSummary> = [-4.8, -4.5, -4.0, -10.0]
            .into_iter()
            .map(|temp| CalibrationFrameSummary {
                camera_temp: Some(temp),
                exposure_s: Some(0.001),
                ..summary(CalibrationKind::Bias)
            })
            .collect();
        let ids = sessions(bias);
        assert!(ids.iter().all(|id| *id == ids[0]), "{ids:?}");
    }

    #[test]
    fn a_session_keeps_its_id_as_warmer_frames_join() {
        let first = summary(CalibrationKind::Dark);
        let mut second = first.clone();
        second.camera_temp = Some(-9.8);
        let alone = sessions(vec![first.clone()]);
        let both = sessions(vec![first, second]);
        assert_eq!(alone[0], both[0]);
        assert_eq!(both[0], both[1]);
    }

    #[test]
    fn filters_narrow_by_every_field() {
        let mut frame = summary(CalibrationKind::Dark);
        frame.quality.grade = Some("poor".into());
        frame.kept_out = Some("noise".into());
        let row = assign_sessions(vec![frame], 12 * 3600).remove(0);
        let keeps = |filter: FrameFilter| filter.keeps(&row);
        assert!(keeps(FrameFilter::default()));
        assert!(keeps(FrameFilter {
            kind: Some(CalibrationKind::Dark),
            exposure: Some(2.52),
            temp_min: Some(-12.0),
            temp_max: Some(-8.0),
            night_from: Some("2026-09-10".into()),
            night_to: Some("2026-09-10".into()),
            grade: Some(GradeFilter::Poor),
            kept_out: Some(true),
            filter: Some("HA".into()),
            rig: Some("rig".into()),
            session: Some(row.session.clone()),
        }));
        assert!(!keeps(FrameFilter {
            kind: Some(CalibrationKind::Flat),
            ..Default::default()
        }));
        assert!(!keeps(FrameFilter {
            exposure: Some(3.0),
            ..Default::default()
        }));
        assert!(!keeps(FrameFilter {
            temp_max: Some(-11.0),
            ..Default::default()
        }));
        assert!(!keeps(FrameFilter {
            night_from: Some("2026-09-11".into()),
            ..Default::default()
        }));
        assert!(!keeps(FrameFilter {
            grade: Some(GradeFilter::Ungraded),
            ..Default::default()
        }));
        assert!(!keeps(FrameFilter {
            kept_out: Some(false),
            ..Default::default()
        }));
    }

    #[test]
    fn pages_stop_at_the_end() {
        assert_eq!(page_bounds(5, 0, Some(2)), (2, 0, 2));
        assert_eq!(page_bounds(5, 2, Some(2)), (2, 4, 5));
        assert_eq!(page_bounds(5, 9, Some(2)), (2, 5, 5));
        assert_eq!(page_bounds(5, 0, Some(0)), (1, 0, 1));
        assert_eq!(page_bounds(5, 0, Some(10_000)), (MAX_PAGE_SIZE, 0, 5));
    }
}
