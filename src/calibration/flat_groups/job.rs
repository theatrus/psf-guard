//! The measuring pass: responses, comparisons and light evidence.
//!
//! 1. Every flat session in use gets a response, from at most
//!    [`metric::MAX_FRAMES`] of its flats read once; a session whose frames
//!    and files are unchanged is not read again.
//! 2. Every pair the grouping needs ([`Book::pairs_to_compare`]) is compared
//!    from the cached planes.
//! 3. Every imaging session shot inside a change's window asks its own
//!    lights which side they are on ([`super::evidence`]).
//!
//! Each step commits as it goes, so a stop loses little. The caller runs
//! this on workers from the background budget, and says which rigs it
//! works on ([`Scope`]); a rig grouped by time is never measured.

use super::evidence::{self, EVIDENCE_ALGORITHM_VERSION};
use super::metric::{self, Plane};
use super::model::{canonical_filter, Book, Side};
use super::store::{self, EvidenceFit, EvidenceRow, ResponseRow, Verdict};
use crate::calibration::{
    catalog_night_boundary, night_of, CalibrationFrame, CalibrationKind, LibrarySnapshot,
};
use anyhow::{Context, Result};
use rayon::prelude::*;
use rusqlite::Connection;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Rounds of comparisons one pass makes at most: the pairs the stored
/// comparisons ask for, then those the families they formed ask for.
const COMPARE_ROUNDS: usize = 3;

/// What one pass did.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct PassOutcome {
    pub sessions: usize,
    pub measured: usize,
    pub unreadable: usize,
    pub compared: usize,
    pub evidence: usize,
    /// Frames read from disk, flats and lights: none when the catalog's
    /// measurements are current.
    pub frames_read: usize,
    /// Imaging sessions in a change's window whose lights the image folders
    /// do not hold yet, as before a refresh has seen them.
    pub lights_missing: usize,
    /// Rigs left with work this pass could not do (flats away, lights not
    /// found): the caller tries them again later.
    pub retry: BTreeSet<String>,
    /// Whether the pass stopped before its end (`keep_going` said so).
    pub stopped: bool,
}

/// Where a pass stands, as it reports it.
#[derive(Debug, Clone, Copy)]
pub struct Progress<'a> {
    /// The rig being worked on.
    pub rig: &'a str,
    /// [`STAGE_MEASURING`], [`STAGE_COMPARING`] or [`STAGE_EVIDENCE`].
    pub stage: &'a str,
    pub done: usize,
    pub total: usize,
}

/// Some rigs of a catalog, or all of them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rigs {
    #[default]
    None,
    Some(BTreeSet<String>),
    All,
}

impl Rigs {
    /// These rigs; none when the list is empty.
    pub fn of<S: Into<String>>(rigs: impl IntoIterator<Item = S>) -> Self {
        let rigs: BTreeSet<String> = rigs.into_iter().map(Into::into).collect();
        if rigs.is_empty() {
            Rigs::None
        } else {
            Rigs::Some(rigs)
        }
    }

    pub fn contains(&self, rig: &str) -> bool {
        match self {
            Rigs::None => false,
            Rigs::Some(rigs) => rigs.contains(rig),
            Rigs::All => true,
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Rigs::None)
    }

    /// These and those.
    pub fn merge(&mut self, other: Rigs) {
        *self = match (std::mem::take(self), other) {
            (Rigs::All, _) | (_, Rigs::All) => Rigs::All,
            (Rigs::None, other) | (other, Rigs::None) => other,
            (Rigs::Some(mut mine), Rigs::Some(theirs)) => {
                mine.extend(theirs);
                Rigs::Some(mine)
            }
        };
    }
}

/// Which work a pass does, and for which rigs. A rig's flats are measured,
/// compared and then its lights asked; a rig's lights alone are only asked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Scope {
    /// Rigs whose flats changed: responses, comparisons, then evidence.
    pub flats: Rigs,
    /// Rigs whose lights changed: light evidence only.
    pub lights: Rigs,
}

impl Scope {
    /// Every rig's flats and lights: what is unfinished or stale anywhere.
    pub fn everything() -> Self {
        Self {
            flats: Rigs::All,
            lights: Rigs::All,
        }
    }

    pub fn flats(rigs: Rigs) -> Self {
        Self {
            flats: rigs,
            lights: Rigs::None,
        }
    }

    pub fn lights(rigs: Rigs) -> Self {
        Self {
            flats: Rigs::None,
            lights: rigs,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.flats.is_empty() && self.lights.is_empty()
    }

    pub fn merge(&mut self, other: Scope) {
        self.flats.merge(other.flats);
        self.lights.merge(other.lights);
    }

    /// Whether this rig's flat sessions are measured and compared.
    pub fn measures(&self, rig: &str) -> bool {
        self.flats.contains(rig)
    }

    /// Whether this rig's lights are asked which side of a change they lie.
    pub fn reads_lights(&self, rig: &str) -> bool {
        self.flats.contains(rig) || self.lights.contains(rig)
    }
}

/// Where a pass's reads and measurements run.
pub trait Workers: Sync {
    /// Run `work` on these workers and wait for it.
    fn run(&self, work: &mut (dyn FnMut() + Send));
}

impl Workers for rayon::ThreadPool {
    fn run(&self, work: &mut (dyn FnMut() + Send)) {
        self.install(work)
    }
}

/// What the pass needs from its caller.
pub struct Pass<'a> {
    pub conn: &'a Connection,
    /// Where response planes are kept.
    pub dir: &'a Path,
    /// The rigs and work this pass does. A rig grouped by time is left
    /// out whatever the scope says.
    pub scope: Scope,
    /// Asked before each piece of work; false ends the pass.
    pub keep_going: &'a (dyn Fn() -> bool + Sync),
    /// Called before each read: waits while interactive work runs.
    pub yield_now: &'a (dyn Fn() + Sync),
    /// Where the pass stands, before each step and at each stage's end.
    pub progress: &'a mut dyn FnMut(&Progress<'_>),
    /// Rigs worked on first, as one a page shows.
    pub goes_first: &'a dyn Fn(&str) -> bool,
    /// A light's file from its name, when the image folders hold it.
    pub find_light: &'a dyn Fn(&str) -> Option<PathBuf>,
    /// The workers reads and measurements run on; the calling thread when
    /// absent.
    pub pool: Option<&'a dyn Workers>,
}

impl Pass<'_> {
    fn in_pool<R: Send>(&self, work: impl FnOnce() -> R + Send) -> R {
        match self.pool {
            Some(pool) => {
                let mut work = Some(work);
                let mut out = None;
                pool.run(&mut || out = work.take().map(|work| work()));
                out.expect("the workers ran the work")
            }
            None => work(),
        }
    }
}

/// The stages a pass reports, in order.
pub const STAGE_MEASURING: &str = "measuring flat sessions";
pub const STAGE_COMPARING: &str = "comparing flat sessions";
pub const STAGE_EVIDENCE: &str = "reading lights shot during a change";

/// Whether flats of this rig are grouped by measurement, so measured.
pub fn rig_is_measured(rig: &str) -> bool {
    super::rig_limits(rig).measured
}

/// One flat session about to be measured.
#[derive(Debug, Clone)]
pub struct SessionPlan {
    pub session_key: String,
    pub family_key: String,
    pub rig_uuid: String,
    pub night: String,
    pub filter: Option<String>,
    pub rotation: Option<f64>,
    /// In capture order.
    pub frames: Vec<CalibrationFrame>,
}

impl SessionPlan {
    /// The frames the response combines: at most [`metric::MAX_FRAMES`],
    /// evenly spaced.
    pub fn combined(&self) -> Vec<&CalibrationFrame> {
        let n = self.frames.len();
        if n <= metric::MAX_FRAMES {
            return self.frames.iter().collect();
        }
        (0..metric::MAX_FRAMES)
            .map(|i| &self.frames[i * (n - 1) / (metric::MAX_FRAMES - 1)])
            .collect()
    }

    pub fn response_key(&self) -> String {
        let mut parts: Vec<String> = self
            .combined()
            .iter()
            .map(|frame| format!("{}:{}", frame.frame_uuid, frame.source_fingerprint))
            .collect();
        parts.sort();
        hex(&format!(
            "{}\u{1f}{}\u{1f}{}",
            metric::RESPONSE_ALGORITHM_VERSION,
            metric::MAX_SIDE,
            parts.join(",")
        ))
    }
}

fn hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The family a flat belongs to, rotation aside: rig, filter, focal length
/// class (a reducer swapped in at the same gain is another light path, not
/// new dust; see [`focal_classes`]), frame size, binning and sensor mode.
pub fn family_key(frame: &CalibrationFrame, focal: Option<f64>) -> String {
    hex(&format!(
        "{}\u{1f}{}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1f}{}",
        frame.rig_uuid,
        frame
            .filter
            .as_deref()
            .map(canonical_filter)
            .unwrap_or_default(),
        focal.map(|mm| (mm * 10.0).round() as i64),
        frame.width,
        frame.height,
        frame.binning_x,
        frame.binning_y,
        frame.gain,
        frame.offset,
        frame
            .readout_mode_name
            .as_deref()
            .map(|name| name.trim().to_ascii_lowercase())
            .unwrap_or_default(),
    ))
}

/// Each flat's focal length class, by frame uuid: per rig, recorded focal
/// lengths chained while each is within `tolerance(rig)` of the next (what
/// selection holds one master's flats to), named by the class's shortest.
/// A flat that records none joins the rig's only class when it has
/// exactly one, and is a class of its own otherwise.
pub fn focal_classes<'a>(
    flats: impl IntoIterator<Item = &'a CalibrationFrame>,
    tolerance: impl Fn(&str) -> f64,
) -> HashMap<String, Option<f64>> {
    let mut by_rig: BTreeMap<&str, Vec<&CalibrationFrame>> = BTreeMap::new();
    for frame in flats {
        by_rig
            .entry(frame.rig_uuid.as_str())
            .or_default()
            .push(frame);
    }
    let mut out = HashMap::new();
    for (rig, frames) in by_rig {
        let tolerance = tolerance(rig);
        let mut known: Vec<f64> = frames
            .iter()
            .filter_map(|frame| frame.focal_length_mm.filter(|mm| mm.is_finite()))
            .collect();
        known.sort_by(f64::total_cmp);
        known.dedup();
        // The shortest of each chain.
        let mut classes: Vec<f64> = Vec::new();
        for (i, mm) in known.iter().enumerate() {
            if i == 0 || mm - known[i - 1] > tolerance {
                classes.push(*mm);
            }
        }
        let class_of = |mm: f64| classes.iter().rev().find(|start| **start <= mm).copied();
        for frame in frames {
            let class = match frame.focal_length_mm.filter(|mm| mm.is_finite()) {
                Some(mm) => class_of(mm),
                None if classes.len() == 1 => Some(classes[0]),
                None => None,
            };
            out.insert(frame.frame_uuid.clone(), class);
        }
    }
    out
}

/// Flat sessions in use: one night of one family, frames whose rotator
/// angle lies within [`super::model::ROTATION_CLUSTER_DEG`] of the
/// session's first.
pub fn plan_sessions(frames: &[CalibrationFrame], boundary: i64) -> Vec<SessionPlan> {
    plan_sessions_with(frames, boundary, |rig| {
        crate::calibration::rules::rules_for_rig(rig)
            .coherence_tolerances()
            .focal_length_mm
    })
}

/// [`plan_sessions`], with each rig's focal length tolerance given.
pub fn plan_sessions_with(
    frames: &[CalibrationFrame],
    boundary: i64,
    focal_tolerance: impl Fn(&str) -> f64,
) -> Vec<SessionPlan> {
    let mut flats: Vec<&CalibrationFrame> = frames
        .iter()
        .filter(|frame| {
            frame.kind == CalibrationKind::Flat
                && !frame.is_master
                && frame.kept_out.is_none()
                && frame.captured_at.is_some()
        })
        .collect();
    flats.sort_by_key(|frame| (frame.captured_at, frame.id));
    let focal = focal_classes(flats.iter().copied(), focal_tolerance);
    let mut sessions: Vec<SessionPlan> = Vec::new();
    for frame in flats {
        let night = night_of(frame.captured_at.unwrap_or_default(), boundary);
        let family = family_key(frame, focal.get(&frame.frame_uuid).copied().flatten());
        let joins = |session: &SessionPlan| {
            session.family_key == family
                && session.night == night
                && match (session.rotation, frame.rotation) {
                    (None, None) => true,
                    (Some(a), Some(b)) => {
                        let d = (a - b).rem_euclid(360.0);
                        d.min(360.0 - d) <= super::model::ROTATION_CLUSTER_DEG
                    }
                    _ => false,
                }
        };
        match sessions.iter_mut().find(|session| joins(session)) {
            Some(session) => session.frames.push(frame.clone()),
            None => sessions.push(SessionPlan {
                session_key: hex(&format!("{family}\u{1f}{night}\u{1f}{}", frame.frame_uuid)),
                family_key: family,
                rig_uuid: frame.rig_uuid.clone(),
                night,
                filter: frame.filter.clone(),
                rotation: frame.rotation,
                frames: vec![frame.clone()],
            }),
        }
    }
    sessions
}

/// Bias and dark-flat levels by rig, gain and offset: each row's kind,
/// exposure and level in ADU.
pub type PedestalTable =
    HashMap<(String, Option<i64>, Option<i64>), Vec<(String, Option<f64>, f64)>>;

/// The pedestal under a session's flats: the matching dark-flats' level,
/// else the rig's bias level at that gain and offset, else nothing.
pub fn pedestals(conn: &Connection) -> Result<PedestalTable> {
    let mut out: PedestalTable = HashMap::new();
    let Ok(mut statement) = conn.prepare(
        "SELECT f.rig_uuid, f.gain, f.offset, f.kind, f.exposure_s,
                json_extract(q.measurement_json, '$.level')
         FROM psf_guard_calibration_frame f
         JOIN psf_guard_calibration_frame_quality q ON q.frame_uuid = f.frame_uuid
         WHERE f.kind IN ('bias', 'dark_flat') AND f.is_master = 0
           AND json_extract(q.measurement_json, '$.level') IS NOT NULL",
    ) else {
        return Ok(out);
    };
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, f64>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (rig, gain, offset, kind, exposure, level) in rows {
        out.entry((rig, gain, offset))
            .or_default()
            .push((kind, exposure, level));
    }
    Ok(out)
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(values[values.len() / 2])
}

/// A pedestal for frames of a rig at this gain and offset and exposure.
pub fn pedestal_for(
    table: &PedestalTable,
    rig: &str,
    gain: Option<i64>,
    offset: Option<i64>,
    exposure: Option<f64>,
) -> Option<f64> {
    let rows = table.get(&(rig.to_string(), gain, offset))?;
    let dark_flats: Vec<f64> = rows
        .iter()
        .filter(|(kind, e, _)| {
            kind == "dark_flat"
                && match (e, exposure) {
                    (Some(a), Some(b)) => (a - b).abs() <= 0.05 * a.max(b).max(0.1),
                    _ => false,
                }
        })
        .map(|(_, _, level)| *level)
        .collect();
    median(dark_flats).or_else(|| {
        median(
            rows.iter()
                .filter(|(kind, _, _)| kind == "bias")
                .map(|(_, _, level)| *level)
                .collect(),
        )
    })
}

/// Read and bin one frame for a response.
pub fn read_binned(path: &Path) -> Result<(Plane, usize)> {
    let frame = crate::image_io::open_linear_frame(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let image = &frame.image;
    let cfa = frame.bayer.is_some() && image.channels == 1;
    let factor = metric::bin_factor(image.width, image.height, cfa);
    Ok((
        metric::bin_frame(
            &image.data,
            image.width,
            image.height,
            image.channels,
            factor,
        ),
        factor,
    ))
}

fn write_plane(dir: &Path, key: &str, plane: &Plane) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = super::response_path(dir, key);
    let temporary = path.with_extension(format!("resp.tmp-{}", std::process::id()));
    std::fs::write(&temporary, metric::encode_plane(plane))?;
    std::fs::rename(&temporary, &path)?;
    Ok(())
}

/// Run one pass: the catalog is planned once, then each rig in the scope
/// goes through its stages in turn (responses, comparisons, light
/// evidence), the rigs `goes_first` names first.
pub fn run(pass: Pass<'_>) -> Result<PassOutcome> {
    let conn = pass.conn;
    let mut outcome = PassOutcome::default();
    let boundary = catalog_night_boundary(conn);
    let library = LibrarySnapshot::load(conn)?;
    let plans = plan_sessions(library.frames(), boundary);
    let pedestal_table = pedestals(conn)?;
    let known: HashMap<String, ResponseRow> = store::load_responses(conn)?
        .into_iter()
        .map(|row| (row.response_key.clone(), row))
        .collect();
    let measurable: Vec<&SessionPlan> = plans
        .iter()
        .filter(|plan| plan.frames.len() >= metric::MIN_FRAMES)
        .collect();
    // Rigs with flats, those the caller puts first ahead of the rest. A rig
    // grouped by time is left out: its selection must not lean on
    // measurements.
    let mut rigs: Vec<&str> = plans
        .iter()
        .map(|plan| plan.rig_uuid.as_str())
        .collect::<BTreeSet<&str>>()
        .into_iter()
        .filter(|rig| pass.scope.reads_lights(rig) && rig_is_measured(rig))
        .collect();
    rigs.sort_by_key(|rig| !(pass.goes_first)(rig));
    let frames_read = AtomicUsize::new(0);
    let mut measured_rigs: BTreeSet<String> = BTreeSet::new();
    let mut current: Vec<String> = Vec::new();
    let mut tried: std::collections::HashSet<(String, String)> = Default::default();
    'rigs: for rig in rigs {
        let report = |stage: &'static str, done: usize, total: usize| Progress {
            rig,
            stage,
            done,
            total,
        };
        let left_before = (outcome.unreadable, outcome.lights_missing);
        if pass.scope.measures(rig) {
            measured_rigs.insert(rig.to_string());
            let plans: Vec<&SessionPlan> = measurable
                .iter()
                .filter(|plan| plan.rig_uuid == rig)
                .copied()
                .collect();
            outcome.sessions += plans.len();
            // A session is done when its response is on disk from every
            // flat it combines, or could not be combined at all. One built
            // while some files were away is read again until they are back.
            let todo: Vec<(&SessionPlan, String)> = plans
                .iter()
                .map(|plan| (*plan, plan.response_key()))
                .filter(|(plan, key)| {
                    !known.get(key).is_some_and(|row| {
                        row.error.is_some()
                            || (super::response_path(pass.dir, key).is_file()
                                && row.frames >= plan.combined().len())
                    })
                })
                .collect();
            for plan in &plans {
                let key = plan.response_key();
                // A dark-flat or bias that arrived since: the pedestal comes
                // off anew, and the comparisons made without it are made
                // again.
                if let Some(row) = known.get(&key) {
                    let first = &plan.frames[0];
                    let pedestal = pedestal_for(
                        &pedestal_table,
                        &plan.rig_uuid,
                        first.gain,
                        first.offset,
                        first.exposure_s,
                    );
                    let moved = match (row.pedestal_adu, pedestal) {
                        (Some(a), Some(b)) => (a - b).abs() > 0.5,
                        (a, b) => a.is_some() != b.is_some(),
                    };
                    if moved {
                        store::set_pedestal(conn, &key, pedestal)?;
                    }
                    // Measured when the family key named less, as before it
                    // took the focal length: the plane stands, the family is
                    // renamed.
                    if row.family_key != plan.family_key || row.session_key != plan.session_key {
                        store::set_family(conn, &key, &plan.family_key, &plan.session_key)?;
                    }
                }
                current.push(key);
            }
            let total = todo.len();
            for (done, (plan, key)) in todo.iter().enumerate() {
                if !(pass.keep_going)() {
                    outcome.stopped = true;
                    break 'rigs;
                }
                (pass.progress)(&report(STAGE_MEASURING, done, total));
                measure_session(
                    &pass,
                    plan,
                    key,
                    &pedestal_table,
                    &frames_read,
                    &mut outcome,
                )?;
            }
            (pass.progress)(&report(STAGE_MEASURING, total, total));

            // Comparisons. A session whose flats record no angle joins a
            // rotation cluster only once compared with it, and then has that
            // family's pairs to make: a second round makes them.
            for _round in 0..COMPARE_ROUNDS {
                let book = Book::build(
                    store::load_responses(conn)?,
                    store::load_comparisons(conn)?,
                    Vec::new(),
                    Vec::new(),
                    super::rig_limits,
                );
                let pairs: Vec<(usize, usize)> = book
                    .pairs_to_compare()
                    .into_iter()
                    .filter(|(a, b)| book.comparison(*a, *b).is_none())
                    .filter(|(a, _)| book.sessions[*a].row.rig_uuid == rig)
                    .filter(|(a, b)| {
                        tried.insert((
                            book.sessions[*a].row.response_key.clone(),
                            book.sessions[*b].row.response_key.clone(),
                        ))
                    })
                    .collect();
                if pairs.is_empty() {
                    break;
                }
                let total = pairs.len();
                for (done, (a, b)) in pairs.iter().enumerate() {
                    if !(pass.keep_going)() {
                        outcome.stopped = true;
                        break 'rigs;
                    }
                    (pass.progress)(&report(STAGE_COMPARING, done, total));
                    (pass.yield_now)();
                    let (sa, sb) = (&book.sessions[*a].row, &book.sessions[*b].row);
                    let (Some(pa), Some(pb)) =
                        (session_plane(pass.dir, sa), session_plane(pass.dir, sb))
                    else {
                        continue;
                    };
                    let bin = sa.bin.unwrap_or(1);
                    if let Some(comparison) = pass.in_pool(|| metric::compare(&pa, &pb, bin)) {
                        store::insert_comparison(
                            conn,
                            &sa.response_key,
                            &sb.response_key,
                            &comparison,
                        )?;
                        outcome.compared += 1;
                    }
                }
                (pass.progress)(&report(STAGE_COMPARING, total, total));
            }
        }

        // Light evidence for this rig's changes.
        let book = super::load_book(conn)?;
        let needed: Vec<(usize, usize)> = book
            .sessions_needing_evidence()
            .into_iter()
            .filter(|(event, _)| book.events[*event].rig_uuid == rig)
            .collect();
        let total = needed.len();
        for (done, (event_index, session_index)) in needed.iter().enumerate() {
            if !(pass.keep_going)() {
                outcome.stopped = true;
                break 'rigs;
            }
            (pass.progress)(&report(STAGE_EVIDENCE, done, total));
            (pass.yield_now)();
            match session_evidence(
                &pass,
                &book,
                *event_index,
                *session_index,
                &pedestal_table,
                &frames_read,
            )? {
                Some(row) => {
                    let change = &book.events[*event_index].key;
                    store::upsert_evidence(conn, &row, change, EVIDENCE_ALGORITHM_VERSION)?;
                    outcome.evidence += 1;
                }
                None => outcome.lights_missing += 1,
            }
        }
        (pass.progress)(&report(STAGE_EVIDENCE, total, total));
        if (outcome.unreadable, outcome.lights_missing) != left_before {
            outcome.retry.insert(rig.to_string());
        }
    }
    outcome.frames_read = frames_read.load(Ordering::Relaxed);

    // Rows of the rigs measured that no current session has go, with their
    // planes; other rigs' rows wait for their own pass.
    store::retain_responses(conn, &measured_rigs, &current)?;
    remove_stale_planes(pass.dir, &store::response_keys(conn)?);
    // Evidence a newer key replaced, of an older fit, or of a session the
    // catalog lost goes; see `store::retain_evidence` for what stays.
    let book = super::load_book(conn)?;
    let wanted: Vec<store::EvidenceWanted> = book
        .sessions_in_windows()
        .into_iter()
        .map(|(event, session, key)| store::EvidenceWanted {
            session: book.imaging[session].key.clone(),
            change: book.events[event].key.clone(),
            key,
        })
        .collect();
    let sessions: Vec<String> = book
        .imaging
        .iter()
        .map(|session| session.key.clone())
        .collect();
    store::retain_evidence(conn, &wanted, &sessions, EVIDENCE_ALGORITHM_VERSION)?;
    Ok(outcome)
}

/// Read one flat session and keep its response.
fn measure_session(
    pass: &Pass<'_>,
    plan: &SessionPlan,
    key: &str,
    pedestal_table: &PedestalTable,
    frames_read: &AtomicUsize,
    outcome: &mut PassOutcome,
) -> Result<()> {
    let combined = plan.combined();
    let yield_now = pass.yield_now;
    let read: Vec<Result<(Plane, usize)>> = pass.in_pool(|| {
        combined
            .par_iter()
            .map(|frame| {
                yield_now();
                frames_read.fetch_add(1, Ordering::Relaxed);
                read_binned(&frame.source_path)
            })
            .collect()
    });
    let (planes, failures): (Vec<_>, Vec<_>) = read.into_iter().partition(|r| r.is_ok());
    let planes: Vec<(Plane, usize)> = planes.into_iter().map(|r| r.expect("ok")).collect();
    if !failures.is_empty() && planes.len() < metric::MIN_FRAMES {
        // Files away, as on an unmounted share: the caller tries again.
        outcome.unreadable += 1;
        return Ok(());
    }
    let bin = planes.first().map_or(1, |(_, bin)| *bin);
    let first = &plan.frames[0];
    let pedestal = pedestal_for(
        pedestal_table,
        &plan.rig_uuid,
        first.gain,
        first.offset,
        first.exposure_s,
    );
    let binned: Vec<Plane> = planes.into_iter().map(|(plane, _)| plane).collect();
    let response = pass.in_pool(|| metric::build_response(&binned, bin));
    let mut row = ResponseRow {
        response_key: key.to_string(),
        session_key: plan.session_key.clone(),
        rig_uuid: plan.rig_uuid.clone(),
        family_key: plan.family_key.clone(),
        night: plan.night.clone(),
        filter: plan.filter.clone(),
        rotation: plan.rotation,
        frame_uuids: plan.frames.iter().map(|f| f.frame_uuid.clone()).collect(),
        frames: binned.len(),
        captured_first: plan.frames.first().and_then(|f| f.captured_at),
        captured_last: plan.frames.last().and_then(|f| f.captured_at),
        level_adu: None,
        pedestal_adu: pedestal,
        noise: None,
        width: None,
        height: None,
        bin: Some(bin),
        algorithm_version: metric::RESPONSE_ALGORITHM_VERSION,
        error: None,
    };
    match response {
        Some(response) => {
            write_plane(pass.dir, key, &response.plane)?;
            row.level_adu = Some(f64::from(response.level));
            row.noise = Some(f64::from(response.noise));
            row.width = Some(response.plane.width);
            row.height = Some(response.plane.height);
            outcome.measured += 1;
        }
        None => row.error = Some("the flats could not be combined".into()),
    }
    store::upsert_response(pass.conn, &row)?;
    Ok(())
}

/// A session's response plane with its pedestal taken out.
pub fn session_plane(dir: &Path, row: &ResponseRow) -> Option<Plane> {
    let plane = super::read_plane(dir, &row.response_key)?;
    let response = metric::FlatResponse {
        plane,
        bin: row.bin.unwrap_or(1),
        level: row.level_adu.unwrap_or(0.0) as f32,
        noise: row.noise.unwrap_or(0.0) as f32,
        frames: row.frames,
    };
    Some(response.without_pedestal(row.pedestal_adu.unwrap_or(0.0) as f32))
}

/// Remove the planes of responses the catalog no longer holds.
fn remove_stale_planes(dir: &Path, current: &[String]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(key) = name.strip_suffix(".resp") else {
            continue;
        };
        if !current.iter().any(|k| k == key) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The change map of an event, from the member [`Book::map_source`]
/// chooses for `filter`. With the before and after response keys it was
/// measured from.
pub fn event_map(
    dir: &Path,
    book: &Book,
    event_index: usize,
    filter: Option<&str>,
) -> Option<(Plane, String, String)> {
    let boundary = &book.boundaries[book.map_member(event_index, filter)?];
    let before = &book.sessions[boundary.before].row;
    let after = &book.sessions[boundary.after].row;
    let map = metric::change_map(&session_plane(dir, before)?, &session_plane(dir, after)?)?;
    Some((map, before.response_key.clone(), after.response_key.clone()))
}

fn session_evidence(
    pass: &Pass<'_>,
    book: &Book,
    event_index: usize,
    session_index: usize,
    pedestals: &PedestalTable,
    frames_read: &AtomicUsize,
) -> Result<Option<EvidenceRow>> {
    let event = &book.events[event_index];
    let session = &book.imaging[session_index];
    // The session's lights by filter, with their files and targets.
    let mut by_filter: BTreeMap<String, Vec<(PathBuf, String)>> = BTreeMap::new();
    {
        let mut statement = pass.conn.prepare(
            "SELECT ai.filtername, ai.metadata, t.name FROM acquiredimage ai
             LEFT JOIN target t ON t.Id = ai.targetId
             WHERE ai.Id = ?1 AND ai.gradingStatus IS NOT 2",
        )?;
        for id in &session.lights {
            let row: Option<(Option<String>, String, Option<String>)> = statement
                .query_row([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .ok();
            let Some((filter, metadata, target)) = row else {
                continue;
            };
            let Some(name) = crate::utils::extract_filename(&metadata) else {
                continue;
            };
            let Some(path) = (pass.find_light)(crate::utils::file_name_of(&name)) else {
                continue;
            };
            by_filter
                .entry(filter.unwrap_or_default())
                .or_default()
                .push((path, target.unwrap_or_default()));
        }
    }
    if by_filter.is_empty() {
        tracing::debug!(
            "light evidence: none of {} lights found",
            session.lights.len()
        );
        return Ok(None);
    }
    let mut fits = Vec::new();
    for (filter, lights) in by_filter {
        if !event.applies_to(Some(&filter)) || lights.len() < evidence::MIN_LIGHTS {
            tracing::debug!(
                "light evidence {filter}: {} lights, or not this change's filter",
                lights.len()
            );
            continue;
        }
        let picked: Vec<&(PathBuf, String)> = if lights.len() > evidence::MAX_LIGHTS {
            (0..evidence::MAX_LIGHTS)
                .map(|i| &lights[i * (lights.len() - 1) / (evidence::MAX_LIGHTS - 1)])
                .collect()
        } else {
            lights.iter().collect()
        };
        // Only this rig's lights.
        let metas: Vec<_> = picked
            .iter()
            .map(|(path, _)| crate::commands::import::headers::read_frame_meta(path))
            .collect();
        if metas
            .iter()
            .any(|meta| crate::calibration::rules::light_rig_uuid(meta) != event.rig_uuid)
        {
            tracing::debug!("light evidence {filter}: lights from another rig");
            continue;
        }
        // A reference flat response of this filter with a known side: the
        // nearest in time, any angle.
        let reference = book
            .evidence_reference(event, &filter, session.start)
            .map(|index| &book.sessions[index]);
        let Some(reference) = reference else {
            tracing::debug!("light evidence {filter}: no flat session on either side");
            continue;
        };
        let reference_side = event.side_of(reference.first).expect("filtered");
        let Some(reference_plane) = session_plane(pass.dir, &reference.row) else {
            tracing::debug!("light evidence {filter}: reference plane missing");
            continue;
        };
        let Some((map, _, _)) = event_map(pass.dir, book, event_index, Some(&filter)) else {
            tracing::debug!("light evidence {filter}: no change map");
            continue;
        };
        let yield_now = pass.yield_now;
        let read: Vec<Result<(Plane, usize)>> = pass.in_pool(|| {
            picked
                .par_iter()
                .map(|(path, _)| {
                    yield_now();
                    frames_read.fetch_add(1, Ordering::Relaxed);
                    read_binned(path)
                })
                .collect()
        });
        let binned: Vec<Plane> = read
            .into_iter()
            .filter_map(Result::ok)
            .map(|(plane, _)| plane)
            .filter(|plane| plane.width == map.width && plane.height == map.height)
            .collect();
        let meta = &metas[0];
        let pedestal = pedestal_for(pedestals, &event.rig_uuid, meta.gain, meta.offset, None)
            .unwrap_or(0.0) as f32;
        let Some(background) =
            evidence::background(&binned, pedestal, reference.row.bin.unwrap_or(1))
        else {
            tracing::debug!(
                "light evidence {filter}: {} of {} lights read at the map's size",
                binned.len(),
                picked.len()
            );
            continue;
        };
        let Some(fit) = evidence::fit(&background, &reference_plane, reference_side, &map) else {
            tracing::debug!("light evidence {filter}: the fit found nothing to weigh");
            continue;
        };
        tracing::debug!(
            "light evidence {filter}: carried {:.2}, spread {:.2}, {:?}",
            fit.carried,
            fit.spread,
            fit.verdict
        );
        let mut targets: Vec<String> = picked.iter().map(|(_, target)| target.clone()).collect();
        targets.sort();
        targets.dedup();
        fits.push(EvidenceFit {
            filter,
            targets,
            lights: binned.len(),
            carried: fit.carried,
            spread: fit.spread.is_finite().then_some(fit.spread),
            verdict: fit.verdict,
        });
    }
    // Lights found but nothing to fit, as when no map spans the change
    // alone: kept as undecided, so the pass does not read them again until
    // a map or reference changes the key.
    let verdicts: Vec<Verdict> = fits.iter().map(|fit| fit.verdict).collect();
    Ok(Some(EvidenceRow {
        session_key: session.key.clone(),
        event_key: book.evidence_key(event, session),
        verdict: evidence::combine(&verdicts),
        fits,
    }))
}

/// Which side of an event the flats of a session lie on, if any.
pub fn session_side(book: &Book, event_index: usize, session_index: usize) -> Option<Side> {
    let event = &book.events[event_index];
    let session = &book.sessions[session_index];
    match (event.side_of(session.first), event.side_of(session.last)) {
        (Some(a), Some(b)) if a == b => Some(a),
        _ => None,
    }
}
