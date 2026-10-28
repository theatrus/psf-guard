//! The measuring pass: responses, comparisons and light evidence.
//!
//! 1. Every flat session in use gets a response, from at most
//!    [`metric::MAX_FRAMES`] of its flats read once; a session whose frames
//!    and files are unchanged is not read again.
//! 2. Every pair the grouping needs ([`Book::pairs_to_compare`]) is compared
//!    from the cached planes.
//! 3. Every imaging session shot inside a change's window asks its own
//!    lights which side they are on ([`super::evidence`]).
//! 4. Every imaging session whose lights' angle points at two or more
//!    response families asks its own lights which one they follow
//!    ([`super::family_job`]).
//!
//! Each step commits as it goes, so a stop loses little. The caller runs
//! this on workers from the background budget, and says which rigs it
//! works on ([`Scope`]); a rig grouped by time is never measured.

use super::evidence::{self, EVIDENCE_ALGORITHM_VERSION};
use super::metric::{self, Plane};
use super::model::Book;
use super::store::{self, EvidenceRow, FileStamp, ResponseRow, UnreadFlat, UnreadKind};
use super::{family_job, family_store};
use crate::calibration::{CalibrationFrame, CalibrationKind, LibrarySnapshot, Nights};
use anyhow::{Context, Result};
use rayon::prelude::*;
use rusqlite::Connection;
pub use seiza_calibration::library::{
    family_key, focal_classes, pedestal_moved, FlatSessionPlan, PedestalLevel, PedestalTable,
    ResponseRead,
};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
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
    /// Sessions left with flats whose files are away: the caller tries
    /// them again later, and a pass reads them once the files are back.
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
    /// Imaging sessions and filters whose lights chose their family.
    pub families: usize,
}

/// Where a pass stands, as it reports it.
#[derive(Debug, Clone, Copy)]
pub struct Progress<'a> {
    /// The rig being worked on.
    pub rig: &'a str,
    /// [`STAGE_MEASURING`], [`STAGE_COMPARING`], [`STAGE_EVIDENCE`] or
    /// [`STAGE_FAMILIES`].
    pub stage: &'a str,
    pub done: usize,
    pub total: usize,
    /// Rigs this pass checks after this one. Whether each has work is
    /// known only once the pass reaches it.
    pub rigs_after: usize,
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
    pub(crate) fn in_pool<R: Send>(&self, work: impl FnOnce() -> R + Send) -> R {
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
pub const STAGE_FAMILIES: &str = "reading lights to choose their flats";

/// Whether flats of this rig are grouped by measurement, so measured.
pub fn rig_is_measured(rig: &str) -> bool {
    super::rig_limits(rig).measured
}

/// Flat sessions in use: one night of one family, frames whose rotator
/// angle lies within [`super::model::ROTATION_CLUSTER_DEG`] of the
/// session's first.
pub fn plan_flat_sessions(frames: &[CalibrationFrame], nights: &Nights) -> Vec<FlatSessionPlan> {
    seiza_calibration::library::plan_flat_sessions(
        frames,
        nights,
        &crate::calibration::rules::rule_book(),
    )
}

/// The pedestal under a session's flats: the matching dark-flats' level,
/// else the rig's bias level at that gain and offset, else nothing.
pub fn pedestals(conn: &Connection) -> Result<PedestalTable> {
    let mut out = PedestalTable::default();
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
        if let Ok(kind) = kind.parse::<CalibrationKind>() {
            out.insert(PedestalLevel {
                rig_uuid: &rig,
                gain,
                offset,
                kind,
                exposure_s: exposure,
                level,
            });
        }
    }
    Ok(out)
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

/// A session's dust, from its cached response plane (the stack of its
/// flats, pedestal out) and each flat's own stored features: no frame is
/// read. Until every flat of the session has a current measurement, the
/// stack's dust stands alone, without persistence, and is made again once
/// they all have one.
///
/// In how many flats each feature shows, which features are kept, and each
/// flat's transients come from `seiza_calibration::dust::session_dust_from_lists`,
/// so this and `find_session_dust` share one retention rule. With fewer
/// than two measured flats persistence cannot be told, and the stack's dust
/// stands alone for good.
fn session_dust(
    dir: &Path,
    row: &ResponseRow,
    lists: Option<crate::calibration::quality::FlatFeatureLists>,
) -> Option<super::SessionDust> {
    use seiza_calibration::dust::{self, DustFeature, DustMap, DustOptions};
    let plane = session_plane(dir, row)?;
    let bin = row.bin.unwrap_or(1);
    let options = DustOptions::default();
    if let Some(lists) = &lists {
        let features: Vec<&[DustFeature]> = lists.iter().map(|(_, list)| list.as_slice()).collect();
        if let Some((mut map, views)) =
            dust::session_dust_from_lists(&plane, bin, &features, &options)
        {
            // The flats stacked, not only those measured.
            map.frames = row.frames;
            let frames = lists
                .iter()
                .zip(views)
                .map(|((frame_uuid, _), view)| super::FrameDustRow {
                    frame_uuid: frame_uuid.clone(),
                    seen: view.seen,
                    transients: view.transients,
                })
                .collect();
            return Some(super::SessionDust {
                map,
                frames,
                persistence: true,
            });
        }
    }
    let mut map = dust::find_dust(&plane, bin, &options).unwrap_or(DustMap {
        algorithm_version: dust::DUST_ALGORITHM_VERSION,
        bin,
        features: Vec::new(),
        vignetting: Vec::new(),
        sigma: 0.0,
        star_fraction: 0.0,
        frames: 0,
    });
    map.frames = row.frames;
    Some(super::SessionDust {
        map,
        frames: Vec::new(),
        persistence: lists.is_some(),
    })
}

fn write_dust(dir: &Path, key: &str, dust: &super::SessionDust) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = super::dust_path(dir, key);
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec(dust)?)?;
    std::fs::rename(&temporary, &path)?;
    Ok(())
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
    let library = LibrarySnapshot::load(conn)?;
    let plans = plan_flat_sessions(library.frames(), &Nights::load(conn));
    let pedestal_table = pedestals(conn)?;
    let known: HashMap<String, ResponseRow> = store::load_responses(conn)?
        .into_iter()
        .map(|row| (row.response_key.clone(), row))
        .collect();
    let measurable: Vec<&FlatSessionPlan> = plans
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
    // The lights' rotator offsets, which the family stage's candidates are
    // turned by: from the lights' metadata and the solves in the catalog's
    // cache folder that changed since the last pass.
    if !rigs.is_empty() && (pass.keep_going)() {
        super::gather_angles(conn, pass.dir.parent())?;
    }
    let frames_read = AtomicUsize::new(0);
    let mut measured_rigs: BTreeSet<String> = BTreeSet::new();
    let mut current: Vec<String> = Vec::new();
    let mut tried: std::collections::HashSet<(String, String)> = Default::default();
    let mut family_keep: Vec<family_store::Kept> = Vec::new();
    let mut family_rigs: BTreeSet<String> = BTreeSet::new();
    let rig_count = rigs.len();
    'rigs: for (index, rig) in rigs.into_iter().enumerate() {
        let report = |stage: &'static str, done: usize, total: usize| Progress {
            rig,
            stage,
            done,
            total,
            rigs_after: rig_count - index - 1,
        };
        let left_before = (outcome.unreadable, outcome.lights_missing);
        if pass.scope.measures(rig) {
            measured_rigs.insert(rig.to_string());
            let plans: Vec<&FlatSessionPlan> = measurable
                .iter()
                .filter(|plan| plan.rig_uuid == rig)
                .copied()
                .collect();
            outcome.sessions += plans.len();
            // A session is done once its response is kept (see
            // `response_is_current`). One left with flats away or refused
            // is read again once they may read, and keeps its rig on the
            // retry ladder meanwhile; it is not read on every pass.
            let mut todo: Vec<(&FlatSessionPlan, String)> = Vec::new();
            for plan in &plans {
                let key = plan.response_key();
                match known.get(&key) {
                    Some(row) if response_is_current(pass.dir, plan, row) => {
                        if row.unread.iter().any(|flat| flat.kind.retries()) {
                            outcome.unreadable += 1;
                        }
                    }
                    _ => todo.push((*plan, key)),
                }
            }
            for plan in &plans {
                let key = plan.response_key();
                // A dark-flat or bias that arrived since: the pedestal comes
                // off anew, and the comparisons made without it are made
                // again. The session's dust was measured with the pedestal
                // out, so it is measured again too.
                if let Some(row) = known.get(&key) {
                    let pedestal = pedestal_table.for_session(plan);
                    if seiza_calibration::library::pedestal_moved(row.pedestal_adu, pedestal) {
                        store::set_pedestal(conn, &key, pedestal)?;
                        let _ = std::fs::remove_file(super::dust_path(pass.dir, &key));
                    }
                    // Measured when the family key named less, as before it
                    // took the focal length: the plane stands, the family is
                    // renamed.
                    if row.family_key != plan.family_key || row.session_key != plan.session_key {
                        store::set_family(conn, &key, &plan.family_key, &plan.session_key)?;
                    }
                    // Measured before the rig's nights split at its own
                    // noon: the same flats under another night's name.
                    if row.night != plan.night {
                        store::set_night(conn, &key, &plan.night)?;
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

            // Each session's dust, from its response plane and its flats'
            // stored features: made where missing, or where it waited for
            // flats that are now measured.
            for row in store::load_responses(conn)?
                .iter()
                .filter(|row| row.rig_uuid == rig && row.error.is_none())
                .filter(|row| current.contains(&row.response_key))
            {
                let key = &row.response_key;
                let stored = super::read_dust(pass.dir, key);
                if stored.as_ref().is_some_and(|dust| dust.persistence) {
                    continue;
                }
                // A flat the response could not read is no witness either.
                let frames: Vec<String> = row
                    .frame_uuids
                    .iter()
                    .filter(|uuid| !row.unread.iter().any(|flat| &flat.frame_uuid == *uuid))
                    .cloned()
                    .collect();
                let lists = crate::calibration::quality::flat_feature_lists(conn, &frames)?;
                if stored.is_some() && lists.is_none() {
                    continue;
                }
                if let Some(dust) = pass.in_pool(|| session_dust(pass.dir, row, lists)) {
                    write_dust(pass.dir, key, &dust)?;
                }
            }

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

        // Which response family this rig's lights follow, where their angle
        // points at two or more ([`super::family_job`]).
        family_store::add_table(conn)?;
        let library = LibrarySnapshot::load(conn)?;
        let book = std::sync::Arc::clone(library.flat_book());
        let questions = family_job::questions(&pass, &library, &book, rig)?;
        let todo: Vec<&family_job::Question> = questions
            .iter()
            .filter(|question| !family_job::answered(question))
            .collect();
        let total = todo.len();
        for (done, question) in todo.iter().enumerate() {
            if !(pass.keep_going)() {
                outcome.stopped = true;
                break 'rigs;
            }
            (pass.progress)(&report(STAGE_FAMILIES, done, total));
            (pass.yield_now)();
            match family_job::answer(&pass, &book, question, &pedestal_table, &frames_read) {
                Some(row) => {
                    family_store::upsert(conn, &row)?;
                    outcome.families += 1;
                }
                None => outcome.lights_missing += 1,
            }
        }
        (pass.progress)(&report(STAGE_FAMILIES, total, total));
        family_keep.extend(
            questions
                .iter()
                .map(|question| family_job::kept(&book, question)),
        );
        family_rigs.insert(rig.to_string());
        if (outcome.unreadable, outcome.lights_missing) != left_before {
            outcome.retry.insert(rig.to_string());
        }
    }
    outcome.frames_read = frames_read.load(Ordering::Relaxed);

    // A rig in scope with measured rows but no flats in use any more (every
    // one kept out by the use/exclude choice, or forgotten) has no plan, so
    // the loop never named it: its rows all go. A rig grouped by time keeps
    // them for when it is measured again.
    let planned: BTreeSet<&str> = plans.iter().map(|plan| plan.rig_uuid.as_str()).collect();
    for row in known.values() {
        let rig = row.rig_uuid.as_str();
        if !planned.contains(rig) && pass.scope.measures(rig) && rig_is_measured(rig) {
            measured_rigs.insert(rig.to_string());
        }
    }
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
    // Family evidence of the rigs whose lights were asked: rows no
    // question asks for go, and rows of sessions the catalog lost.
    let judged: std::collections::HashSet<String> = book
        .imaging
        .iter()
        .filter(|session| session.rigs.iter().any(|rig| family_rigs.contains(rig)))
        .map(|session| session.key.clone())
        .collect();
    let all: std::collections::HashSet<String> = sessions.into_iter().collect();
    family_store::retain(conn, &family_keep, &judged, &all)?;
    Ok(outcome)
}

/// Whether a session's kept response stands, so the pass reads nothing for
/// it: its plane is on disk and it combines every flat it could read, or it
/// could not be combined at all. The flats it could not read count as done
/// until one may read now ([`may_read_now`]). An older row that names no
/// unread flats but combines fewer than its session holds is read once more
/// to name them.
fn response_is_current(dir: &Path, plan: &FlatSessionPlan, row: &ResponseRow) -> bool {
    let combined = plan.combined();
    let back = row.unread.iter().any(|flat| {
        combined
            .iter()
            .any(|frame| frame.frame_uuid == flat.frame_uuid && may_read_now(flat, frame))
    });
    if back {
        return false;
    }
    row.error.is_some()
        || (super::response_path(dir, &row.response_key).is_file()
            && row.frames + row.unread.len() >= combined.len())
}

/// The size and modification time of the file at `path` now; none when no
/// file is there.
fn file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok().filter(|meta| meta.is_file())?;
    let modified_ns = meta
        .modified()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos() as u64);
    Some(FileStamp {
        size: meta.len(),
        modified_ns,
    })
}

/// Whether a flat a session could not read is worth reading again: a
/// missing file is there now, a refused one opens, or the file at its path
/// is another than the one that failed (its size or time differ). Only
/// stats and opens; reads no pixel.
fn may_read_now(flat: &UnreadFlat, frame: &CalibrationFrame) -> bool {
    let path = &frame.source_path;
    let replaced = || file_stamp(path).is_some_and(|now| flat.stamp != Some(now));
    match flat.kind {
        UnreadKind::Missing => path.is_file(),
        UnreadKind::Transient => replaced() || std::fs::File::open(path).is_ok(),
        UnreadKind::Undecodable => replaced(),
    }
}

/// Why a flat would not read, from the I/O error under it: no file, a file
/// the file system would not give up, or one that is not a readable image
/// (bad data, cut short).
fn unread_kind(error: &anyhow::Error, path: &Path) -> UnreadKind {
    use std::io::ErrorKind;
    let io = error.chain().find_map(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind)
            // A FITS error keeps its I/O error without naming it a source.
            .or_else(|| match cause.downcast_ref::<seiza_fits::FitsError>() {
                Some(seiza_fits::FitsError::Io(error)) => Some(error.kind()),
                _ => None,
            })
    });
    match io {
        Some(ErrorKind::NotFound) => UnreadKind::Missing,
        Some(ErrorKind::InvalidData | ErrorKind::UnexpectedEof) => UnreadKind::Undecodable,
        Some(_) => UnreadKind::Transient,
        None if !path.is_file() => UnreadKind::Missing,
        None => UnreadKind::Undecodable,
    }
}

/// Read one flat session and keep its response, with the flats it could
/// not read and why.
fn measure_session(
    pass: &Pass<'_>,
    plan: &FlatSessionPlan,
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
    let mut planes: Vec<(Plane, usize)> = Vec::new();
    let mut unread: Vec<UnreadFlat> = Vec::new();
    for (frame, result) in combined.iter().zip(read) {
        match result {
            Ok(plane) => planes.push(plane),
            Err(error) => unread.push(UnreadFlat {
                frame_uuid: frame.frame_uuid.clone(),
                kind: unread_kind(&error, &frame.source_path),
                reason: format!("{error:#}"),
                stamp: file_stamp(&frame.source_path),
            }),
        }
    }
    // Files away or refused, as on an unmounted share, a copy moved
    // elsewhere or a permission: the caller tries again, and a pass reads
    // the session once they may read.
    if unread.iter().any(|flat| flat.kind.retries()) {
        outcome.unreadable += 1;
    }
    let bin = planes.first().map_or(1, |(_, bin)| *bin);
    let pedestal = pedestal_table.for_session(plan);
    let binned: Vec<Plane> = planes.into_iter().map(|(plane, _)| plane).collect();
    let response = pass.in_pool(|| metric::build_response(&binned, bin));
    let row = plan.measured_row(
        pedestal,
        ResponseRead {
            read: binned.len(),
            bin,
            response: response.as_ref(),
            unread,
        },
    );
    if let Some(response) = &response {
        write_plane(pass.dir, key, &response.plane)?;
        // A new plane, as when flats that were away came back: its dust
        // is found again from it.
        let _ = std::fs::remove_file(super::dust_path(pass.dir, key));
        outcome.measured += 1;
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

/// Remove the planes and dust of responses the catalog no longer holds,
/// and dust kept under an older dust algorithm.
fn remove_stale_planes(dir: &Path, current: &[String]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // A plane, or a session's dust beside it.
        let Some(key) = name
            .strip_suffix(".resp")
            .or_else(|| name.split_once(".dust-v").map(|(key, _)| key))
        else {
            continue;
        };
        // A session's dust of an older algorithm is measured again under
        // the current one's name.
        let outdated = name.contains(".dust-v")
            && super::dust_path(dir, key).file_name() != Some(entry.file_name().as_os_str());
        if outdated || !current.iter().any(|k| k == key) {
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
    let map = evidence::change_map(
        &session_plane(dir, before)?,
        &session_plane(dir, after)?,
        &boundary.vignetting_changes,
        before.bin.unwrap_or(1),
    )?;
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
    use seiza_calibration::library::EvidenceLight;
    let event = &book.events[event_index];
    let session = &book.imaging[session_index];
    // The session's lights, with their files and targets.
    let mut lights = Vec::new();
    {
        let mut statement = pass.conn.prepare(
            "SELECT ai.filtername, ai.metadata, t.name, ai.acquireddate FROM acquiredimage ai
             LEFT JOIN target t ON t.Id = ai.targetId
             WHERE ai.Id = ?1 AND ai.gradingStatus IS NOT 2",
        )?;
        for id in &session.lights {
            let row: Option<(Option<String>, String, Option<String>, i64)> = statement
                .query_row([id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .ok();
            let Some((filter, metadata, target, at)) = row else {
                continue;
            };
            let Some(name) = crate::utils::extract_filename(&metadata) else {
                continue;
            };
            let Some(path) = (pass.find_light)(crate::utils::file_name_of(&name)) else {
                continue;
            };
            lights.push(EvidenceLight {
                path,
                at,
                angle: None,
                filter: filter.unwrap_or_default(),
                target: target.unwrap_or_default(),
            });
        }
    }
    let Some(question) =
        seiza_calibration::library::change_question(book, event_index, session_index, lights)
    else {
        tracing::debug!(
            "light evidence: none of {} lights found",
            session.lights.len()
        );
        return Ok(None);
    };
    let mut fits = Vec::new();
    for filter in &question.filters {
        let name = &filter.filter;
        // Only this rig's lights.
        let metas: Vec<_> = filter
            .lights
            .iter()
            .map(|light| crate::commands::import::headers::read_frame_meta(&light.path))
            .collect();
        if metas
            .iter()
            .any(|meta| crate::calibration::rules::light_rig_uuid(meta) != event.rig_uuid)
        {
            tracing::debug!("light evidence {name}: lights from another rig");
            continue;
        }
        let Some(reference_plane) = session_plane(pass.dir, &book.sessions[filter.reference].row)
        else {
            tracing::debug!("light evidence {name}: reference plane missing");
            continue;
        };
        let (before, after) = filter.map_responses(book);
        let Some(map) = session_plane(pass.dir, before)
            .zip(session_plane(pass.dir, after))
            .and_then(|(before, after)| filter.change_map(book, &before, &after))
        else {
            tracing::debug!("light evidence {name}: no change map");
            continue;
        };
        let yield_now = pass.yield_now;
        let read: Vec<Result<(Plane, usize)>> = pass.in_pool(|| {
            filter
                .lights
                .par_iter()
                .map(|light| {
                    yield_now();
                    frames_read.fetch_add(1, Ordering::Relaxed);
                    read_binned(&light.path)
                })
                .collect()
        });
        let binned: Vec<Plane> = read
            .into_iter()
            .filter_map(Result::ok)
            .map(|(plane, _)| plane)
            .collect();
        let pedestal = pedestals.light_pedestal(&event.rig_uuid, metas[0].gain, metas[0].offset);
        match filter.fit(book, binned, &reference_plane, &map, pedestal) {
            Some(fit) => {
                tracing::debug!(
                    "light evidence {name}: carried {:.2}, spread {:?}, {:?}",
                    fit.carried,
                    fit.spread,
                    fit.verdict
                );
                fits.push(fit);
            }
            None => tracing::debug!("light evidence {name}: the fit found nothing to weigh"),
        }
    }
    // Lights found but nothing to fit, as when no map spans the change
    // alone: kept as undecided, so the pass does not read them again until
    // a map or reference changes the key.
    Ok(Some(question.row(book, fits)))
}

#[cfg(test)]
mod stale_dust_tests {
    use super::*;

    /// Once DUST_ALGORITHM_VERSION moves, a session's dust of the older
    /// version goes rather than staying beside the current one.
    #[test]
    fn an_older_dust_version_is_removed() {
        let temp = tempfile::tempdir().unwrap();
        let key = "abc123";
        std::fs::write(temp.path().join(format!("{key}.resp")), b"plane").unwrap();
        std::fs::write(super::super::dust_path(temp.path(), key), b"{}").unwrap();
        let older = temp.path().join(format!("{key}.dust-v0.json"));
        std::fs::write(&older, b"{}").unwrap();
        remove_stale_planes(temp.path(), &[key.to_string()]);
        assert!(super::super::dust_path(temp.path(), key).is_file());
        assert!(!older.is_file(), "a dust file of an older version stays");
    }
}

#[cfg(test)]
mod session_dust_tests {
    use super::*;
    use seiza_calibration::dust::{DustFeature, SeenIn};

    const BIN: usize = 7;

    /// A session of `frames` flats whose stack holds one wide disk, 0.3%
    /// deep, on a little noise; its plane is written where the pass keeps
    /// it.
    fn session(dir: &Path, frames: usize) -> ResponseRow {
        let (width, height) = (512, 384);
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let data = (0..width * height)
            .map(|index| {
                let (x, y) = ((index % width) as f32, (index / width) as f32);
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let noise = ((seed >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.001;
                let disk = if (x - 200.0).hypot(y - 150.0) <= 40.0 {
                    0.997
                } else {
                    1.0
                };
                disk + noise
            })
            .collect();
        let row = ResponseRow {
            response_key: "session".to_string(),
            session_key: "session".to_string(),
            rig_uuid: "rig".to_string(),
            family_key: "family".to_string(),
            night: "2026-10-01".to_string(),
            filter: None,
            rotation: None,
            frame_uuids: (0..frames).map(|i| format!("f{i}")).collect(),
            frames,
            captured_first: None,
            captured_last: None,
            level_adu: Some(30_000.0),
            pedestal_adu: None,
            noise: Some(0.0004),
            width: Some(width),
            height: Some(height),
            bin: Some(BIN),
            algorithm_version: metric::RESPONSE_ALGORITHM_VERSION,
            error: None,
            unread: Vec::new(),
        };
        let plane = Plane {
            width,
            height,
            data,
        };
        write_plane(dir, &row.response_key, &plane).unwrap();
        row
    }

    /// Each named flat holding `features`, as its stored measurement does.
    fn lists(
        names: &[&str],
        features: &[DustFeature],
    ) -> crate::calibration::quality::FlatFeatureLists {
        names
            .iter()
            .map(|name| (name.to_string(), features.to_vec()))
            .collect()
    }

    /// The flats' own features say in how many of them each feature of
    /// the stack shows, by Seiza's rule, and each flat keeps its view in
    /// the order given.
    #[test]
    fn the_flats_lists_give_persistence_and_each_flats_view() {
        let temp = tempfile::tempdir().unwrap();
        let row = session(temp.path(), 6);
        let alone = session_dust(temp.path(), &row, None).unwrap();
        assert!(!alone.persistence, "no flat is measured yet");
        assert!(alone.frames.is_empty());
        assert!(!alone.map.features.is_empty(), "the stack holds the disk");
        let disk = alone.map.features.clone();

        // A passing speck on the last flat alone, far from the disk.
        let mut speck = disk[0];
        speck.shape = seiza_calibration::dust::DustShape::Compact;
        speck.radius = 4.0;
        speck.x = 3000.0;
        speck.y = 2200.0;
        speck.left = 2996;
        speck.right = 3004;
        speck.top = 2196;
        speck.bottom = 2204;
        let mut flats = lists(&["a", "b", "c"], &disk);
        flats[2].1.push(speck);
        let dust = session_dust(temp.path(), &row, Some(flats)).unwrap();
        assert!(dust.persistence);
        assert_eq!(dust.map.frames, 6, "the flats stacked, not those listed");
        assert_eq!(
            dust.map.features.len(),
            disk.len(),
            "{:?}",
            dust.map.features
        );
        assert!(
            dust.map
                .features
                .iter()
                .all(|f| f.seen_in == Some(SeenIn { frames: 3, of: 3 })),
            "{:?}",
            dust.map.features
        );
        let uuids: Vec<&str> = dust.frames.iter().map(|f| f.frame_uuid.as_str()).collect();
        assert_eq!(uuids, ["a", "b", "c"]);
        let every: Vec<usize> = (0..disk.len()).collect();
        assert!(
            dust.frames.iter().all(|f| f.seen == every),
            "{:?}",
            dust.frames
        );
        assert!(dust.frames[..2].iter().all(|f| f.transients.is_empty()));
        assert_eq!(dust.frames[2].transients.len(), 1, "{:?}", dust.frames[2]);
    }

    /// One measured flat cannot tell persistence: the stack's dust stands
    /// alone, and is not made again on every pass.
    #[test]
    fn one_measured_flat_leaves_the_stack_alone_for_good() {
        let temp = tempfile::tempdir().unwrap();
        let row = session(temp.path(), 6);
        let alone = session_dust(temp.path(), &row, None).unwrap();
        let dust =
            session_dust(temp.path(), &row, Some(lists(&["a"], &alone.map.features))).unwrap();
        assert!(dust.persistence);
        assert!(dust.frames.is_empty());
        assert_eq!(dust.map.frames, 6);
        assert_eq!(dust.map.features, alone.map.features);
    }
}
