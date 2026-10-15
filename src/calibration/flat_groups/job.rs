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
//! this inside a pool sized from the background budget.

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
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// What one pass did.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct PassOutcome {
    pub sessions: usize,
    pub measured: usize,
    pub unreadable: usize,
    pub compared: usize,
    pub evidence: usize,
}

/// What the pass needs from its caller.
pub struct Pass<'a> {
    pub conn: &'a Connection,
    /// Where response planes are kept.
    pub dir: &'a Path,
    /// Asked before each piece of work; false ends the pass.
    pub keep_going: &'a (dyn Fn() -> bool + Sync),
    /// Called before each read: waits while interactive work runs.
    pub yield_now: &'a (dyn Fn() + Sync),
    /// `(stage, done, total)`.
    pub progress: &'a mut dyn FnMut(&str, usize, usize),
    /// A light's file from its name, when the image folders hold it.
    pub find_light: &'a dyn Fn(&str) -> Option<PathBuf>,
    /// The pool reads and measurements run on; the calling thread's when
    /// absent.
    pub pool: Option<&'a rayon::ThreadPool>,
}

impl Pass<'_> {
    fn in_pool<R: Send>(&self, work: impl FnOnce() -> R + Send) -> R {
        match self.pool {
            Some(pool) => pool.install(work),
            None => work(),
        }
    }
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

/// The family a flat belongs to, rotation aside: rig, filter, frame size,
/// binning and sensor mode.
pub fn family_key(frame: &CalibrationFrame) -> String {
    hex(&format!(
        "{}\u{1f}{}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1f}{}",
        frame.rig_uuid,
        frame
            .filter
            .as_deref()
            .map(canonical_filter)
            .unwrap_or_default(),
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

/// Flat sessions in use: one night of one family, frames whose rotator
/// angle lies within [`super::model::ROTATION_CLUSTER_DEG`] of the
/// session's first.
pub fn plan_sessions(frames: &[CalibrationFrame], boundary: i64) -> Vec<SessionPlan> {
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
    let mut sessions: Vec<SessionPlan> = Vec::new();
    for frame in flats {
        let night = night_of(frame.captured_at.unwrap_or_default(), boundary);
        let family = family_key(frame);
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

/// Run one pass.
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
    outcome.sessions = measurable.len();
    let mut current: Vec<String> = Vec::new();
    let todo: Vec<(&SessionPlan, String)> = measurable
        .iter()
        .map(|plan| (*plan, plan.response_key()))
        .filter(|(_, key)| {
            !known.get(key).is_some_and(|row| {
                row.error.is_some() || super::response_path(pass.dir, key).is_file()
            })
        })
        .collect();
    for plan in &measurable {
        current.push(plan.response_key());
    }
    let total = todo.len();
    for (done, (plan, key)) in todo.iter().enumerate() {
        if !(pass.keep_going)() {
            return Ok(outcome);
        }
        (pass.progress)("measuring flat sessions", done, total);
        let combined = plan.combined();
        let yield_now = pass.yield_now;
        let read: Vec<Result<(Plane, usize)>> = pass.in_pool(|| {
            combined
                .par_iter()
                .map(|frame| {
                    yield_now();
                    read_binned(&frame.source_path)
                })
                .collect()
        });
        let (planes, failures): (Vec<_>, Vec<_>) = read.into_iter().partition(|r| r.is_ok());
        let planes: Vec<(Plane, usize)> = planes.into_iter().map(|r| r.expect("ok")).collect();
        if !failures.is_empty() && planes.len() < metric::MIN_FRAMES {
            // Files away, as on an unmounted share: try again next pass.
            outcome.unreadable += 1;
            continue;
        }
        let bin = planes.first().map_or(1, |(_, bin)| *bin);
        let first = &plan.frames[0];
        let pedestal = pedestal_for(
            &pedestal_table,
            &plan.rig_uuid,
            first.gain,
            first.offset,
            first.exposure_s,
        );
        let binned: Vec<Plane> = planes.into_iter().map(|(plane, _)| plane).collect();
        let response = pass.in_pool(|| metric::build_response(&binned, bin));
        let mut row = ResponseRow {
            response_key: key.clone(),
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
        store::upsert_response(conn, &row)?;
    }
    (pass.progress)("measuring flat sessions", total, total);
    store::retain_responses(conn, &current)?;
    remove_stale_planes(pass.dir, &current);

    // Comparisons.
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
        .collect();
    let total = pairs.len();
    for (done, (a, b)) in pairs.iter().enumerate() {
        if !(pass.keep_going)() {
            return Ok(outcome);
        }
        (pass.progress)("comparing flat sessions", done, total);
        let (sa, sb) = (&book.sessions[*a].row, &book.sessions[*b].row);
        let (Some(pa), Some(pb)) = (session_plane(pass.dir, sa), session_plane(pass.dir, sb))
        else {
            continue;
        };
        let bin = sa.bin.unwrap_or(1);
        if let Some(comparison) = pass.in_pool(|| metric::compare(&pa, &pb, bin)) {
            store::insert_comparison(conn, &sa.response_key, &sb.response_key, &comparison)?;
            outcome.compared += 1;
        }
    }
    (pass.progress)("comparing flat sessions", total, total);

    // Light evidence.
    let book = super::load_book(conn)?;
    let needed = book.sessions_needing_evidence();
    let total = needed.len();
    for (done, (event_index, session_index)) in needed.iter().enumerate() {
        if !(pass.keep_going)() {
            return Ok(outcome);
        }
        (pass.progress)("reading lights shot during a change", done, total);
        if let Some(row) =
            session_evidence(&pass, &book, *event_index, *session_index, &pedestal_table)?
        {
            store::upsert_evidence(conn, &row, EVIDENCE_ALGORITHM_VERSION)?;
            outcome.evidence += 1;
        }
    }
    (pass.progress)("reading lights shot during a change", total, total);
    Ok(outcome)
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

/// The change map of an event, from its member boundary in `filter` when
/// it has one, else (for a camera-side change) its strongest member. With
/// the before and after response keys it was measured from.
pub fn event_map(
    dir: &Path,
    book: &Book,
    event_index: usize,
    filter: Option<&str>,
) -> Option<(Plane, String, String)> {
    let event = &book.events[event_index];
    let filter = filter.map(canonical_filter);
    let same_filter = event.boundaries.iter().copied().find(|b| {
        let family = &book.families[book.boundaries[*b].family];
        family.filter.as_deref().map(canonical_filter) == filter
    });
    let chosen = same_filter.or_else(|| {
        event.camera_side.then(|| {
            event
                .boundaries
                .iter()
                .copied()
                .max_by(|a, b| {
                    let d = |i: usize| book.boundaries[i].dust.unwrap_or(0.0);
                    d(*a).total_cmp(&d(*b))
                })
                .expect("events have boundaries")
        })
    })?;
    let boundary = &book.boundaries[chosen];
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
            .sessions
            .iter()
            .filter(|s| {
                s.row.rig_uuid == event.rig_uuid
                    && s.row.filter.as_deref().map(canonical_filter)
                        == Some(canonical_filter(&filter))
                    && event.side_of(s.first).is_some()
                    && event.side_of(s.last).is_some()
            })
            .min_by_key(|s| (s.first - session.start).abs());
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
    if fits.is_empty() {
        return Ok(None);
    }
    let verdicts: Vec<Verdict> = fits.iter().map(|fit| fit.verdict).collect();
    Ok(Some(EvidenceRow {
        session_key: session.key.clone(),
        event_key: event.key.clone(),
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
