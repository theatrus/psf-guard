//! Flat groups, change events and where each light stands, worked out from
//! what the catalog keeps. Nothing here reads pixels.
//!
//! - A **family** is a **response family**: the measured sessions of one
//!   rig, filter, focal class and sensor mode that describe one light path.
//!   The recorded rotator angle is a prior, not an identity. Angles first
//!   form rotation clusters by single linkage: they split only where a gap
//!   between them is wider than the rig's rotation tolerance. Clusters
//!   whose sessions measure alike then join: two sessions shot within one
//!   flat window, or neighbours in time, alike within the limits, with no
//!   pair shot within one window apart. Clusters never measured alike stay
//!   apart. A session whose flats record no angle joins the family it
//!   measures alike with, else stays on its own.
//! - Within a family, sessions merge into **groups** in time order: the
//!   neighbouring pair with the smallest linkage first, while it stays
//!   within the limits. Linkage is the largest difference between any two
//!   sessions of the two groups that were compared, so a slow drift cannot
//!   chain nights that differ into one group.
//! - Each boundary between groups is a window of time, from the last flat
//!   before it to the first after, with the spots that changed.
//! - Boundaries of one rig whose windows overlap and whose spots coincide
//!   merge into one **change event**. Dust on the camera side turns with the
//!   camera, so the same shadow shows in every filter and in every response
//!   family; an event seen in two filters, or in two response families of
//!   one filter, applies to every light of the rig, one seen in a single
//!   family only to that filter.
//! - A light stands **before** or **after** an event by its time, by the
//!   continuous imaging session it belongs to, or by its own pixels (see
//!   [`super::evidence`]); otherwise it is undetermined.

use super::lights::{AngleEpochs, FamilyEvidence};
use super::metric::{Spot, MEDIAN_FRAMES};
use super::rows::{ComparisonRow, EvidenceRow, ResponseRow, Verdict};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};

/// Flats of one night whose rotator angles lie within this of the session's
/// first are one session: the coherence tolerance one master's flats are
/// held to. Also the gap that separates rotation clusters on a rig without
/// limits of its own.
pub const ROTATION_CLUSTER_DEG: f64 = 2.0;
/// One flat run: a twilight's flats, filter after filter, are shot within
/// this. A change whose window is shorter landed between the flats of one
/// run, or is two changes that met there.
pub const FLAT_RUN_SECONDS: i64 = 3600;
/// Lights of one profile closer than this in time are one continuous
/// imaging session. A night's pauses are shorter; a day is longer.
pub const IMAGING_GAP_SECONDS: i64 = 3 * 3600;
/// A comparison that takes a session of fewer than
/// [`MEDIAN_FRAMES`] flats sets this many of its own spreads against its
/// dust before the limit: two flats combine by a mean, not a median, and
/// the deepest spot of a noisier ratio stands that much higher for noise
/// alone.
pub const NOISY_DUST_SIGMAS: f64 = 2.0;

/// The limits one rig's sessions are held to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub measured: bool,
    /// Fractions of the background.
    pub dust: f64,
    pub broad: f64,
    /// The flat session window, in seconds: a master spans at most twice it.
    pub window: i64,
    /// The rotation tolerance one master's flats are held to, degrees: a
    /// gap in the rig's flat angles wider than this separates two rotation
    /// clusters. Never taken above [`ROTATION_CLUSTER_DEG`].
    pub rotation: f64,
}

impl Limits {
    /// How far two sessions are apart against these limits: alike at 1 or
    /// less.
    pub fn distance(&self, comparison: &ComparisonRow) -> f64 {
        (comparison.dust / self.dust.max(1e-9)).max(comparison.broad / self.broad.max(1e-9))
    }

    /// [`Self::distance`] for a pair of which one session combines fewer
    /// than [`MEDIAN_FRAMES`] flats: its dust less [`NOISY_DUST_SIGMAS`] of
    /// the comparison's own spread, the height noise alone gives the
    /// deepest spot of a noisier ratio.
    pub fn noisy_distance(&self, comparison: &ComparisonRow) -> f64 {
        let dust = (comparison.dust - NOISY_DUST_SIGMAS * comparison.sigma).max(0.0);
        (dust / self.dust.max(1e-9)).max(comparison.broad / self.broad.max(1e-9))
    }
}

/// The recorded angles of one rotation cluster of a family, lowest to
/// highest going round the circle from `low`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AngleSpan {
    pub low: f64,
    pub high: f64,
}

impl AngleSpan {
    /// "100.1–104.4°", or "82.2°" for one angle.
    pub fn label(&self) -> String {
        if (self.high - self.low).abs() < 0.05 {
            format!("{:.1}°", self.low)
        } else {
            format!("{:.1}–{:.1}°", self.low, self.high)
        }
    }

    /// How far `angle` lies outside the span, around the circle: 0 inside.
    pub fn gap_to(&self, angle: f64) -> f64 {
        let width = (self.high - self.low).rem_euclid(360.0);
        let into = (angle - self.low).rem_euclid(360.0);
        if into <= width {
            0.0
        } else {
            rotation_gap(angle, self.low).min(rotation_gap(angle, self.high))
        }
    }
}

/// One measured flat session.
#[derive(Debug, Clone, Serialize)]
pub struct Session {
    #[serde(flatten)]
    pub row: ResponseRow,
    pub first: i64,
    pub last: i64,
    /// For flats that record no angle: the angle of the rotation cluster
    /// they measure alike with, when they do.
    pub inferred_rotation: Option<f64>,
}

/// Why two neighbouring groups stay apart.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Boundary {
    /// Index into the book's families.
    pub family: usize,
    /// The last session before and the first after, as indices into the
    /// book's sessions.
    pub before: usize,
    pub after: usize,
    /// The last flat before and the first flat after, Unix seconds.
    pub start: i64,
    pub end: i64,
    /// The neighbouring pair's metrics, fractions of the background; absent
    /// when that pair was not compared.
    pub dust: Option<f64>,
    pub broad: Option<f64>,
    /// When the neighbours themselves are alike but the groups are not: the
    /// worst pair across the boundary, by session index, and its distance.
    pub drift: Option<(usize, usize, f64)>,
    pub spots: Vec<Spot>,
}

/// A rig-wide change: boundaries of one rig that overlap in time and share
/// a spot.
#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub key: String,
    pub rig_uuid: String,
    /// The narrowest window any member gives.
    pub start: i64,
    pub end: i64,
    /// Indices into the book's boundaries.
    pub boundaries: Vec<usize>,
    pub filters: BTreeSet<String>,
    /// The angle of each response family it was seen in.
    pub rotations: Vec<Option<f64>>,
    /// The response families it was seen in, by index.
    pub families: BTreeSet<usize>,
    /// Seen in two filters, or in two response families of one filter: on
    /// the camera side, so it applies to every filter of the rig.
    pub camera_side: bool,
    pub dust: f64,
    pub broad: f64,
    /// The spots of the member with the most dust.
    pub spots: Vec<Spot>,
    /// The window is shorter than one flat run ([`FLAT_RUN_SECONDS`]): the
    /// change landed between the flats of one twilight, or its members
    /// are two changes that met there.
    pub within_flat_run: bool,
}

impl Event {
    pub fn applies_to(&self, filter: Option<&str>) -> bool {
        self.camera_side
            || filter.is_some_and(|filter| self.filters.contains(&canonical_filter(filter)))
    }

    /// Which side of the event a moment is on; `None` inside the window.
    pub fn side_of(&self, at: i64) -> Option<Side> {
        if at <= self.start {
            Some(Side::Before)
        } else if at >= self.end {
            Some(Side::After)
        } else {
            None
        }
    }
}

/// How much a rig's flats depend on the rotator angle: same-filter sessions
/// at different angles shot within one flat session window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Default)]
pub struct AngleTerm {
    pub pairs: usize,
    pub dust: f64,
    pub broad: f64,
}

/// An angle difference folded into (−180°, 180°].
fn wrap_degrees(difference: f64) -> f64 {
    let folded = difference.rem_euclid(360.0);
    if folded > 180.0 {
        folded - 360.0
    } else {
        folded
    }
}

/// How many distinct flat sessions each end of an alike range needs: two.
/// One session at an end can make a range alone: a gradient of its
/// twilight, or a header that changed while the rotator did not, would
/// pass for an angle that does not matter. A second session at that end
/// repeats the measurement.
pub const ALIKE_RANGE_MIN_SESSIONS: usize = 2;

/// How many flat nights each end's sessions must reach: two. Two sessions
/// of one night share its sky, its dust and its temperature.
pub const ALIKE_RANGE_MIN_NIGHTS: usize = 2;

/// Two angles a family's flats measure alike at: the family's own angle
/// term, from pairs of its sessions compared directly. A flat at one end
/// serves the family's lights at the other (see [`AlikeRange::serves`]),
/// from the light's side of every change.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlikeRange {
    /// The recorded angles of the sessions at each end, the short way
    /// round from the first to the second.
    pub ends: [AngleSpan; 2],
    /// The arc each alike pair it rests on spans, going round from `low`
    /// to `high`, never more than a half turn.
    pub arcs: Vec<AngleSpan>,
    /// The span from the first end to the second.
    pub span: AngleSpan,
    pub pairs: usize,
    /// Distinct sessions and flat nights at each end.
    pub sessions: [usize; 2],
    pub ends_nights: [usize; 2],
    /// Flat nights the pairs reach, both ends together.
    pub nights: usize,
    /// The family's groups its pairs come from, and the carrying class
    /// they share ([`Family::carry_class`]): it serves flats of the groups
    /// of that class only.
    pub groups: Vec<usize>,
    pub class: usize,
}

impl AlikeRange {
    /// Whether a light at `light` may take flats at `flat` through it: the
    /// arc of one pair it rests on holds both, within `tolerance` at
    /// either end, and they lie no further apart than that arc plus the
    /// tolerance, so the tolerance counts once. A chain of pairs never
    /// bridges angles no pair compared.
    pub fn serves(&self, light: f64, flat: f64, tolerance: f64) -> bool {
        self.arcs.iter().any(|arc| {
            let width = (arc.high - arc.low).rem_euclid(360.0);
            arc.gap_to(light) <= tolerance
                && arc.gap_to(flat) <= tolerance
                && rotation_gap(light, flat) <= width + tolerance
        })
    }

    /// "101.9–102.2° and 104.2–104.4°".
    pub fn label(&self) -> String {
        format!("{} and {}", self.ends[0].label(), self.ends[1].label())
    }
}

/// A family of sessions: one rig, filter, sensor mode and rotation cluster.
#[derive(Debug, Clone, Serialize)]
pub struct Family {
    pub key: String,
    pub rig_uuid: String,
    /// The sessions' family key: rig, filter, optics and sensor mode,
    /// rotation aside.
    pub family_key: String,
    pub filter: Option<String>,
    /// The middle angle of its largest rotation cluster; absent for flats
    /// that record none.
    pub rotation: Option<f64>,
    /// The recorded angles of each rotation cluster it joins, in angle
    /// order: its measured angle range. Empty for flats that record none.
    pub angles: Vec<AngleSpan>,
    /// Session indices in time order.
    pub sessions: Vec<usize>,
    /// Session indices per group, in time order.
    pub groups: Vec<Vec<usize>>,
    /// Indices into the book's boundaries, between consecutive groups.
    pub boundaries: Vec<usize>,
    /// The angles its flats measure alike at ([`AlikeRange`]), by their
    /// first end. Empty unless enough pairs measured alike.
    pub alike_ranges: Vec<AlikeRange>,
    /// Per group, the class of groups an alike range carries across: the
    /// next group shares its class only when the change between them is
    /// camera-side dust ([`Book::carries_angle_term`]).
    pub carry_class: Vec<usize>,
}

/// A continuous run of lights of one profile.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImagingSession {
    pub key: String,
    pub profile: Option<String>,
    /// The rigs its profile shot with: only their lights it can place.
    pub rigs: Vec<String>,
    pub start: i64,
    pub end: i64,
    /// `acquiredimage` ids, in time order.
    pub lights: Vec<i64>,
}

/// Which side of a change something stands on.
pub use super::evidence::Side;

/// How a light was put on one side of an event.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Placement {
    /// Its capture time is outside the event's window.
    Time { side: Side },
    /// Its imaging session reaches outside the window, and a change does
    /// not land inside one continuous session.
    Session { side: Side, start: i64, end: i64 },
    /// Its imaging session's own lights carry the change, or lack it.
    Pixels {
        side: Side,
        targets: Vec<String>,
        filters: Vec<String>,
        lights: usize,
    },
    /// None of those decide.
    Undetermined {
        session: Option<(i64, i64)>,
        pixels: Option<String>,
    },
}

impl Placement {
    pub fn side(&self) -> Option<Side> {
        match self {
            Placement::Time { side }
            | Placement::Session { side, .. }
            | Placement::Pixels { side, .. } => Some(*side),
            Placement::Undetermined { .. } => None,
        }
    }
}

/// Everything grouping knows about one catalog.
#[derive(Debug, Default)]
pub struct Book {
    pub sessions: Vec<Session>,
    pub families: Vec<Family>,
    pub boundaries: Vec<Boundary>,
    pub events: Vec<Event>,
    /// Light angle epochs: each rig's sky − mechanical rotator offset over
    /// time, from the lights' own solves (see [`super::lights`]).
    pub epochs: AngleEpochs,
    /// What imaging sessions' own lights said about which family their
    /// flats come from, by session key and filter.
    pub family_evidence: FamilyEvidence,
    /// Boundaries on either side of one night unlike the nights around it,
    /// which measure alike with each other: a hazy dawn, not a change.
    /// They split groups but make no event, so no light is flagged for them.
    pub outliers: BTreeSet<usize>,
    /// Splits across two changes another family tells apart: they date
    /// neither, and make no event of their own.
    pub spanning: BTreeSet<usize>,
    pub angle_terms: HashMap<String, AngleTerm>,
    pub imaging: Vec<ImagingSession>,
    /// Each session's family, by index.
    pub(super) family_of: Vec<usize>,
    /// Each session's rotation cluster, by index; `usize::MAX` for one that
    /// records no angle.
    cluster_of: Vec<usize>,
    /// Each rotation cluster's sessions, with its rig and family key.
    clusters: Vec<(String, String, Vec<usize>)>,
    by_response: HashMap<String, usize>,
    by_frame: HashMap<String, usize>,
    comparisons: HashMap<(usize, usize), ComparisonRow>,
    evidence: HashMap<(String, String), EvidenceRow>,
    limits: HashMap<String, Limits>,
}

/// Filters compare without case or surrounding space.
pub fn canonical_filter(filter: &str) -> String {
    filter.trim().to_ascii_lowercase()
}

fn rotation_gap(a: f64, b: f64) -> f64 {
    let difference = (a - b).rem_euclid(360.0);
    difference.min(360.0 - difference)
}

/// Single-linkage rotation clusters of `(session, angle)`: around the
/// circle, a gap wider than `tolerance` between neighbouring angles starts
/// a new cluster, and nothing else does. Each cluster in angle order.
fn rotation_clusters(mut angled: Vec<(usize, f64)>, tolerance: f64) -> Vec<Vec<(usize, f64)>> {
    let n = angled.len();
    if n == 0 {
        return Vec::new();
    }
    let around = |v: f64| v.rem_euclid(360.0);
    angled.sort_by(|a, b| around(a.1).total_cmp(&around(b.1)).then(a.0.cmp(&b.0)));
    // The gap from each angle to the next, the last wrapping to the first.
    let gaps: Vec<f64> = (0..n)
        .map(|i| {
            let (a, b) = (around(angled[i].1), around(angled[(i + 1) % n].1));
            if i + 1 < n {
                b - a
            } else {
                b + 360.0 - a
            }
        })
        .collect();
    let Some(first_cut) = (0..n).find(|i| gaps[*i] > tolerance) else {
        return vec![angled];
    };
    let mut clusters = Vec::new();
    let mut current = Vec::new();
    for step in 1..=n {
        let i = (first_cut + step) % n;
        current.push(angled[i]);
        if gaps[i] > tolerance {
            clusters.push(std::mem::take(&mut current));
        }
    }
    clusters
}

/// A family while clusters join.
struct FamilyDraft {
    /// The middle angle of its largest cluster.
    middle: Option<f64>,
    anchor_size: usize,
    angles: Vec<AngleSpan>,
    sessions: Vec<usize>,
}

impl Book {
    /// Build the book from the catalog's rows, with each rig's limits from
    /// `limits_for`.
    pub fn build(
        responses: Vec<ResponseRow>,
        comparisons: Vec<ComparisonRow>,
        evidence: Vec<EvidenceRow>,
        imaging: Vec<ImagingSession>,
        limits_for: impl Fn(&str) -> Limits,
    ) -> Self {
        let mut book = Book::default();
        for row in responses {
            let (Some(first), Some(last)) = (row.captured_first, row.captured_last) else {
                continue;
            };
            if row.error.is_some() {
                continue;
            }
            let index = book.sessions.len();
            book.by_response.insert(row.response_key.clone(), index);
            for uuid in &row.frame_uuids {
                book.by_frame.insert(uuid.clone(), index);
            }
            book.sessions.push(Session {
                row,
                first,
                last,
                inferred_rotation: None,
            });
        }
        for comparison in comparisons {
            let (Some(&a), Some(&b)) = (
                book.by_response.get(&comparison.earlier_key),
                book.by_response.get(&comparison.later_key),
            ) else {
                continue;
            };
            book.comparisons.insert((a, b), comparison);
        }
        for row in evidence {
            book.evidence
                .insert((row.session_key.clone(), row.event_key.clone()), row);
        }
        book.imaging = imaging;
        let rigs: BTreeSet<String> = book
            .sessions
            .iter()
            .map(|s| s.row.rig_uuid.clone())
            .collect();
        for rig in rigs {
            book.limits.insert(rig.clone(), limits_for(&rig));
        }
        book.build_families();
        book.build_groups();
        book.find_outliers();
        book.find_spanning();
        book.build_events();
        book.build_angle_terms();
        book.build_alike_ranges();
        book
    }

    pub fn limits(&self, rig: &str) -> Option<Limits> {
        self.limits.get(rig).copied()
    }

    /// The comparison of two sessions, whichever was earlier.
    pub fn comparison(&self, a: usize, b: usize) -> Option<&ComparisonRow> {
        self.comparisons
            .get(&(a, b))
            .or_else(|| self.comparisons.get(&(b, a)))
    }

    /// The measured session a flat belongs to.
    pub fn session_of_frame(&self, frame_uuid: &str) -> Option<usize> {
        self.by_frame.get(frame_uuid).copied()
    }

    pub fn session_by_response(&self, key: &str) -> Option<usize> {
        self.by_response.get(key).copied()
    }

    fn build_families(&mut self) {
        let mut by_key: HashMap<(String, String), Vec<usize>> = HashMap::new();
        for (index, session) in self.sessions.iter().enumerate() {
            by_key
                .entry((session.row.rig_uuid.clone(), session.row.family_key.clone()))
                .or_default()
                .push(index);
        }
        let mut keys: Vec<_> = by_key.keys().cloned().collect();
        keys.sort();
        self.family_of = vec![usize::MAX; self.sessions.len()];
        self.cluster_of = vec![usize::MAX; self.sessions.len()];
        for key in keys {
            let members = by_key.remove(&key).unwrap_or_default();
            let limits = self.limits(&key.0);
            // The rig's tolerance, capped at the coherence tolerance one
            // master's flats are held to: a wide match setting lets lights
            // reach further for flats, it does not make two angles one.
            let tolerance = limits.map_or(ROTATION_CLUSTER_DEG, |limits| {
                limits.rotation.min(ROTATION_CLUSTER_DEG)
            });
            let (angled, unknown): (Vec<usize>, Vec<usize>) = members
                .into_iter()
                .partition(|index| self.angle(*index).is_some());
            let clusters = rotation_clusters(
                angled
                    .into_iter()
                    .filter_map(|index| Some((index, self.angle(index)?)))
                    .collect(),
                tolerance,
            );
            // Every rotation cluster starts as a family of its own.
            let mut families: Vec<FamilyDraft> = Vec::new();
            for cluster in clusters {
                let id = self.clusters.len();
                let sessions: Vec<usize> = cluster.iter().map(|(i, _)| *i).collect();
                for index in &sessions {
                    self.cluster_of[*index] = id;
                }
                self.clusters
                    .push((key.0.clone(), key.1.clone(), sessions.clone()));
                families.push(FamilyDraft {
                    middle: Some(cluster[cluster.len() / 2].1),
                    anchor_size: cluster.len(),
                    angles: vec![AngleSpan {
                        low: cluster[0].1.rem_euclid(360.0),
                        high: cluster[cluster.len() - 1].1.rem_euclid(360.0),
                    }],
                    sessions,
                });
            }
            // Clusters whose sessions measure alike are one light path.
            if let Some(limits) = limits {
                self.join_alike_clusters(&mut families, &limits);
            }
            // Flats that record no angle join the family they measure
            // alike with; the rest stay together on their own.
            let mut alone = Vec::new();
            let members: Vec<Vec<usize>> = families.iter().map(|f| f.sessions.clone()).collect();
            for index in unknown {
                match self.measured_cluster(index, &members) {
                    Some((family, angle)) => {
                        self.sessions[index].inferred_rotation = Some(angle);
                        families[family].sessions.push(index);
                    }
                    None => alone.push(index),
                }
            }
            if !alone.is_empty() {
                families.push(FamilyDraft {
                    middle: None,
                    anchor_size: 0,
                    angles: Vec::new(),
                    sessions: alone,
                });
            }
            for mut draft in families {
                draft
                    .sessions
                    .sort_by_key(|index| (self.sessions[*index].first, *index));
                draft
                    .angles
                    .sort_by(|a, b| a.low.total_cmp(&b.low).then(a.high.total_cmp(&b.high)));
                let filter = self.sessions[draft.sessions[0]].row.filter.clone();
                for index in &draft.sessions {
                    self.family_of[*index] = self.families.len();
                }
                // Named by its earliest session's first flat: a session
                // that arrives later never renames it, nor does a new night
                // boundary, so a choice made about the family by hand can
                // find it again.
                let anchor = self.sessions[draft.sessions[0]]
                    .row
                    .frame_uuids
                    .first()
                    .cloned()
                    .unwrap_or_else(|| self.sessions[draft.sessions[0]].row.response_key.clone());
                self.families.push(Family {
                    key: format!("{}:{anchor}", key.1),
                    rig_uuid: key.0.clone(),
                    family_key: key.1.clone(),
                    filter,
                    rotation: draft.middle,
                    angles: draft.angles,
                    sessions: draft.sessions,
                    groups: Vec::new(),
                    boundaries: Vec::new(),
                    alike_ranges: Vec::new(),
                    carry_class: Vec::new(),
                });
            }
        }
    }

    /// Join rotation clusters of one family key whose sessions measure
    /// alike, closest first. Two clusters join when some pair of their
    /// sessions, shot within one flat window or neighbours in time, is
    /// alike within the limits, and no such pair is apart: complete linkage
    /// over the pairs that speak to the angle. A pair apart keeps the
    /// clusters apart, so a light is never handed flats that measured apart
    /// from its own angle's, unless a change of dust explains it: a third
    /// session, beside one of the pair at its angle, measured alike with
    /// the other and apart from its own neighbour. Clusters never measured
    /// alike stay apart.
    fn join_alike_clusters(&self, families: &mut Vec<FamilyDraft>, limits: &Limits) {
        loop {
            let mut best: Option<(usize, usize, f64)> = None;
            for x in 0..families.len() {
                for y in x + 1..families.len() {
                    if let Some(distance) =
                        self.family_link(&families[x].sessions, &families[y].sessions, limits)
                        && best.is_none_or(|(_, _, b)| distance < b)
                    {
                        best = Some((x, y, distance));
                    }
                }
            }
            let Some((x, y, _)) = best else { break };
            let joined = families.remove(y);
            let into = &mut families[x];
            if joined.anchor_size > into.anchor_size {
                into.middle = joined.middle;
                into.anchor_size = joined.anchor_size;
            }
            into.angles.extend(joined.angles);
            into.sessions.extend(joined.sessions);
        }
    }

    /// The closest likeness between two sets of sessions, when they may
    /// join: see [`Self::join_alike_clusters`].
    fn family_link(&self, left: &[usize], right: &[usize], limits: &Limits) -> Option<f64> {
        let mut alike: Option<f64> = None;
        for a in left {
            let right_neighbours = self.neighbours_in(*a, right);
            for b in right {
                let Some(distance) = self.pair_distance(*a, *b, limits) else {
                    continue;
                };
                let within =
                    (self.sessions[*a].first - self.sessions[*b].first).abs() <= limits.window;
                let neighbours =
                    right_neighbours.contains(b) || self.neighbours_in(*b, left).contains(a);
                if (within || neighbours)
                    && distance > 1.0
                    && !self.apart_by_a_change(*a, *b, left, right, limits)
                {
                    return None;
                }
                if (within || neighbours) && distance <= 1.0 {
                    alike = Some(alike.map_or(distance, |d: f64| d.min(distance)));
                }
            }
        }
        alike
    }

    /// Whether sessions `a` (of `left`) and `b` (of `right`), measured
    /// apart, differ by a change of dust rather than by their angles: a
    /// session at one's angle measured alike with the other, and apart
    /// from the one beside it. A morning's two angles alike, and the same
    /// angle apart from an earlier morning, is new dust, not the angle.
    fn apart_by_a_change(
        &self,
        a: usize,
        b: usize,
        left: &[usize],
        right: &[usize],
        limits: &Limits,
    ) -> bool {
        let alike = |x: usize, y: usize| self.pair_distance(x, y, limits).is_some_and(|d| d <= 1.0);
        let apart = |x: usize, y: usize| self.pair_distance(x, y, limits).is_some_and(|d| d > 1.0);
        left.iter().any(|c| *c != a && alike(*c, b) && apart(a, *c))
            || right
                .iter()
                .any(|c| *c != b && alike(a, *c) && apart(*c, b))
    }

    /// How far two sessions measured apart against `limits`, with the noise
    /// of a session of fewer than [`MEDIAN_FRAMES`] flats allowed for;
    /// `None` when they were not compared.
    pub fn pair_distance(&self, a: usize, b: usize, limits: &Limits) -> Option<f64> {
        let comparison = self.comparison(a, b)?;
        let few = |i: usize| self.sessions[i].row.frames < MEDIAN_FRAMES;
        Some(if few(a) || few(b) {
            limits.noisy_distance(comparison)
        } else {
            limits.distance(comparison)
        })
    }

    /// Whether two sessions both record angles, in different rotation
    /// clusters: a pair at two angles. A session whose flats record no
    /// angle sits at the angle it measured alike with, never at another.
    pub fn at_two_angles(&self, a: usize, b: usize) -> bool {
        self.angle(a).is_some()
            && self.angle(b).is_some()
            && self.cluster_of[a] != self.cluster_of[b]
    }

    /// A session's recorded rotator angle, when it has a usable one.
    fn angle(&self, index: usize) -> Option<f64> {
        self.sessions[index].row.rotation.filter(|r| r.is_finite())
    }

    /// The sessions of `cluster` (angled ones only) just before and just
    /// after `session` in time.
    fn neighbours_in(&self, session: usize, cluster: &[usize]) -> Vec<usize> {
        let at = (self.sessions[session].first, session);
        let key = |i: usize| (self.sessions[i].first, i);
        let angled = cluster.iter().copied().filter(|i| self.angle(*i).is_some());
        let before = angled
            .clone()
            .filter(|i| key(*i) < at)
            .max_by_key(|i| key(*i));
        let after = angled.filter(|i| key(*i) > at).min_by_key(|i| key(*i));
        before.into_iter().chain(after).collect()
    }

    /// For a session whose flats record no angle: the family one of whose
    /// neighbours in time it measures alike with, and that neighbour's
    /// angle. Only when the measurements single the family out: every
    /// other family measured, and unlike. On a rig whose angles barely
    /// matter a night alike with several families says nothing of its
    /// angle, and stays unknown.
    fn measured_cluster(&self, session: usize, families: &[Vec<usize>]) -> Option<(usize, f64)> {
        let limits = self.limits(&self.sessions[session].row.rig_uuid)?;
        let mut alike = None;
        for (c, members) in families.iter().enumerate() {
            let closest = self
                .neighbours_in(session, members)
                .into_iter()
                .filter_map(|n| Some((self.pair_distance(session, n, &limits)?, self.angle(n)?)))
                .min_by(|a, b| a.0.total_cmp(&b.0));
            match closest {
                // Never compared: nothing rules this family out.
                None => return None,
                Some((distance, angle)) if distance <= 1.0 => {
                    if alike.is_some() {
                        return None;
                    }
                    alike = Some((c, angle));
                }
                Some(_) => {}
            }
        }
        alike
    }

    /// Whether two sessions of one family are compared: within twice the
    /// window of each other, or neighbours in time.
    fn compared_pair(&self, a: usize, b: usize, neighbours: bool, limits: &Limits) -> bool {
        neighbours || (self.sessions[a].first - self.sessions[b].first).abs() <= 2 * limits.window
    }

    fn linkage(
        &self,
        left: &[usize],
        right: &[usize],
        limits: &Limits,
        order: &[usize],
    ) -> Option<f64> {
        let position = |session: usize| order.iter().position(|s| *s == session).unwrap_or(0);
        let mut worst: Option<f64> = None;
        for a in left {
            for b in right {
                let neighbours = position(*b) == position(*a) + 1;
                if !self.compared_pair(*a, *b, neighbours, limits) {
                    continue;
                }
                if let Some(distance) = self.pair_distance(*a, *b, limits) {
                    worst = Some(worst.map_or(distance, |w: f64| w.max(distance)));
                }
            }
        }
        worst
    }

    fn build_groups(&mut self) {
        for family_index in 0..self.families.len() {
            let family = &self.families[family_index];
            let Some(limits) = self.limits(&family.rig_uuid) else {
                continue;
            };
            let order = family.sessions.clone();
            let mut groups: Vec<Vec<usize>> = order.iter().map(|s| vec![*s]).collect();
            // Merge the neighbouring pair with the smallest linkage while it
            // stays within the limits. A pair never compared has no evidence
            // of a difference and merges first.
            loop {
                let mut best: Option<(usize, f64)> = None;
                for i in 0..groups.len().saturating_sub(1) {
                    let linkage = self
                        .linkage(&groups[i], &groups[i + 1], &limits, &order)
                        .unwrap_or(0.0);
                    if linkage <= 1.0 && best.is_none_or(|(_, b)| linkage < b) {
                        best = Some((i, linkage));
                    }
                }
                let Some((i, _)) = best else { break };
                let right = groups.remove(i + 1);
                groups[i].extend(right);
            }
            let mut boundaries = Vec::new();
            for pair in groups.windows(2) {
                let last = *pair[0].last().expect("groups are not empty");
                let first = pair[1][0];
                // The change is told by the nearest pair across it at one
                // angle: a family's two angles differ by their light path
                // too, so a pair at two angles says nothing of the change.
                let (before, after) = pair[0]
                    .iter()
                    .flat_map(|a| pair[1].iter().map(move |b| (*a, *b)))
                    .filter(|(a, b)| !self.at_two_angles(*a, *b))
                    .min_by_key(|(a, b)| (self.sessions[*b].first - self.sessions[*a].last, *a, *b))
                    .unwrap_or((last, first));
                let neighbour = self.comparison(before, after);
                let distance = self.pair_distance(before, after, &limits);
                let drift = if distance.is_none_or(|d| d <= 1.0) {
                    // The neighbours agree: say which pair across the
                    // boundary did not.
                    let mut worst: Option<(usize, usize, f64)> = None;
                    for a in &pair[0] {
                        for b in &pair[1] {
                            if let Some(d) = self.pair_distance(*a, *b, &limits)
                                && worst.is_none_or(|(_, _, w)| d > w)
                            {
                                worst = Some((*a, *b, d));
                            }
                        }
                    }
                    worst
                } else {
                    None
                };
                let index = self.boundaries.len();
                self.boundaries.push(Boundary {
                    family: family_index,
                    before,
                    after,
                    start: self.sessions[last].last,
                    end: self.sessions[first].first,
                    dust: neighbour.map(|c| c.dust),
                    broad: neighbour.map(|c| c.broad),
                    drift,
                    spots: neighbour.map(|c| c.spots.clone()).unwrap_or_default(),
                });
                boundaries.push(index);
            }
            self.families[family_index].groups = groups;
            self.families[family_index].boundaries = boundaries;
        }
    }

    fn shares_spot(a: &Boundary, b: &Boundary) -> bool {
        Self::spots_meet(
            &a.spots.iter().collect::<Vec<_>>(),
            &b.spots.iter().collect::<Vec<_>>(),
        )
    }

    /// Whether `member` shows a shadow of `split`: one of the split's
    /// strong spots, in `member` at least half as deep. A comparison lists
    /// faint spots down to 5σ, and one of them landing near another's is
    /// no shared shadow; a shallower shadow beside a deeper one is.
    fn shows_shadow(member: &Boundary, split: &Boundary) -> bool {
        Self::strong_spots(split).iter().any(|shadow| {
            member.spots.iter().any(|spot| {
                spot.darker == shadow.darker
                    && spot.peak >= 0.5 * shadow.peak
                    && ((spot.x - shadow.x).powi(2) + (spot.y - shadow.y).powi(2)).sqrt()
                        <= spot.radius.max(shadow.radius)
            })
        })
    }

    /// Groups with a split on either side, as `(split before, split
    /// after)`: each proves two changes, one in either split.
    fn bounded_groups(&self) -> Vec<(usize, usize)> {
        let mut bounded = Vec::new();
        for family in &self.families {
            for g in 1..family.groups.len().saturating_sub(1) {
                let (left, right) = (family.boundaries[g - 1], family.boundaries[g]);
                let change = |b: usize| {
                    !self.outliers.contains(&b) && {
                        let boundary = &self.boundaries[b];
                        !self.at_two_angles(boundary.before, boundary.after)
                    }
                };
                if change(left) && change(right) {
                    bounded.push((left, right));
                }
            }
        }
        bounded
    }

    /// Whether some family of the rig saw two changes inside `span` (give
    /// or take one flat run, as one morning's filters are shot minutes
    /// apart), and `shows` holds of the splits of both.
    fn two_changes_within(
        &self,
        bounded: &[(usize, usize)],
        rig: &str,
        span: (i64, i64),
        shows: impl Fn(usize) -> bool,
    ) -> bool {
        bounded.iter().any(|(before, after)| {
            let (before_split, after_split) = (&self.boundaries[*before], &self.boundaries[*after]);
            self.families[before_split.family].rig_uuid == rig
                && before_split.start >= span.0 - FLAT_RUN_SECONDS
                && after_split.end <= span.1 + FLAT_RUN_SECONDS
                && shows(*before)
                && shows(*after)
        })
    }

    /// Splits across two changes: another family saw both inside the
    /// split's window, and the split shows the shadows of both. Such a
    /// split dates neither change; it joins no event and only confirms,
    /// after, which filters and angles saw the changes it shows.
    fn find_spanning(&mut self) {
        let bounded = self.bounded_groups();
        let spanning: BTreeSet<usize> = (0..self.boundaries.len())
            .filter(|b| !self.outliers.contains(b))
            .filter(|b| {
                let split = &self.boundaries[*b];
                let family = &self.families[split.family];
                self.two_changes_within(
                    &bounded,
                    &family.rig_uuid,
                    (split.start, split.end),
                    |other| {
                        self.boundaries[other].family != split.family
                            && Self::shows_shadow(split, &self.boundaries[other])
                    },
                )
            })
            .collect();
        self.spanning = spanning;
    }

    /// A boundary's spots at least half as deep as its deepest.
    fn strong_spots(boundary: &Boundary) -> Vec<&Spot> {
        let deepest = boundary
            .spots
            .iter()
            .map(|spot| spot.peak)
            .fold(0f32, f32::max);
        boundary
            .spots
            .iter()
            .filter(|spot| spot.peak >= 0.5 * deepest)
            .collect()
    }

    fn spots_meet(a: &[&Spot], b: &[&Spot]) -> bool {
        a.iter().any(|sa| {
            b.iter().any(|sb| {
                sa.darker == sb.darker && {
                    let distance = ((sa.x - sb.x).powi(2) + (sa.y - sb.y).powi(2)).sqrt();
                    distance <= sa.radius.max(sb.radius)
                }
            })
        })
    }

    fn find_outliers(&mut self) {
        let mut outliers = BTreeSet::new();
        for family in &self.families {
            let Some(limits) = self.limits(&family.rig_uuid) else {
                continue;
            };
            for g in 1..family.groups.len().saturating_sub(1) {
                if family.groups[g].len() != 1 {
                    continue;
                }
                let (Some(before), Some(after)) =
                    (family.groups[g - 1].last(), family.groups[g + 1].first())
                else {
                    continue;
                };
                if self
                    .pair_distance(*before, *after, &limits)
                    .is_some_and(|d| d <= 1.0)
                {
                    outliers.insert(family.boundaries[g - 1]);
                    outliers.insert(family.boundaries[g]);
                }
            }
        }
        self.outliers = outliers;
    }

    fn build_events(&mut self) {
        // Union boundaries of one rig whose windows overlap and whose spots
        // coincide; the merged window is the overlap of all.
        let mut parent: Vec<usize> = (0..self.boundaries.len()).collect();
        fn find(parent: &mut [usize], i: usize) -> usize {
            let mut root = i;
            while parent[root] != root {
                root = parent[root];
            }
            parent[i] = root;
            root
        }
        let mut windows: Vec<(i64, i64)> =
            self.boundaries.iter().map(|b| (b.start, b.end)).collect();
        // The span the members of each merge reach over, first to last.
        let mut spans = windows.clone();
        // Each merge's members, by its root.
        let mut merged: Vec<Vec<usize>> = (0..self.boundaries.len()).map(|i| vec![i]).collect();
        let bounded = self.bounded_groups();
        // A boundary between two angles of one family is a split of the
        // family, never a change: the two angles may differ by the angle
        // alone.
        let across_angles = |i: usize| {
            let boundary = &self.boundaries[i];
            self.at_two_angles(boundary.before, boundary.after)
        };
        let apart =
            |i: usize| self.outliers.contains(&i) || self.spanning.contains(&i) || across_angles(i);
        for i in 0..self.boundaries.len() {
            for j in (i + 1)..self.boundaries.len() {
                if apart(i) || apart(j) {
                    continue;
                }
                let (a, b) = (&self.boundaries[i], &self.boundaries[j]);
                let (fa, fb) = (&self.families[a.family], &self.families[b.family]);
                if fa.rig_uuid != fb.rig_uuid || a.family == b.family {
                    continue;
                }
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri == rj {
                    continue;
                }
                let (wi, wj) = (windows[ri], windows[rj]);
                let (start, end) = (wi.0.max(wj.0), wi.1.min(wj.1));
                if start >= end || !Self::shares_spot(a, b) {
                    continue;
                }
                // One change, or two that another family tells apart: a
                // group of its sessions with a split on either side, both
                // splits inside the span the members reach over, and the
                // members showing the shadows of both.
                let span = (spans[ri].0.min(spans[rj].0), spans[ri].1.max(spans[rj].1));
                let members: Vec<usize> = merged[ri].iter().chain(&merged[rj]).copied().collect();
                let seen = |split: usize| {
                    members
                        .iter()
                        .any(|m| Self::shows_shadow(&self.boundaries[*m], &self.boundaries[split]))
                };
                if self.two_changes_within(&bounded, &fa.rig_uuid, span, seen) {
                    continue;
                }
                parent[rj] = ri;
                windows[ri] = (start, end);
                spans[ri] = span;
                merged[ri] = members;
            }
        }
        let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in (0..self.boundaries.len()).filter(|i| !apart(*i)) {
            let root = find(&mut parent, i);
            members.entry(root).or_default().push(i);
        }
        let mut roots: Vec<usize> = members.keys().copied().collect();
        roots.sort_by_key(|root| (windows[*root].0, *root));
        for root in roots {
            let boundaries = members.remove(&root).unwrap_or_default();
            let (start, end) = windows[root];
            let first = &self.boundaries[boundaries[0]];
            let rig = self.families[first.family].rig_uuid.clone();
            let filters: BTreeSet<String> = boundaries
                .iter()
                .filter_map(|b| self.families[self.boundaries[*b].family].filter.clone())
                .map(|f| canonical_filter(&f))
                .collect();
            let mut rotations: Vec<Option<f64>> = Vec::new();
            let mut families: BTreeSet<usize> = BTreeSet::new();
            for b in &boundaries {
                let family = self.boundaries[*b].family;
                if families.insert(family) {
                    rotations.push(self.families[family].rotation);
                }
            }
            let strongest = boundaries
                .iter()
                .copied()
                .max_by(|a, b| {
                    let dust = |i: usize| self.boundaries[i].dust.unwrap_or(0.0);
                    dust(*a).total_cmp(&dust(*b))
                })
                .unwrap_or(boundaries[0]);
            let dust = boundaries
                .iter()
                .filter_map(|b| self.boundaries[*b].dust)
                .fold(0f64, f64::max);
            let broad = boundaries
                .iter()
                .filter_map(|b| self.boundaries[*b].broad)
                .fold(0f64, f64::max);
            self.events.push(Event {
                key: event_key(&rig, start, end),
                rig_uuid: rig,
                start,
                end,
                camera_side: filters.len() >= 2 || self.two_families_of_one_key(&families),
                filters,
                rotations,
                families,
                dust,
                broad,
                spots: self.boundaries[strongest].spots.clone(),
                boundaries,
                within_flat_run: end - start < FLAT_RUN_SECONDS,
            });
        }
        // A split across two changes still saw each change it shows: its
        // filter and angle count toward the camera side, its window toward
        // nothing.
        for b in self.spanning.clone() {
            let split = &self.boundaries[b];
            let family = &self.families[split.family];
            for event in &mut self.events {
                let inside = event.start >= split.start - FLAT_RUN_SECONDS
                    && event.end <= split.end + FLAT_RUN_SECONDS;
                if event.rig_uuid != family.rig_uuid
                    || !inside
                    || !event
                        .boundaries
                        .iter()
                        .any(|m| Self::shows_shadow(split, &self.boundaries[*m]))
                {
                    continue;
                }
                if let Some(filter) = &family.filter {
                    event.filters.insert(canonical_filter(filter));
                }
                if event.families.insert(split.family) {
                    event.rotations.push(family.rotation);
                }
            }
        }
        for index in 0..self.events.len() {
            let two = self.two_families_of_one_key(&self.events[index].families);
            let event = &mut self.events[index];
            event.camera_side = event.filters.len() >= 2 || two;
        }
    }

    /// Whether two of `families` are of one family key (one filter, optics
    /// and sensor mode) and so differ by their light path alone: a change
    /// seen in both turned with the camera.
    fn two_families_of_one_key(&self, families: &BTreeSet<usize>) -> bool {
        let mut keys = BTreeSet::new();
        families
            .iter()
            .any(|family| !keys.insert(self.families[*family].family_key.as_str()))
    }

    fn build_angle_terms(&mut self) {
        for i in 0..self.sessions.len() {
            for j in (i + 1)..self.sessions.len() {
                if !self.across_clusters(i, j) {
                    continue;
                }
                let (a, b) = (&self.sessions[i], &self.sessions[j]);
                let Some(limits) = self.limits(&a.row.rig_uuid) else {
                    continue;
                };
                if (a.first - b.first).abs() > limits.window {
                    continue;
                }
                let Some(comparison) = self.comparison(i, j) else {
                    continue;
                };
                let (dust, broad, rig) =
                    (comparison.dust, comparison.broad, a.row.rig_uuid.clone());
                let term = self.angle_terms.entry(rig).or_default();
                term.pairs += 1;
                term.dust = term.dust.max(dust);
                term.broad = term.broad.max(broad);
            }
        }
    }

    /// Each family's [`AlikeRange`]s, from pairs of its sessions at two
    /// angles further apart than the rig's tolerance, compared directly,
    /// in one group, with no change of the rig that reaches the filter
    /// between them. A range gathers the alike pairs whose ends lie within
    /// the tolerance of one pair's ends, from groups of one carrying class
    /// ([`Self::carries_angle_term`]). It needs
    /// [`ALIKE_RANGE_MIN_SESSIONS`] distinct sessions on
    /// [`ALIKE_RANGE_MIN_NIGHTS`] nights at each end, and no pair of the
    /// family measured apart that it would serve. Only measured pairs
    /// count: a header's angle never makes or widens one.
    fn build_alike_ranges(&mut self) {
        for family_index in 0..self.families.len() {
            let classes = self.carry_classes(family_index);
            self.families[family_index].carry_class = classes;
            let ranges = self.alike_ranges_of(family_index);
            self.families[family_index].alike_ranges = ranges;
        }
    }

    /// Per group of a family, its carrying class: consecutive groups share
    /// one when the change between them carries an angle term.
    fn carry_classes(&self, family_index: usize) -> Vec<usize> {
        let family = &self.families[family_index];
        let limits = self.limits(&family.rig_uuid);
        let mut classes = Vec::with_capacity(family.groups.len());
        for g in 0..family.groups.len() {
            let class = match (g, limits) {
                (0, _) => 0,
                (_, Some(limits)) if self.carries_angle_term(family.boundaries[g - 1], &limits) => {
                    classes[g - 1]
                }
                _ => classes[g - 1] + 1,
            };
            classes.push(class);
        }
        classes
    }

    /// Whether an angle term measured on one side of a boundary holds on
    /// the other: only when the boundary is part of a camera-side change of
    /// dust, with spots and no broad change over the limit. How flats
    /// depend on the angle comes from the telescope side (vignetting,
    /// reflections, the light path through a rotating camera's optics);
    /// dust on the camera's window or filter moves with the camera, adds
    /// the same shadows at every angle, and leaves that dependence as it
    /// was. A broad change, a change seen in one filter alone (filter or
    /// telescope side), a split that makes no change, or a split made by
    /// hand carries nothing.
    // TODO(#878): a boundary a person split or joined by hand carries
    // nothing; check the choices here once they reach the book.
    pub fn carries_angle_term(&self, boundary: usize, limits: &Limits) -> bool {
        self.events.iter().any(|event| {
            event.boundaries.contains(&boundary)
                && event.camera_side
                && !event.spots.is_empty()
                && event.broad <= limits.broad
        })
    }

    fn alike_ranges_of(&self, family_index: usize) -> Vec<AlikeRange> {
        struct Pair<'a> {
            ends: [(usize, f64); 2],
            arc: AngleSpan,
            alike: bool,
            group: usize,
            class: usize,
            nights: [&'a str; 2],
        }
        let family = &self.families[family_index];
        let Some(limits) = self.limits(&family.rig_uuid) else {
            return Vec::new();
        };
        let tolerance = limits.rotation;
        let mut pairs: Vec<Pair> = Vec::new();
        for (g, group) in family.groups.iter().enumerate() {
            for (i, a) in group.iter().enumerate() {
                for b in &group[i + 1..] {
                    let (Some(x), Some(y)) = (self.angle(*a), self.angle(*b)) else {
                        continue;
                    };
                    if rotation_gap(x, y) <= tolerance || self.change_between(family, *a, *b) {
                        continue;
                    }
                    let Some(distance) = self.pair_distance(*a, *b, &limits) else {
                        continue;
                    };
                    // The short way round, from the first end.
                    let (from, to) = if (y - x).rem_euclid(360.0) <= 180.0 {
                        ((*a, x), (*b, y))
                    } else {
                        ((*b, y), (*a, x))
                    };
                    pairs.push(Pair {
                        ends: [from, to],
                        arc: AngleSpan {
                            low: from.1,
                            high: to.1,
                        },
                        alike: distance <= 1.0,
                        group: g,
                        class: family.carry_class.get(g).copied().unwrap_or(g),
                        nights: [
                            self.sessions[from.0].row.night.as_str(),
                            self.sessions[to.0].row.night.as_str(),
                        ],
                    });
                }
            }
        }
        // Which end of `q` lies at each end of `p`, if both do.
        let lined_up = |p: &Pair, q: &Pair| -> Option<[usize; 2]> {
            let near = |a: f64, b: f64| rotation_gap(a, b) <= tolerance;
            if near(p.ends[0].1, q.ends[0].1) && near(p.ends[1].1, q.ends[1].1) {
                Some([0, 1])
            } else if near(p.ends[0].1, q.ends[1].1) && near(p.ends[1].1, q.ends[0].1) {
                Some([1, 0])
            } else {
                None
            }
        };
        let mut seen: BTreeSet<Vec<usize>> = BTreeSet::new();
        let mut found: Vec<AlikeRange> = Vec::new();
        for seed in pairs.iter().filter(|pair| pair.alike) {
            let members: Vec<(usize, [usize; 2])> = pairs
                .iter()
                .enumerate()
                .filter(|(_, q)| q.alike && q.class == seed.class)
                .filter_map(|(index, q)| lined_up(seed, q).map(|order| (index, order)))
                .collect();
            if !seen.insert(members.iter().map(|(index, _)| *index).collect()) {
                continue;
            }
            let mut sessions: [BTreeSet<usize>; 2] = Default::default();
            let mut nights: [BTreeSet<&str>; 2] = Default::default();
            let mut angles: [Vec<f64>; 2] = Default::default();
            let mut groups = BTreeSet::new();
            for (index, order) in &members {
                let q = &pairs[*index];
                groups.insert(q.group);
                for side in 0..2 {
                    let end = order[side];
                    sessions[side].insert(q.ends[end].0);
                    nights[side].insert(q.nights[end]);
                    angles[side].push(q.ends[end].1);
                }
            }
            if sessions
                .iter()
                .any(|set| set.len() < ALIKE_RANGE_MIN_SESSIONS)
                || nights.iter().any(|set| set.len() < ALIKE_RANGE_MIN_NIGHTS)
            {
                continue;
            }
            let span_of = |angles: &[f64], around: f64| {
                let offsets = angles.iter().map(|a| wrap_degrees(a - around));
                let low = offsets.clone().fold(f64::INFINITY, f64::min);
                let high = offsets.fold(f64::NEG_INFINITY, f64::max);
                AngleSpan {
                    low: (around + low).rem_euclid(360.0),
                    high: (around + high).rem_euclid(360.0),
                }
            };
            let ends = [
                span_of(&angles[0], seed.ends[0].1),
                span_of(&angles[1], seed.ends[1].1),
            ];
            let all_nights: BTreeSet<&str> = nights[0].union(&nights[1]).copied().collect();
            let range = AlikeRange {
                ends,
                arcs: members.iter().map(|(index, _)| pairs[*index].arc).collect(),
                span: AngleSpan {
                    low: ends[0].low,
                    high: ends[1].high,
                },
                pairs: members.len(),
                sessions: [sessions[0].len(), sessions[1].len()],
                ends_nights: [nights[0].len(), nights[1].len()],
                nights: all_nights.len(),
                groups: groups.into_iter().collect(),
                class: seed.class,
            };
            // A pair of the family measured apart that it would serve rules
            // it out.
            if pairs
                .iter()
                .any(|pair| !pair.alike && range.serves(pair.ends[0].1, pair.ends[1].1, tolerance))
            {
                continue;
            }
            found.push(range);
        }
        found.sort_by(|a, b| {
            a.ends[0]
                .low
                .total_cmp(&b.ends[0].low)
                .then(a.ends[1].low.total_cmp(&b.ends[1].low))
        });
        found
    }

    /// Whether a change of the rig that reaches the family's filter falls
    /// between two of its sessions.
    fn change_between(&self, family: &Family, a: usize, b: usize) -> bool {
        let (a, b) = (&self.sessions[a], &self.sessions[b]);
        let (earlier, later) = if a.first <= b.first { (a, b) } else { (b, a) };
        self.events.iter().any(|event| {
            event.rig_uuid == family.rig_uuid
                && event.applies_to(family.filter.as_deref())
                && event.start < later.first
                && event.end > earlier.last
        })
    }

    /// A range of `family` that serves a light at `light` with flats at
    /// `flat`, within the rig's `tolerance`, whichever group it was
    /// measured in.
    pub fn alike_range(
        &self,
        family: usize,
        light: f64,
        flat: f64,
        tolerance: f64,
    ) -> Option<&AlikeRange> {
        self.families[family]
            .alike_ranges
            .iter()
            .find(|range| range.serves(light, flat, tolerance))
    }

    /// A range of `family` that serves a light at `light` with the flats
    /// of `session`: one measured in the session's carrying class.
    pub fn alike_range_for(
        &self,
        family: usize,
        session: usize,
        light: f64,
        tolerance: f64,
    ) -> Option<&AlikeRange> {
        let family_ref = &self.families[family];
        let flat = self.angle(session)?;
        let group = family_ref
            .groups
            .iter()
            .position(|group| group.contains(&session))?;
        let class = family_ref.carry_class.get(group).copied()?;
        family_ref
            .alike_ranges
            .iter()
            .filter(|range| range.class == class)
            .find(|range| range.serves(light, flat, tolerance))
    }

    /// Whether two sessions are of one rig and family key but different
    /// rotation clusters, each with its angle recorded: a pair at two
    /// angles, whether or not they measured alike.
    fn across_clusters(&self, a: usize, b: usize) -> bool {
        let (sa, sb) = (&self.sessions[a].row, &self.sessions[b].row);
        sa.rig_uuid == sb.rig_uuid
            && sa.family_key == sb.family_key
            && self.cluster_of[a] != self.cluster_of[b]
            && self.angle(a).is_some()
            && self.angle(b).is_some()
    }

    /// Pairs the measurement pass should compare: within each family, every
    /// pair within twice the window and every pair of neighbours; across
    /// rotation clusters of one filter, pairs shot within one window, which
    /// give the rig's angle term and join clusters that measure alike; and
    /// every session against its neighbours in time in every other rotation
    /// cluster of its family key, so each has a measured neighbourhood
    /// whatever its header says, and one whose flats record no angle can be
    /// placed. Earlier first.
    pub fn pairs_to_compare(&self) -> Vec<(usize, usize)> {
        let mut pairs = BTreeSet::new();
        for family in &self.families {
            let Some(limits) = self.limits(&family.rig_uuid) else {
                continue;
            };
            for (position, a) in family.sessions.iter().enumerate() {
                for (other, b) in family.sessions.iter().enumerate().skip(position + 1) {
                    // Next-but-one too: a night unlike both of its
                    // neighbours is only an outlier when they are alike.
                    if self.compared_pair(*a, *b, other <= position + 2, &limits) {
                        pairs.insert((*a, *b));
                    }
                }
            }
        }
        for i in 0..self.sessions.len() {
            for j in 0..self.sessions.len() {
                let (a, b) = (&self.sessions[i], &self.sessions[j]);
                if i == j
                    || a.first > b.first
                    || (a.first == b.first && i > j)
                    || !self.across_clusters(i, j)
                {
                    continue;
                }
                let Some(limits) = self.limits(&a.row.rig_uuid) else {
                    continue;
                };
                if b.first - a.first <= limits.window {
                    pairs.insert((i, j));
                }
            }
        }
        let earlier_first = |a: usize, b: usize| {
            if (self.sessions[a].first, a) <= (self.sessions[b].first, b) {
                (a, b)
            } else {
                (b, a)
            }
        };
        for session in 0..self.sessions.len() {
            let row = &self.sessions[session].row;
            for (id, (rig, family_key, members)) in self.clusters.iter().enumerate() {
                if *rig != row.rig_uuid
                    || *family_key != row.family_key
                    || self.cluster_of[session] == id
                {
                    continue;
                }
                for neighbour in self.neighbours_in(session, members) {
                    pairs.insert(earlier_first(session, neighbour));
                }
            }
        }
        pairs.into_iter().collect()
    }

    /// The angle a flat that records none was measured to share, from the
    /// rotation cluster its session joined.
    pub fn inferred_rotation(&self, frame_uuid: &str) -> Option<f64> {
        self.sessions[self.session_of_frame(frame_uuid)?].inferred_rotation
    }

    /// Whether every session in `sessions` was measured and each pair was
    /// compared and found alike: then mixing them needs no warning.
    pub fn measured_alike(&self, sessions: &[usize]) -> bool {
        sessions.iter().enumerate().all(|(i, a)| {
            sessions.iter().skip(i + 1).all(|b| {
                let rig = &self.sessions[*a].row.rig_uuid;
                self.limits(rig)
                    .and_then(|limits| self.pair_distance(*a, *b, &limits))
                    .is_some_and(|d| d <= 1.0)
            })
        })
    }

    /// The imaging session of `rig` a light shot at `at` belongs to.
    /// Another rig's session, shot the same night, says nothing of it.
    pub fn imaging_session(&self, rig: &str, at: i64) -> Option<&ImagingSession> {
        self.imaging.iter().find(|session| {
            session.start <= at && at <= session.end && session.rigs.iter().any(|r| r == rig)
        })
    }

    /// Where a light of the event's rig shot at `at` stands against it.
    pub fn place(&self, event: &Event, at: i64) -> Placement {
        if let Some(side) = event.side_of(at) {
            return Placement::Time { side };
        }
        let session = self.imaging_session(&event.rig_uuid, at);
        if let Some(session) = session {
            // A change does not land inside one continuous session.
            if session.start <= event.start {
                return Placement::Session {
                    side: Side::Before,
                    start: session.start,
                    end: session.end,
                };
            }
            if session.end >= event.end {
                return Placement::Session {
                    side: Side::After,
                    start: session.start,
                    end: session.end,
                };
            }
            if let Some(evidence) = self
                .evidence
                .get(&(session.key.clone(), self.evidence_key(event, session)))
            {
                let decided: Vec<_> = evidence
                    .fits
                    .iter()
                    .filter(|fit| fit.verdict != Verdict::Undecided)
                    .collect();
                let side = match evidence.verdict {
                    Verdict::Before => Some(Side::Before),
                    Verdict::After => Some(Side::After),
                    Verdict::Undecided => None,
                };
                if let Some(side) = side {
                    let mut targets: BTreeSet<String> = BTreeSet::new();
                    let mut filters: BTreeSet<String> = BTreeSet::new();
                    for fit in &decided {
                        targets.extend(fit.targets.iter().cloned());
                        filters.insert(fit.filter.clone());
                    }
                    return Placement::Pixels {
                        side,
                        targets: targets.into_iter().collect(),
                        filters: filters.into_iter().collect(),
                        lights: decided.iter().map(|fit| fit.lights).sum(),
                    };
                }
                return Placement::Undetermined {
                    session: Some((session.start, session.end)),
                    pixels: Some("its lights could not tell".into()),
                };
            }
        }
        Placement::Undetermined {
            session: session.map(|s| (s.start, s.end)),
            pixels: None,
        }
    }

    /// Events of a rig that apply to a filter, earliest first.
    pub fn events_for<'a>(
        &'a self,
        rig: &'a str,
        filter: Option<&'a str>,
    ) -> impl Iterator<Item = &'a Event> + 'a {
        self.events
            .iter()
            .filter(move |event| event.rig_uuid == rig && event.applies_to(filter))
    }

    /// The before and after sessions a correction map for `filter` is
    /// measured from: the event's member in that filter when it has one,
    /// else (for a camera-side change) its strongest member. Only a member
    /// that spans this change alone serves: one whose window another change
    /// of the rig also falls in would carry that change too. `None` when no
    /// member does.
    pub fn map_source(&self, event_index: usize, filter: Option<&str>) -> Option<(String, String)> {
        self.map_source_for(event_index, filter, None)
    }

    /// [`Self::map_source`] for the flat `flat_uuid` and the flats of its
    /// family: a member whose window holds another split of that family
    /// would carry that split's change across too, so it does not serve.
    pub fn map_source_for(
        &self,
        event_index: usize,
        filter: Option<&str>,
        flat_uuid: Option<&str>,
    ) -> Option<(String, String)> {
        let flats = flat_uuid.and_then(|uuid| self.session_of_frame(uuid));
        let chosen = self.map_member_for(event_index, filter, flats)?;
        let boundary = &self.boundaries[chosen];
        Some((
            self.sessions[boundary.before].row.response_key.clone(),
            self.sessions[boundary.after].row.response_key.clone(),
        ))
    }

    /// The member boundary [`Self::map_source`] measures from.
    pub fn map_member(&self, event_index: usize, filter: Option<&str>) -> Option<usize> {
        self.map_member_for(event_index, filter, None)
    }

    /// [`Self::map_member`] for flats of the session `flats`.
    pub fn map_member_for(
        &self,
        event_index: usize,
        filter: Option<&str>,
        flats: Option<usize>,
    ) -> Option<usize> {
        let event = self.events.get(event_index)?;
        let filter = filter.map(canonical_filter);
        let alone: Vec<usize> = event
            .boundaries
            .iter()
            .copied()
            .filter(|b| self.spans_one_change(event_index, *b))
            .filter(|b| flats.is_none_or(|flats| self.state_matches(event_index, *b, flats)))
            .collect();
        alone
            .iter()
            .copied()
            .find(|b| {
                self.families[self.boundaries[*b].family]
                    .filter
                    .as_deref()
                    .map(canonical_filter)
                    == filter
            })
            .or_else(|| {
                event.camera_side.then(|| {
                    alone.iter().copied().max_by(|a, b| {
                        let d = |i: usize| self.boundaries[i].dust.unwrap_or(0.0);
                        d(*a).total_cmp(&d(*b))
                    })
                })?
            })
    }

    /// Whether the flats of session `flats` are in the state the member's
    /// session on their side of the change shows: no other change of the
    /// rig that reaches the member's filter falls between the two. Else
    /// the map would carry the flats from a state they are not in.
    fn state_matches(&self, event_index: usize, boundary: usize, flats: usize) -> bool {
        let event = &self.events[event_index];
        let member = &self.boundaries[boundary];
        let flats_session = &self.sessions[flats];
        let ours = if flats_session.last <= event.start {
            &self.sessions[member.before]
        } else {
            &self.sessions[member.after]
        };
        let (low, high) = (
            flats_session.first.min(ours.first),
            flats_session.last.max(ours.last),
        );
        let family = &self.families[member.family];
        !self.events.iter().enumerate().any(|(index, other)| {
            index != event_index
                && other.rig_uuid == family.rig_uuid
                && other.applies_to(family.filter.as_deref())
                && other.start < high
                && other.end > low
        })
    }

    /// Whether no other change of the rig that reaches the boundary's
    /// filter falls in its window: then its two sessions differ by this
    /// change alone.
    fn spans_one_change(&self, event_index: usize, boundary: usize) -> bool {
        let member = &self.boundaries[boundary];
        let family = &self.families[member.family];
        !self.events.iter().enumerate().any(|(index, other)| {
            index != event_index
                && other.rig_uuid == family.rig_uuid
                && other.applies_to(family.filter.as_deref())
                && other.start < member.end
                && other.end > member.start
        })
    }

    /// The flat session a light-evidence fit in `filter` divides by: the
    /// nearest in time to `at` with a known side of the change, any angle.
    pub fn evidence_reference(&self, event: &Event, filter: &str, at: i64) -> Option<usize> {
        let filter = canonical_filter(filter);
        (0..self.sessions.len())
            .filter(|i| {
                let s = &self.sessions[*i];
                s.row.rig_uuid == event.rig_uuid
                    && s.row.filter.as_deref().map(canonical_filter).as_ref() == Some(&filter)
                    && event.side_of(s.first).is_some()
                    && event.side_of(s.last).is_some()
            })
            .min_by_key(|i| ((self.sessions[*i].first - at).abs(), *i))
    }

    /// The key an imaging session's evidence about a change is kept under:
    /// the change, and for each filter of its rig the map's two sessions
    /// and the reference flat the fit would use. A map or reference that
    /// changes asks the lights again.
    pub fn evidence_key(&self, event: &Event, session: &ImagingSession) -> String {
        use sha2::Digest;
        let Some(index) = self.events.iter().position(|e| std::ptr::eq(e, event)) else {
            return event.key.clone();
        };
        let filters: BTreeSet<String> = self
            .families
            .iter()
            .filter(|family| family.rig_uuid == event.rig_uuid)
            .filter_map(|family| family.filter.as_deref().map(canonical_filter))
            .collect();
        let response = |session: Option<usize>| {
            session.map_or("-", |i| self.sessions[i].row.response_key.as_str())
        };
        let mut text = event.key.clone();
        for filter in &filters {
            let member = self
                .map_member(index, Some(filter))
                .map(|b| &self.boundaries[b]);
            text.push_str(&format!(
                "\u{1f}{filter}:{}:{}:{}",
                response(member.map(|b| b.before)),
                response(member.map(|b| b.after)),
                response(self.evidence_reference(event, filter, session.start))
            ));
        }
        let digest = sha2::Sha256::digest(text.as_bytes());
        digest.iter().take(12).map(|b| format!("{b:02x}")).collect()
    }

    /// Imaging sessions inside an event's window, with their evidence keys.
    pub fn sessions_in_windows(&self) -> Vec<(usize, usize, String)> {
        let mut out = Vec::new();
        for (event_index, event) in self.events.iter().enumerate() {
            let measured = self.limits(&event.rig_uuid).is_some_and(|l| l.measured);
            if !measured {
                continue;
            }
            for (session_index, session) in self.imaging.iter().enumerate() {
                if session.rigs.contains(&event.rig_uuid)
                    && session.start > event.start
                    && session.end < event.end
                {
                    out.push((
                        event_index,
                        session_index,
                        self.evidence_key(event, session),
                    ));
                }
            }
        }
        out
    }

    /// Imaging sessions inside an event's window with no evidence yet: what
    /// the light-evidence pass should read.
    pub fn sessions_needing_evidence(&self) -> Vec<(usize, usize)> {
        self.sessions_in_windows()
            .into_iter()
            .filter(|(_, session, key)| {
                !self
                    .evidence
                    .contains_key(&(self.imaging[*session].key.clone(), key.clone()))
            })
            .map(|(event, session, _)| (event, session))
            .collect()
    }
}

pub fn event_key(rig: &str, start: i64, end: i64) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(format!("{rig}\u{1f}{start}\u{1f}{end}").as_bytes());
    digest.iter().take(12).map(|b| format!("{b:02x}")).collect()
}

/// Continuous runs of lights per profile: `(id, profile, captured_at)`,
/// each with the rigs `rigs_of` its profile.
pub fn imaging_sessions(
    mut lights: Vec<(i64, Option<String>, i64)>,
    rigs_of: impl Fn(Option<&str>) -> Vec<String>,
) -> Vec<ImagingSession> {
    lights.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)));
    let mut out: Vec<ImagingSession> = Vec::new();
    for (id, profile, at) in lights {
        match out.last_mut() {
            Some(session)
                if session.profile == profile && at - session.end <= IMAGING_GAP_SECONDS =>
            {
                session.end = at;
                session.lights.push(id);
            }
            _ => out.push(ImagingSession {
                key: format!("{}:{id}", profile.as_deref().unwrap_or("-")),
                rigs: rigs_of(profile.as_deref()),
                profile,
                start: at,
                end: at,
                lights: vec![id],
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::flat_groups::metric::SpotKind;

    fn row(key: &str, family: &str, filter: &str, rotation: Option<f64>, at: i64) -> ResponseRow {
        ResponseRow {
            response_key: key.into(),
            session_key: format!("s-{key}"),
            rig_uuid: "rig".into(),
            family_key: family.into(),
            night: String::new(),
            filter: Some(filter.into()),
            rotation,
            frame_uuids: vec![
                format!("{key}-f1"),
                format!("{key}-f2"),
                format!("{key}-f3"),
            ],
            frames: 3,
            captured_first: Some(at),
            captured_last: Some(at + 600),
            level_adu: Some(30_000.0),
            pedestal_adu: Some(500.0),
            noise: Some(0.0003),
            width: Some(100),
            height: Some(80),
            bin: Some(10),
            algorithm_version: 1,
            error: None,
            unread: Vec::new(),
        }
    }

    fn spot(x: f32, darker: bool) -> Spot {
        Spot {
            kind: if darker {
                SpotKind::New
            } else {
                SpotKind::Gone
            },
            darker,
            x,
            y: 3500.0,
            radius: 450.0,
            area: 600_000.0,
            peak: 0.016,
        }
    }

    fn cmp(a: &str, b: &str, dust: f64, broad: f64, spots: Vec<Spot>) -> ComparisonRow {
        ComparisonRow {
            earlier_key: a.into(),
            later_key: b.into(),
            broad,
            dust,
            sigma: 0.0002,
            spots,
            broad_x: 0.5,
            broad_y: 0.5,
        }
    }

    const DAY: i64 = 86_400;

    /// The default limits: 1% dust, 2% pattern, a 24-hour session.
    fn rules() -> Limits {
        Limits {
            measured: true,
            dust: 0.01,
            broad: 0.02,
            window: 86_400,
            rotation: 2.0,
        }
    }

    fn new_dust(x: f32) -> Spot {
        Spot { x, ..spot(x, true) }
    }

    fn alike(a: &str, b: &str) -> ComparisonRow {
        cmp(a, b, 0.002, 0.002, vec![])
    }

    fn spans(book: &Book) -> Vec<(String, usize, usize)> {
        book.families
            .iter()
            .flat_map(|family| family.alike_ranges.iter())
            .map(|range| (range.label(), range.pairs, range.nights))
            .collect()
    }

    /// 100° and 103.6°, two sessions at each end on four nights.
    fn two_by_two(more: Vec<ComparisonRow>) -> Book {
        let mut comparisons = vec![alike("a", "b"), alike("c", "d")];
        comparisons.extend(more);
        nights_book(
            &[
                ("a", 100.0, 0),
                ("b", 103.6, 1),
                ("c", 100.0, 2),
                ("d", 103.6, 3),
            ],
            comparisons,
        )
    }

    #[test]
    fn a_familys_alike_pairs_grant_the_angles_they_compare() {
        let book = two_by_two(vec![]);
        assert_eq!(book.families.len(), 1, "{:?}", book.families);
        assert_eq!(spans(&book), vec![("100.0° and 103.6°".to_string(), 2, 4)]);
        let range = &book.families[0].alike_ranges[0];
        assert_eq!((range.sessions, range.ends_nights), ([2, 2], [2, 2]));
        // A light at 100° takes 103.6° flats, and the tolerance counts once:
        // 1.5° outside an end is fine, but not 5.6° + more apart.
        assert!(book.alike_range(0, 100.0, 103.6, 2.0).is_some());
        assert!(book.alike_range(0, 98.5, 103.6, 2.0).is_some());
        assert!(book.alike_range(0, 97.5, 103.6, 2.0).is_none());
        assert!(book.alike_range(0, 99.0, 105.0, 2.0).is_none());
    }

    #[test]
    fn a_pair_apart_that_a_range_would_serve_rules_it_out() {
        // A (100°) and D (103.6°) measured apart, three days apart and not
        // neighbours: the range would hand a light at 100° the 103.6° flats.
        let book = two_by_two(vec![cmp("a", "d", 0.03, 0.002, vec![])]);
        assert_eq!(book.families.len(), 1, "{:?}", book.families);
        assert!(spans(&book).is_empty(), "{:?}", spans(&book));
    }

    #[test]
    fn overlapping_ranges_stand_side_by_side_and_never_bridge() {
        // 100°↔103.6° and 103.6°↔107.2°, each two by two; 100° and 107.2°
        // measured apart. Both ranges stand, sharing an end; neither hands
        // a light at 100° the 107.2° flats.
        let book = nights_book(
            &[
                ("a", 100.0, 0),
                ("b", 103.6, 1),
                ("c", 100.0, 2),
                ("d", 103.6, 3),
                ("e", 107.2, 4),
                ("f", 107.2, 5),
            ],
            vec![
                alike("a", "b"),
                alike("c", "d"),
                alike("b", "e"),
                alike("d", "f"),
                cmp("a", "f", 0.03, 0.002, vec![]),
            ],
        );
        assert_eq!(book.families.len(), 1, "{:?}", book.families);
        assert_eq!(
            spans(&book),
            vec![
                ("100.0° and 103.6°".to_string(), 2, 4),
                ("103.6° and 107.2°".to_string(), 2, 4)
            ]
        );
        assert!(book.alike_range(0, 100.0, 107.2, 2.0).is_none());
        assert!(book.alike_range(0, 107.2, 103.6, 2.0).is_some());
    }

    #[test]
    fn too_few_sessions_or_nights_at_an_end_grant_nothing() {
        // One pair.
        let book = nights_book(&[("a", 100.0, 0), ("b", 103.6, 1)], vec![alike("a", "b")]);
        assert!(spans(&book).is_empty());
        // Two pairs sharing the 103.6° session.
        let book = nights_book(
            &[("a", 100.0, 0), ("b", 103.6, 1), ("c", 100.0, 2)],
            vec![alike("a", "b"), alike("b", "c")],
        );
        assert!(spans(&book).is_empty(), "{:?}", spans(&book));
        // Two sessions at each end, but each end's on one night.
        let rows = [
            ("a", 100.0, 0),
            ("b", 103.6, 0),
            ("c", 100.0, 0),
            ("d", 103.6, 0),
        ]
        .iter()
        .enumerate()
        .map(|(i, (key, rotation, _))| ResponseRow {
            night: "2026-09-01".into(),
            ..row(key, "HA", "Ha", Some(*rotation), i as i64 * 3_600)
        })
        .collect();
        let book = Book::build(
            rows,
            vec![alike("a", "b"), alike("c", "d")],
            vec![],
            vec![],
            |_| rules(),
        );
        assert!(spans(&book).is_empty(), "{:?}", spans(&book));
        // Two sessions on two nights at each end.
        assert_eq!(spans(&two_by_two(vec![])).len(), 1);
    }

    #[test]
    fn a_wide_range_serves_only_along_its_own_arc() {
        // 100° and 160° alike, two by two: the arc between them, but never
        // the other way round, and never further than the arc.
        let book = nights_book(
            &[
                ("a", 100.0, 0),
                ("b", 160.0, 1),
                ("c", 100.0, 2),
                ("d", 160.0, 3),
            ],
            vec![alike("a", "b"), alike("c", "d")],
        );
        assert_eq!(spans(&book), vec![("100.0° and 160.0°".to_string(), 2, 4)]);
        assert!(book.alike_range(0, 130.0, 160.0, 2.0).is_some());
        assert!(book.alike_range(0, 40.0, 100.0, 2.0).is_none());
        assert!(book.alike_range(0, 100.0, 220.0, 2.0).is_none());
    }

    #[test]
    fn a_range_wraps_round_zero() {
        let book = nights_book(
            &[
                ("a", 358.5, 0),
                ("b", 2.0, 1),
                ("c", 358.5, 2),
                ("d", 2.0, 3),
            ],
            vec![alike("a", "b"), alike("c", "d")],
        );
        assert_eq!(book.families.len(), 1, "{:?}", book.families);
        assert_eq!(spans(&book), vec![("358.5° and 2.0°".to_string(), 2, 4)]);
        assert!(book.alike_range(0, 357.5, 2.0, 2.0).is_some());
        assert!(book.alike_range(0, 2.0, 358.5, 2.0).is_some());
        assert!(book.alike_range(0, 180.0, 2.0, 2.0).is_none());
    }

    /// Hα at 100° and 103.6° on days 0–1 and again on days 10–11, with new
    /// dust between them seen by the OIII flats too, its pattern change
    /// `broad`: one pair on each side of the change.
    fn across_dust(broad: f64) -> Book {
        let at = |key: &str, filter: &str, rotation: f64, day: i64| ResponseRow {
            night: format!("2026-09-{:02}", day + 1),
            ..row(key, filter, filter, Some(rotation), day * DAY)
        };
        let dust = |a: &str, b: &str| cmp(a, b, 0.02, broad, vec![new_dust(900.0)]);
        Book::build(
            vec![
                at("a", "HA", 100.0, 0),
                at("b", "HA", 103.6, 1),
                at("c", "HA", 100.0, 10),
                at("d", "HA", 103.6, 11),
                at("o1", "OIII", 100.0, 0),
                at("o2", "OIII", 100.0, 10),
            ],
            vec![
                alike("a", "b"),
                alike("c", "d"),
                dust("a", "c"),
                dust("b", "d"),
                dust("b", "c"),
                dust("o1", "o2"),
            ],
            vec![],
            vec![],
            |_| rules(),
        )
    }

    #[test]
    fn a_range_carries_across_camera_side_dust_only() {
        // Camera-side dust: the pair before it and the pair after it are
        // one range, measured in two groups.
        let book = across_dust(0.002);
        let ha = book
            .families
            .iter()
            .position(|f| f.filter.as_deref() == Some("HA"))
            .unwrap();
        assert_eq!(book.events.len(), 1, "{:?}", book.events);
        assert!(book.events[0].camera_side);
        assert_eq!(book.families[ha].carry_class, vec![0, 0]);
        let ranges = &book.families[ha].alike_ranges;
        assert_eq!(ranges.len(), 1, "{ranges:?}");
        assert_eq!(ranges[0].groups.len(), 2);
        // A broad change with it (vignetting) carries nothing: one pair on
        // each side is not enough on either.
        let book = across_dust(0.03);
        let ha = book
            .families
            .iter()
            .position(|f| f.filter.as_deref() == Some("HA"))
            .unwrap();
        assert_eq!(book.families[ha].carry_class, vec![0, 1]);
        assert!(book.families[ha].alike_ranges.is_empty());
    }

    /// Ha flats at `(key, angle, day)`, each on its own night.
    fn nights_book(sessions: &[(&str, f64, i64)], comparisons: Vec<ComparisonRow>) -> Book {
        let rows = sessions
            .iter()
            .map(|(key, rotation, day)| ResponseRow {
                night: format!("2026-09-{:02}", day + 1),
                ..row(key, "HA", "Ha", Some(*rotation), day * DAY)
            })
            .collect();
        Book::build(rows, comparisons, vec![], vec![], |_| rules())
    }

    // Regression from the review of bc5d5286 (High, Q1): C925's G and R range 101.9–104.2° "(2 pairs,
    // 3 nights)" is one 104.2° session (08-24) compared with two ~102°
    // sessions. One session at one end -- a gradient of its twilight, a
    // header that changed while the rotator did not -- makes the range alone.
    #[test]
    fn review_repro_one_session_at_an_end_grants_no_range() {
        let book = nights_book(
            &[("a", 101.99, 0), ("b", 101.93, 10), ("c", 104.2, 13)],
            vec![alike("a", "c"), alike("b", "c")],
        );
        assert_eq!(book.families.len(), 1, "{:?}", book.families);
        assert!(spans(&book).is_empty(), "{:?}", spans(&book));
    }

    // Regression from the review of bc5d5286 (High, Q2): a chain of alike neighbours spans ends that
    // were never compared. 100° and 116° flats never met, yet a light at
    // 100° may take the 116° flats; with the tolerance added at each end, a
    // light at 98.1° may take flats at 117.9°.
    #[test]
    fn review_repro_a_chain_does_not_grant_ends_never_compared() {
        let book = nights_book(
            &[
                ("a", 100.0, 0),
                ("b", 104.0, 1),
                ("c", 108.0, 2),
                ("d", 112.0, 3),
                ("e", 116.0, 4),
            ],
            vec![
                alike("a", "b"),
                alike("b", "c"),
                alike("c", "d"),
                alike("d", "e"),
            ],
        );
        assert_eq!(book.families.len(), 1, "{:?}", book.families);
        assert!(
            book.alike_range(0, 100.0, 116.0, 2.0).is_none(),
            "{:?}",
            spans(&book)
        );
    }

    // Regression from the review of bc5d5286 (Medium, Q2): the tolerance counts at both ends. Two
    // ~2.3° pairs (C925 G/R shape, here with distinct sessions) let a light
    // at 99.95° take flats at 106.15°, 6.2° off, over a measured 2.3°.
    #[test]
    fn review_repro_the_tolerance_is_not_added_at_both_ends() {
        let book = nights_book(
            &[
                ("a", 101.9, 0),
                ("b", 104.2, 1),
                ("c", 101.9, 2),
                ("d", 104.2, 3),
            ],
            vec![alike("a", "b"), alike("c", "d")],
        );
        assert_eq!(spans(&book).len(), 1, "{:?}", spans(&book));
        assert!(book.alike_range(0, 99.95, 106.15, 2.0).is_none());
    }

    // Regression from the review of bc5d5286 (High, Q2): UltraCat131 Hα's 0.0–163.8° shape. Before new
    // dust, 0° and 113.9° measured alike; after it, 53.5°, 113.7° and 163.8°.
    // No pair spans the change, yet the union is one range, so a light at 0°
    // after the dust may take 53.5° flats that were never compared with any
    // 0° flat, and no apart pair can veto it (cross-group pairs are skipped).
    #[test]
    fn review_repro_a_range_is_not_stitched_across_a_change() {
        let book = nights_book(
            &[
                ("a", 0.0, 0),
                ("b", 113.9, 1),
                ("d", 113.7, 10),
                ("c", 53.5, 11),
                ("e", 163.8, 12),
                ("f", 53.5, 13),
            ],
            vec![
                alike("a", "b"),
                cmp("b", "d", 0.02, 0.002, vec![new_dust(900.0)]),
                alike("c", "d"),
                alike("d", "e"),
                alike("e", "f"),
            ],
        );
        assert_eq!(book.families.len(), 1, "{:?}", book.families);
        assert_eq!(
            book.events.len(),
            1,
            "{:?} {:?}",
            book.events,
            book.families
        );
        assert!(
            book.alike_range(0, 0.0, 53.5, 2.0).is_none(),
            "{:?}",
            spans(&book)
        );
    }

    #[test]
    fn rotation_clusters_split_only_at_wide_gaps() {
        let clusters = |angles: &[f64]| -> Vec<Vec<f64>> {
            let mut clusters: Vec<Vec<f64>> =
                rotation_clusters(angles.iter().copied().enumerate().collect(), 2.0)
                    .into_iter()
                    .map(|cluster| cluster.into_iter().map(|(_, a)| a).collect())
                    .collect();
            clusters.sort_by(|a, b| a[0].total_cmp(&b[0]));
            clusters
        };
        // C925: 104.0° and 104.22° were two clusters when each grew from
        // its lowest angle; every gap from 100.1° to 104.4° is under 2°.
        assert_eq!(
            clusters(&[104.22, 101.3, 104.0, 82.2, 103.2, 100.1, 102.2, 104.4]),
            vec![
                vec![82.2],
                vec![100.1, 101.3, 102.2, 103.2, 104.0, 104.22, 104.4]
            ]
        );
        // Around zero.
        assert_eq!(
            clusters(&[359.5, 0.8, 180.0]),
            vec![vec![180.0], vec![359.5, 0.8]]
        );
        assert_eq!(clusters(&[10.0, 11.0]), vec![vec![10.0, 11.0]]);
    }

    #[test]
    fn sessions_a_small_angle_apart_are_one_family_and_compared() {
        // C925's G flats: 104.0° on 05-19 and 103.2° on 08-10, with the
        // donut between them; 101.5° on 08-11, after the donut left.
        let responses = vec![
            row("may", "G", "G", Some(104.0), 0),
            row("aug10", "G", "G", Some(103.2), 83 * DAY),
            row("aug11", "G", "G", Some(101.5), 84 * DAY),
        ];
        let book = Book::build(responses.clone(), vec![], vec![], vec![], |_| rules());
        assert_eq!(book.families.len(), 1);
        let pairs = book.pairs_to_compare();
        assert!(pairs.contains(&(0, 1)), "{pairs:?}");
        let book = Book::build(
            responses,
            vec![
                cmp("may", "aug10", 0.021, 0.006, vec![new_dust(500.0)]),
                cmp("aug10", "aug11", 0.023, 0.001, vec![spot(3500.0, false)]),
                cmp("may", "aug11", 0.004, 0.006, vec![]),
            ],
            vec![],
            vec![],
            |_| rules(),
        );
        // The window ends at the first flat that shows the change.
        assert_eq!(book.families[0].groups.len(), 3);
        assert_eq!(book.boundaries[0].end, 83 * DAY);
        // A night unlike two nights that agree is one odd night.
        assert_eq!(book.outliers.len(), 2);
    }

    /// C925's two corner-shadow changes, May→Aug and Aug→Sep. OIII at 104°
    /// sees each apart, with a group between; R at 104° has no flats
    /// between and splits once across both; OIII at 82° sees only the
    /// second.
    fn two_changes() -> Book {
        let responses = vec![
            row("oiii104-may", "OIII", "OIII", Some(104.0), 0),
            row("oiii104-aug18", "OIII", "OIII", Some(104.2), 91 * DAY),
            row("oiii104-aug24", "OIII", "OIII", Some(104.2), 97 * DAY),
            row("oiii104-sep15", "OIII", "OIII", Some(104.3), 119 * DAY),
            row("r104-may", "R", "R", Some(104.0), 300),
            row("r104-aug25", "R", "R", Some(104.2), 98 * DAY + 400),
            row("oiii82-aug21", "OIII", "OIII", Some(82.2), 94 * DAY),
            row("oiii82-sep25", "OIII", "OIII", Some(82.2), 129 * DAY),
        ];
        let (first, second) = (new_dust(500.0), new_dust(219.0));
        let comparisons = vec![
            cmp(
                "oiii104-may",
                "oiii104-aug18",
                0.02,
                0.009,
                vec![first.clone()],
            ),
            cmp("oiii104-aug18", "oiii104-aug24", 0.002, 0.001, vec![]),
            cmp(
                "oiii104-aug24",
                "oiii104-sep15",
                0.018,
                0.004,
                vec![second.clone()],
            ),
            cmp(
                "oiii104-aug18",
                "oiii104-sep15",
                0.018,
                0.004,
                vec![second.clone()],
            ),
            // R shows the second shadow at half the depth of the first:
            // still a shadow of the second change.
            cmp(
                "r104-may",
                "r104-aug25",
                0.021,
                0.008,
                vec![
                    Spot {
                        peak: 0.021,
                        ..first
                    },
                    Spot {
                        peak: 0.009,
                        ..second.clone()
                    },
                ],
            ),
            cmp("oiii82-aug21", "oiii82-sep25", 0.011, 0.002, vec![second]),
        ];
        Book::build(responses, comparisons, vec![], vec![], |_| rules())
    }

    #[test]
    fn a_split_across_two_changes_never_joins_the_second() {
        let book = two_changes();
        let r = book
            .boundaries
            .iter()
            .position(|b| book.families[b.family].filter.as_deref() == Some("R"))
            .unwrap();
        // OIII at 104° proves both changes lie inside R's long split, and
        // R shows the shadows of both: R does not join the second change,
        // nor narrow it to the five minutes of 08-25 its flats share.
        let second = book
            .events
            .iter()
            .find(|event| event.filters.len() == 1 && event.rotations.len() == 2)
            .expect("OIII at two angles");
        assert!(!second.boundaries.contains(&r), "{:?}", book.events);
        assert_eq!((second.start, second.end), (97 * DAY + 600, 119 * DAY));
        assert!(second.camera_side);
        assert!(book.events.iter().all(|event| !event.within_flat_run));
        // R may join the first: no family shows a second change inside
        // the span of that merge.
        let first = book
            .events
            .iter()
            .find(|event| event.boundaries.contains(&r))
            .unwrap();
        assert_eq!((first.start, first.end), (900, 91 * DAY));
    }

    #[test]
    fn a_window_shorter_than_one_flat_run_is_flagged() {
        // R's flats after the change were shot five minutes after G's
        // flats before it, on one morning.
        let responses = vec![
            row("g-before", "G", "G", Some(3.2), 10 * DAY),
            row("g-after", "G", "G", Some(3.2), 20 * DAY),
            row("r-before", "R", "R", Some(3.2), 0),
            row("r-after", "R", "R", Some(3.2), 10 * DAY + 900),
        ];
        let comparisons = vec![
            cmp("g-before", "g-after", 0.016, 0.0, vec![new_dust(1460.0)]),
            cmp("r-before", "r-after", 0.016, 0.0, vec![new_dust(1460.0)]),
        ];
        let book = Book::build(responses, comparisons, vec![], vec![], |_| rules());
        assert_eq!(book.events.len(), 1);
        let event = &book.events[0];
        assert_eq!((event.start, event.end), (10 * DAY + 600, 10 * DAY + 900));
        assert!(event.within_flat_run);
    }

    /// A donut between May and August 11, seen by L and B. A corner
    /// shadow later, seen by SII from May to September and by OIII from
    /// August 21: SII's pair spans the donut as well.
    fn map_book() -> Book {
        let responses = vec![
            row("l-may", "L", "L", Some(103.2), 0),
            row("l-aug11", "L", "L", Some(101.9), 84 * DAY),
            row("b-may", "B", "B", Some(103.2), 300),
            row("b-aug11", "B", "B", Some(101.7), 84 * DAY + 300),
            row("sii-may", "SII", "SII", Some(104.0), 600),
            row("sii-sep15", "SII", "SII", Some(104.0), 119 * DAY),
            row("oiii-aug21", "OIII", "OIII", Some(101.3), 94 * DAY),
            row("oiii-sep25", "OIII", "OIII", Some(101.3), 129 * DAY),
        ];
        let (donut, corner) = (new_dust(3500.0), new_dust(219.0));
        let comparisons = vec![
            cmp("l-may", "l-aug11", 0.02, 0.001, vec![donut.clone()]),
            cmp("b-may", "b-aug11", 0.02, 0.001, vec![donut]),
            cmp("sii-may", "sii-sep15", 0.033, 0.012, vec![corner.clone()]),
            cmp("oiii-aug21", "oiii-sep25", 0.011, 0.002, vec![corner]),
        ];
        Book::build(responses, comparisons, vec![], vec![], |_| rules())
    }

    #[test]
    fn a_map_is_borrowed_only_from_a_member_that_spans_its_change_alone() {
        let book = map_book();
        assert_eq!(book.events.len(), 2, "{:?}", book.events);
        let corner_event = book
            .events
            .iter()
            .position(|event| event.filters.contains("sii"))
            .unwrap();
        assert!(book.events[corner_event].camera_side);
        // An L light: L has no member; SII is the strongest, but its pair
        // carries the donut too. OIII's pair spans the corner alone.
        let source = book.map_source(corner_event, Some("L")).unwrap();
        assert_eq!(source, ("oiii-aug21".into(), "oiii-sep25".into()));
        // SII's own lights: no member of theirs spans it alone either.
        let sii = book.map_source(corner_event, Some("SII")).unwrap();
        assert_eq!(sii.0, "oiii-aug21");
    }

    #[test]
    fn a_map_is_never_borrowed_for_flats_in_another_state() {
        // The same rig as above. OIII's pair is clean, from 08-21 to 09-25,
        // but L's flats of May lie across the donut from its 08-21 side:
        // carried with the corner's map alone, they would keep that
        // difference. L's flats of 08-11 are in OIII's 08-21 state.
        let book = map_book();
        let corner_event = book
            .events
            .iter()
            .position(|event| event.filters.contains("sii"))
            .unwrap();
        assert_eq!(
            book.map_source_for(corner_event, Some("L"), Some("l-may-f1")),
            None
        );
        assert_eq!(
            book.map_source_for(corner_event, Some("L"), Some("l-aug11-f1")),
            Some(("oiii-aug21".into(), "oiii-sep25".into()))
        );
    }

    #[test]
    fn flats_without_an_angle_join_the_cluster_they_measure_alike_with() {
        // C925's 08-18 flats record no angle. G at 101.5° the nights
        // either side, and at 81.2° three days before.
        let mut responses = vec![
            row("g101-aug16", "G", "G", Some(101.5), 0),
            row("g-none-aug18", "G", "G", None, 2 * DAY),
            row("g101-aug22", "G", "G", Some(101.5), 6 * DAY),
            row("g81-aug14", "G", "G", Some(81.2), -2 * DAY),
        ];
        responses.sort_by_key(|row| row.captured_first);
        let unmeasured = Book::build(responses.clone(), vec![], vec![], vec![], |_| rules());
        let pairs: Vec<(String, String)> = unmeasured
            .pairs_to_compare()
            .into_iter()
            .map(|(a, b)| {
                (
                    unmeasured.sessions[a].row.response_key.clone(),
                    unmeasured.sessions[b].row.response_key.clone(),
                )
            })
            .collect();
        for pair in [
            ("g101-aug16", "g-none-aug18"),
            ("g-none-aug18", "g101-aug22"),
            ("g81-aug14", "g-none-aug18"),
        ] {
            assert!(
                pairs.contains(&(pair.0.into(), pair.1.into())),
                "{pair:?} in {pairs:?}"
            );
        }
        let measured = |alike: f64| {
            Book::build(
                responses.clone(),
                vec![
                    cmp("g101-aug16", "g-none-aug18", alike, 0.002, vec![]),
                    cmp("g-none-aug18", "g101-aug22", alike, 0.002, vec![]),
                    cmp("g81-aug14", "g-none-aug18", 0.03, 0.02, vec![]),
                    cmp("g101-aug16", "g101-aug22", 0.002, 0.001, vec![]),
                ],
                vec![],
                vec![],
                |_| rules(),
            )
        };
        let book = measured(0.002);
        let family = book
            .families
            .iter()
            .find(|family| family.rotation == Some(101.5))
            .unwrap();
        assert_eq!(family.sessions.len(), 3, "the night joins 101.5°");
        assert_eq!(family.groups.len(), 1);
        assert_eq!(book.inferred_rotation("g-none-aug18-f1"), Some(101.5));
        assert!(book.families.iter().all(|family| family.rotation.is_some()));

        // Unlike either: it stays on its own, at no angle.
        let book = measured(0.03);
        assert_eq!(book.inferred_rotation("g-none-aug18-f1"), None);
        assert!(book.families.iter().any(|family| family.rotation.is_none()));
    }

    #[test]
    fn alike_nights_form_one_group_and_a_step_splits() {
        let responses = vec![
            row("a", "R", "R", Some(3.2), 0),
            row("b", "R", "R", Some(3.2), DAY),
            row("c", "R", "R", Some(3.2), 2 * DAY),
            row("d", "R", "R", Some(3.2), 6 * DAY),
        ];
        let comparisons = vec![
            cmp("a", "b", 0.001, 0.001, vec![]),
            cmp("b", "c", 0.002, 0.001, vec![]),
            cmp("a", "c", 0.002, 0.001, vec![]),
            cmp("c", "d", 0.016, 0.001, vec![spot(1460.0, true)]),
        ];
        let book = Book::build(responses, comparisons, vec![], vec![], |_| rules());
        assert_eq!(book.families.len(), 1);
        assert_eq!(book.families[0].groups.len(), 2);
        assert_eq!(book.families[0].groups[0].len(), 3);
        let boundary = &book.boundaries[0];
        assert_eq!(boundary.start, 2 * DAY + 600);
        assert_eq!(boundary.end, 6 * DAY);
        assert_eq!(book.events.len(), 1);
        assert!(!book.events[0].camera_side);
    }

    #[test]
    fn drift_cannot_chain_nights_that_differ() {
        // Neighbours at 0.6 of the limit, two apart at 1.2, all within 2W.
        let responses = vec![
            row("a", "R", "R", None, 0),
            row("b", "R", "R", None, DAY / 2),
            row("c", "R", "R", None, DAY),
        ];
        let comparisons = vec![
            cmp("a", "b", 0.006, 0.0, vec![]),
            cmp("b", "c", 0.006, 0.0, vec![]),
            cmp("a", "c", 0.012, 0.0, vec![spot(100.0, true)]),
        ];
        let book = Book::build(responses, comparisons, vec![], vec![], |_| rules());
        assert_eq!(book.families[0].groups.len(), 2);
        let boundary = &book.boundaries[0];
        assert!(boundary.drift.is_some());
    }

    #[test]
    fn one_shadow_at_two_angles_is_one_camera_side_event() {
        let responses = vec![
            row("r3-before", "R", "R", Some(3.2), 0),
            row("r3-after", "R", "R", Some(3.2), 4 * DAY),
            row("ha114-before", "HA", "HA", Some(113.8), -9 * DAY),
            row("ha114-after", "HA", "HA", Some(113.8), 11 * DAY),
        ];
        let comparisons = vec![
            cmp(
                "r3-before",
                "r3-after",
                0.016,
                0.001,
                vec![spot(1418.0, true)],
            ),
            cmp(
                "ha114-before",
                "ha114-after",
                0.0157,
                0.01,
                vec![spot(1498.0, true)],
            ),
        ];
        let book = Book::build(responses, comparisons, vec![], vec![], |_| rules());
        assert_eq!(book.events.len(), 1);
        let event = &book.events[0];
        assert!(event.camera_side);
        assert_eq!(event.start, 600);
        assert_eq!(event.end, 4 * DAY);
        assert!(event.applies_to(Some("OIII")));
    }

    #[test]
    fn one_night_unlike_its_alike_neighbours_makes_no_change() {
        // A hazy dawn on the second morning: unlike the first and the
        // third, which are alike.
        let responses = vec![
            row("a", "L", "L", Some(3.2), 0),
            row("b", "L", "L", Some(3.2), 3 * DAY),
            row("c", "L", "L", Some(3.2), 6 * DAY),
        ];
        let book = |comparisons: Vec<ComparisonRow>| {
            Book::build(responses.clone(), comparisons, vec![], vec![], |_| rules())
        };
        let hazy = book(vec![
            cmp("a", "b", 0.001, 0.03, vec![]),
            cmp("b", "c", 0.001, 0.03, vec![]),
            cmp("a", "c", 0.001, 0.002, vec![]),
        ]);
        assert_eq!(hazy.families[0].groups.len(), 3, "it still keeps to itself");
        assert_eq!(hazy.outliers.len(), 2);
        assert!(hazy.events.is_empty(), "{:?}", hazy.events);
        // Its neighbours are compared, so the outlier can be told.
        let pairs = hazy.pairs_to_compare();
        assert!(pairs.contains(&(0, 2)), "{pairs:?}");

        // The third morning stays changed: a change, not an outlier.
        let changed = book(vec![
            cmp("a", "b", 0.001, 0.03, vec![]),
            cmp("b", "c", 0.001, 0.03, vec![]),
            cmp("a", "c", 0.001, 0.03, vec![]),
        ]);
        assert!(changed.outliers.is_empty());
        assert_eq!(changed.events.len(), 2);
    }

    #[test]
    fn filter_dust_stays_with_its_filter() {
        let responses = vec![
            row("a", "L", "L", Some(3.2), 0),
            row("b", "L", "L", Some(3.2), 3 * DAY),
        ];
        let comparisons = vec![cmp("a", "b", 0.02, 0.0, vec![spot(5000.0, true)])];
        let book = Book::build(responses, comparisons, vec![], vec![], |_| rules());
        assert!(book.events[0].applies_to(Some("l")));
        assert!(!book.events[0].applies_to(Some("R")));
    }

    #[test]
    fn a_session_that_begins_before_the_window_is_before() {
        let responses = vec![
            row("a", "R", "R", Some(3.2), 0),
            row("b", "R", "R", Some(3.2), 4 * DAY),
        ];
        let comparisons = vec![cmp("a", "b", 0.016, 0.0, vec![spot(1.0, true)])];
        let imaging = imaging_sessions(
            vec![
                (1, Some("p".into()), 300),
                (2, Some("p".into()), 3_000),
                (3, Some("p".into()), 2 * DAY),
                (4, Some("p".into()), 2 * DAY + 3_600),
            ],
            |_| vec!["rig".into()],
        );
        let book = Book::build(responses, comparisons, vec![], imaging, |_| rules());
        let event = &book.events[0];
        assert_eq!(
            book.place(event, 100),
            Placement::Time { side: Side::Before }
        );
        assert!(matches!(
            book.place(event, 3_000),
            Placement::Session {
                side: Side::Before,
                ..
            }
        ));
        assert!(matches!(
            book.place(event, 2 * DAY + 3_600),
            Placement::Undetermined { .. }
        ));
    }

    #[test]
    fn another_rigs_session_places_no_light() {
        let responses = vec![
            row("a", "R", "R", Some(3.2), 0),
            row("b", "R", "R", Some(3.2), 4 * DAY),
        ];
        let comparisons = vec![cmp("a", "b", 0.016, 0.0, vec![spot(1.0, true)])];
        // Another rig images all night from before the window; this rig's
        // lone light sits inside it.
        let imaging = imaging_sessions(
            vec![
                (1, Some("another".into()), 300),
                (2, Some("another".into()), 3_000),
                (3, Some("another".into()), 2 * DAY + 7_200),
                (4, Some("this".into()), 2 * DAY + 3_600),
            ]
            .into_iter()
            .chain((0..60).map(|i| (10 + i, Some("another".into()), 3_000 + i * 3_500)))
            .collect(),
            |profile| match profile {
                Some("this") => vec!["rig".into()],
                _ => vec!["other-rig".into()],
            },
        );
        let book = Book::build(responses, comparisons, vec![], imaging, |_| rules());
        let event = &book.events[0];
        let placement = book.place(event, 2 * DAY + 3_600);
        assert!(
            matches!(placement, Placement::Undetermined { .. }),
            "{placement:?}"
        );
        // Only this rig's session asks for its lights to be read.
        let needed = book.sessions_needing_evidence();
        assert_eq!(needed.len(), 1);
        assert_eq!(book.imaging[needed[0].1].profile.as_deref(), Some("this"));
    }

    #[test]
    fn pixel_evidence_places_a_session_inside_the_window() {
        let responses = vec![
            row("a", "R", "R", Some(3.2), 0),
            row("b", "R", "R", Some(3.2), 4 * DAY),
        ];
        let comparisons = vec![cmp("a", "b", 0.016, 0.0, vec![spot(1.0, true)])];
        let imaging = imaging_sessions(
            vec![(1, Some("p".into()), DAY), (2, Some("p".into()), DAY + 600)],
            |_| vec!["rig".into()],
        );
        // The key the pass stores it under, from the book it reads with.
        let unread = Book::build(
            responses.clone(),
            comparisons.clone(),
            vec![],
            imaging.clone(),
            |_| rules(),
        );
        assert_eq!(unread.sessions_needing_evidence(), vec![(0, 0)]);
        let key = unread.evidence_key(&unread.events[0], &unread.imaging[0]);
        let evidence = vec![EvidenceRow {
            session_key: imaging[0].key.clone(),
            event_key: key,
            verdict: Verdict::Before,
            fits: vec![crate::calibration::flat_groups::store::EvidenceFit {
                filter: "L".into(),
                targets: vec!["NGC 7023".into()],
                lights: 8,
                carried: 0.02,
                spread: Some(0.05),
                verdict: Verdict::Before,
            }],
        }];
        let book = Book::build(
            responses.clone(),
            comparisons.clone(),
            evidence.clone(),
            imaging.clone(),
            |_| rules(),
        );
        assert!(book.sessions_needing_evidence().is_empty());
        let placement = book.place(&book.events[0], DAY + 300);
        assert!(
            matches!(
                placement,
                Placement::Pixels {
                    side: Side::Before,
                    ..
                }
            ),
            "{placement:?}"
        );

        // The morning before is measured again from other flats (one was
        // kept out): the window stands, but the map and the reference the
        // fit divides by are new, so the lights are asked again.
        let mut nearer = responses;
        nearer[0].response_key = "a-again".into();
        let comparisons = vec![cmp("a-again", "b", 0.016, 0.0, vec![spot(1.0, true)])];
        let book = Book::build(nearer, comparisons, evidence, imaging, |_| rules());
        assert_eq!(book.events[0].key, unread.events[0].key);
        assert_eq!(book.sessions_needing_evidence().len(), 1);
        assert!(matches!(
            book.place(&book.events[0], DAY + 300),
            Placement::Undetermined { .. }
        ));
    }

    #[test]
    fn the_angle_term_comes_from_one_morning_at_two_angles() {
        let responses = vec![
            row("r3", "R", "R", Some(3.2), 0),
            row("r114", "R", "R", Some(113.8), 300),
            row("r54", "R", "R", Some(53.5), 600),
        ];
        let comparisons = vec![
            cmp("r3", "r114", 0.0028, 0.0071, vec![]),
            cmp("r3", "r54", 0.002, 0.004, vec![]),
            cmp("r114", "r54", 0.0014, 0.0062, vec![]),
        ];
        let book = Book::build(responses, comparisons, vec![], vec![], |_| rules());
        let term = book.angle_terms["rig"];
        assert_eq!(term.pairs, 3);
        assert!((term.broad - 0.0071).abs() < 1e-9);
        assert_eq!(book.pairs_to_compare().len(), 3);
    }

    #[test]
    fn a_wide_match_tolerance_keeps_two_angles_apart() {
        // A rig whose lights may take flats up to 90° off their angle: the
        // stacker accepts it, but 0° and 90° are still two angles.
        let responses = vec![
            row("ha0", "Ha", "Ha", Some(0.0), 0),
            row("ha90", "Ha", "Ha", Some(90.0), DAY),
        ];
        let wide = Limits {
            rotation: 90.0,
            ..rules()
        };
        let book = Book::build(responses, vec![], vec![], vec![], |_| wide);
        assert_eq!(book.families.len(), 2, "0° and 90° are two angles");
    }

    #[test]
    fn a_night_with_no_angle_takes_the_angle_it_measured_alike_with() {
        // One cluster chained from 100.0° to 105.7°. The night with no
        // angle is compared with its neighbour in time, at 100.0°, and is
        // alike with it: it is at 100.0°, not at the cluster's middle.
        let responses = vec![
            row("none", "G", "G", None, -DAY),
            row("g100", "G", "G", Some(100.0), 0),
            row("g101", "G", "G", Some(101.9), 10 * DAY),
            row("g103", "G", "G", Some(103.8), 20 * DAY),
            row("g105", "G", "G", Some(105.7), 30 * DAY),
        ];
        let book = Book::build(
            responses,
            vec![cmp("none", "g100", 0.002, 0.001, vec![])],
            vec![],
            vec![],
            |_| rules(),
        );
        assert_eq!(book.inferred_rotation("none-f1"), Some(100.0));
    }

    #[test]
    fn a_night_alike_with_two_angles_keeps_no_angle() {
        // Clean optics: the night with no angle measures alike with its
        // neighbours at 10° and at 100°. That says nothing of its angle.
        let responses = vec![
            row("g10", "G", "G", Some(10.0), 0),
            row("g100", "G", "G", Some(100.0), 300),
            row("none", "G", "G", None, DAY),
        ];
        let measured = |at_100: f64| {
            Book::build(
                responses.clone(),
                vec![
                    cmp("g10", "none", 0.002, 0.001, vec![]),
                    cmp("g100", "none", at_100, 0.001, vec![]),
                ],
                vec![],
                vec![],
                |_| rules(),
            )
        };
        assert_eq!(measured(0.003).inferred_rotation("none-f1"), None);
        // Unlike 100°: then it is at 10°.
        assert_eq!(measured(0.03).inferred_rotation("none-f1"), Some(10.0));
    }

    #[test]
    fn a_split_across_both_changes_makes_no_change_of_its_own() {
        // two_changes(), but R's later flats come after the second change:
        // its one split spans both, which OIII at 104° tells apart. It
        // dates neither, and is no R change four months wide, but it does
        // show both changes in R.
        let responses = vec![
            row("oiii104-may", "OIII", "OIII", Some(104.0), 0),
            row("oiii104-aug18", "OIII", "OIII", Some(104.2), 91 * DAY),
            row("oiii104-aug24", "OIII", "OIII", Some(104.2), 97 * DAY),
            row("oiii104-sep15", "OIII", "OIII", Some(104.3), 119 * DAY),
            row("r104-may", "R", "R", Some(104.0), 300),
            row("r104-sep20", "R", "R", Some(104.2), 124 * DAY),
            row("oiii82-aug21", "OIII", "OIII", Some(82.2), 94 * DAY),
            row("oiii82-sep25", "OIII", "OIII", Some(82.2), 129 * DAY),
        ];
        let (first, second) = (new_dust(500.0), new_dust(219.0));
        let comparisons = vec![
            cmp(
                "oiii104-may",
                "oiii104-aug18",
                0.02,
                0.009,
                vec![first.clone()],
            ),
            cmp("oiii104-aug18", "oiii104-aug24", 0.002, 0.001, vec![]),
            cmp(
                "oiii104-aug24",
                "oiii104-sep15",
                0.018,
                0.004,
                vec![second.clone()],
            ),
            cmp(
                "oiii104-aug18",
                "oiii104-sep15",
                0.018,
                0.004,
                vec![second.clone()],
            ),
            cmp(
                "r104-may",
                "r104-sep20",
                0.021,
                0.008,
                vec![second.clone(), first],
            ),
            cmp("oiii82-aug21", "oiii82-sep25", 0.011, 0.002, vec![second]),
        ];
        let book = Book::build(responses, comparisons, vec![], vec![], |_| rules());
        let windows: Vec<(i64, i64, usize, bool)> = book
            .events
            .iter()
            .map(|e| {
                (
                    e.start / DAY,
                    e.end / DAY,
                    e.boundaries.len(),
                    e.camera_side,
                )
            })
            .collect();
        assert_eq!(book.events.len(), 2, "{windows:?}");
        assert_eq!(book.spanning.len(), 1);
        for event in &book.events {
            assert!(event.filters.contains("r"), "{:?}", event.filters);
            assert!(event.camera_side, "{windows:?}");
            assert!(
                event.boundaries.iter().all(|b| !book.spanning.contains(b)),
                "{windows:?}"
            );
        }
    }

    /// C925's OIII flats at 82.2° and 72.6°, shot within one flat window,
    /// measure alike: one light path, whatever the rotator recorded.
    #[test]
    fn clusters_far_apart_that_measure_alike_are_one_family() {
        let responses = vec![
            row("oiii82", "OIII", "OIII", Some(82.2), 0),
            row("oiii72", "OIII", "OIII", Some(72.6), DAY / 2),
            row("oiii104", "OIII", "OIII", Some(104.2), DAY),
        ];
        let unmeasured = Book::build(responses.clone(), vec![], vec![], vec![], |_| rules());
        assert_eq!(unmeasured.families.len(), 3, "never compared: apart");
        let book = Book::build(
            responses,
            vec![
                cmp("oiii82", "oiii72", 0.0023, 0.0037, vec![]),
                cmp("oiii82", "oiii104", 0.03, 0.02, vec![]),
                cmp("oiii72", "oiii104", 0.032, 0.02, vec![]),
            ],
            vec![],
            vec![],
            |_| rules(),
        );
        assert_eq!(book.families.len(), 2, "{:?}", book.families);
        let joined = book
            .families
            .iter()
            .position(|family| family.sessions.len() == 2)
            .unwrap();
        assert_eq!(book.angle_label(joined), "72.6° & 82.2°");
        assert_eq!(book.families[joined].groups.len(), 1);
        // Its key is its earliest session's first flat, whatever joins
        // later.
        assert_eq!(book.families[joined].key, "OIII:oiii82-f1");
        // The angle term still counts the pair at two angles.
        assert_eq!(book.angle_terms["rig"].pairs, 3);
    }

    #[test]
    fn a_pair_apart_within_one_window_keeps_two_angles_apart() {
        // Alike as neighbours days apart, apart on one morning: the angle
        // matters, and the likeness was a coincidence of time.
        let responses = vec![
            row("a10", "R", "R", Some(10.0), 0),
            row("a90", "R", "R", Some(90.0), 600),
            row("b90", "R", "R", Some(90.0), 5 * DAY),
        ];
        let book = Book::build(
            responses,
            vec![
                cmp("a10", "a90", 0.02, 0.003, vec![]),
                cmp("a10", "b90", 0.002, 0.001, vec![]),
                cmp("a90", "b90", 0.001, 0.001, vec![]),
            ],
            vec![],
            vec![],
            |_| rules(),
        );
        assert_eq!(book.families.len(), 2, "{:?}", book.families);
        // Neighbours in every other cluster are asked for, whatever the
        // header says.
        let unmeasured = Book::build(
            vec![
                row("a10", "R", "R", Some(10.0), 0),
                row("c90", "R", "R", Some(90.0), 9 * DAY),
            ],
            vec![],
            vec![],
            vec![],
            |_| rules(),
        );
        assert_eq!(unmeasured.pairs_to_compare(), vec![(0, 1)]);
    }

    #[test]
    fn a_change_in_two_response_families_of_one_filter_is_camera_side() {
        let responses = |alike: bool| {
            let comparisons = vec![
                cmp(
                    "r0-before",
                    "r0-after",
                    0.016,
                    0.001,
                    vec![new_dust(1418.0)],
                ),
                cmp(
                    "r90-before",
                    "r90-after",
                    0.016,
                    0.001,
                    vec![new_dust(1418.0)],
                ),
                // The two angles on the first morning.
                cmp(
                    "r0-before",
                    "r90-before",
                    if alike { 0.002 } else { 0.03 },
                    0.002,
                    vec![],
                ),
            ];
            Book::build(
                vec![
                    row("r0-before", "R", "R", Some(0.0), 0),
                    row("r90-before", "R", "R", Some(90.0), 600),
                    row("r0-after", "R", "R", Some(0.0), 4 * DAY),
                    row("r90-after", "R", "R", Some(90.0), 6 * DAY),
                ],
                comparisons,
                vec![],
                vec![],
                |_| rules(),
            )
        };
        let apart = responses(false);
        assert_eq!(apart.families.len(), 2);
        assert_eq!(apart.events.len(), 1, "{:?}", apart.events);
        assert!(apart.events[0].camera_side, "seen in two light paths");
        // One light path at two angles: one family, one filter, its own.
        let alike = responses(true);
        assert_eq!(alike.families.len(), 1);
        assert!(alike.events.iter().all(|event| !event.camera_side));
    }

    #[test]
    fn two_flat_sessions_allow_for_their_noise() {
        let mut responses = vec![
            row("a", "L", "L", Some(3.2), 0),
            row("b", "L", "L", Some(3.2), DAY),
        ];
        let comparison = ComparisonRow {
            sigma: 0.001,
            ..cmp("a", "b", 0.0115, 0.001, vec![])
        };
        // Three flats each: 1.15% is past the 1% limit.
        let three = Book::build(
            responses.clone(),
            vec![comparison.clone()],
            vec![],
            vec![],
            |_| rules(),
        );
        assert_eq!(three.families[0].groups.len(), 2);
        // Two flats in one of them: 1.15% less two spreads is within it.
        responses[1].frames = 2;
        responses[1].frame_uuids.truncate(2);
        let two = Book::build(responses, vec![comparison], vec![], vec![], |_| rules());
        assert_eq!(two.families[0].groups.len(), 1);
    }
}
