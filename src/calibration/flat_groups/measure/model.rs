//! Flat groups, change events and where each light stands, worked out from
//! what the catalog keeps. Nothing here reads pixels.
//!
//! - A **family** is the measured sessions of one rig, filter, sensor mode
//!   and rotation cluster: the sessions that could ever share a master.
//! - Within a family, sessions merge into **groups** in time order: the
//!   neighbouring pair with the smallest linkage first, while it stays
//!   within the limits. Linkage is the largest difference between any two
//!   sessions of the two groups that were compared, so a slow drift cannot
//!   chain nights that differ into one group.
//! - Each boundary between groups is a window of time, from the last flat
//!   before it to the first after, with the spots that changed.
//! - Boundaries of one rig whose windows overlap and whose spots coincide
//!   merge into one **change event**. Dust on the camera side turns with the
//!   camera, so the same shadow shows in every filter and at every angle;
//!   an event seen in two filters or at two angles applies to every light
//!   of the rig, one seen in a single filter only to that filter.
//! - A light stands **before** or **after** an event by its time, by the
//!   continuous imaging session it belongs to, or by its own pixels (see
//!   [`super::evidence`]); otherwise it is undetermined.

use super::metric::Spot;
use super::rows::{ComparisonRow, EvidenceRow, ResponseRow, Verdict};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};

/// Sessions of a family whose rotator angles lie within this of the
/// cluster's first angle are one rotation cluster: the coherence tolerance
/// one master's flats are held to.
pub const ROTATION_CLUSTER_DEG: f64 = 2.0;
/// Lights of one profile closer than this in time are one continuous
/// imaging session. A night's pauses are shorter; a day is longer.
pub const IMAGING_GAP_SECONDS: i64 = 3 * 3600;

/// The limits one rig's sessions are held to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub measured: bool,
    /// Fractions of the background.
    pub dust: f64,
    pub broad: f64,
    /// The flat session window, in seconds: a master spans at most twice it.
    pub window: i64,
}

impl Limits {
    /// How far two sessions are apart against these limits: alike at 1 or
    /// less.
    pub fn distance(&self, comparison: &ComparisonRow) -> f64 {
        (comparison.dust / self.dust.max(1e-9)).max(comparison.broad / self.broad.max(1e-9))
    }
}

/// One measured flat session.
#[derive(Debug, Clone, Serialize)]
pub struct Session {
    #[serde(flatten)]
    pub row: ResponseRow,
    pub first: i64,
    pub last: i64,
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
    pub rotations: Vec<Option<f64>>,
    /// Seen in two filters or at two angles: on the camera side, so it
    /// applies to every filter of the rig.
    pub camera_side: bool,
    pub dust: f64,
    pub broad: f64,
    /// The spots of the member with the most dust.
    pub spots: Vec<Spot>,
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

/// A family of sessions: one rig, filter, sensor mode and rotation cluster.
#[derive(Debug, Clone, Serialize)]
pub struct Family {
    pub key: String,
    pub rig_uuid: String,
    pub filter: Option<String>,
    /// The cluster's first angle; absent for a rig that records none.
    pub rotation: Option<f64>,
    /// Session indices in time order.
    pub sessions: Vec<usize>,
    /// Session indices per group, in time order.
    pub groups: Vec<Vec<usize>>,
    /// Indices into the book's boundaries, between consecutive groups.
    pub boundaries: Vec<usize>,
}

/// A continuous run of lights of one profile.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImagingSession {
    pub key: String,
    pub profile: Option<String>,
    pub start: i64,
    pub end: i64,
    /// `acquiredimage` ids, in time order.
    pub lights: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Before,
    After,
}

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
    pub angle_terms: HashMap<String, AngleTerm>,
    pub imaging: Vec<ImagingSession>,
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
            book.sessions.push(Session { row, first, last });
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
        book.build_events();
        book.build_angle_terms();
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
        for key in keys {
            let mut members = by_key.remove(&key).unwrap_or_default();
            // Rotation clusters: by angle, each starting at its first angle.
            members.sort_by(|a, b| {
                let (ra, rb) = (
                    self.sessions[*a].row.rotation,
                    self.sessions[*b].row.rotation,
                );
                ra.partial_cmp(&rb)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(self.sessions[*a].first.cmp(&self.sessions[*b].first))
            });
            let mut clusters: Vec<(Option<f64>, Vec<usize>)> = Vec::new();
            for index in members {
                let rotation = self.sessions[index].row.rotation;
                let joins = |cluster: &(Option<f64>, Vec<usize>)| match (cluster.0, rotation) {
                    (None, None) => true,
                    (Some(start), Some(angle)) => {
                        rotation_gap(start, angle) <= ROTATION_CLUSTER_DEG
                    }
                    _ => false,
                };
                match clusters.iter_mut().find(|cluster| joins(cluster)) {
                    Some(cluster) => cluster.1.push(index),
                    None => clusters.push((rotation, vec![index])),
                }
            }
            for (rotation, mut sessions) in clusters {
                sessions.sort_by_key(|index| (self.sessions[*index].first, *index));
                let filter = self.sessions[sessions[0]].row.filter.clone();
                self.families.push(Family {
                    key: format!(
                        "{}:{}",
                        key.1,
                        rotation.map_or_else(|| "none".to_string(), |r| format!("{r:.1}"))
                    ),
                    rig_uuid: key.0.clone(),
                    filter,
                    rotation,
                    sessions,
                    groups: Vec::new(),
                    boundaries: Vec::new(),
                });
            }
        }
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
                if let Some(comparison) = self.comparison(*a, *b) {
                    let distance = limits.distance(comparison);
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
                let before = *pair[0].last().expect("groups are not empty");
                let after = pair[1][0];
                let neighbour = self.comparison(before, after);
                let distance = neighbour.map(|c| limits.distance(c));
                let drift = if distance.is_none_or(|d| d <= 1.0) {
                    // The neighbours agree: say which pair across the
                    // boundary did not.
                    let mut worst: Option<(usize, usize, f64)> = None;
                    for a in &pair[0] {
                        for b in &pair[1] {
                            if let Some(c) = self.comparison(*a, *b) {
                                let d = limits.distance(c);
                                if worst.is_none_or(|(_, _, w)| d > w) {
                                    worst = Some((*a, *b, d));
                                }
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
                    start: self.sessions[before].last,
                    end: self.sessions[after].first,
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
        a.spots.iter().any(|sa| {
            b.spots.iter().any(|sb| {
                sa.darker == sb.darker && {
                    let distance = ((sa.x - sb.x).powi(2) + (sa.y - sb.y).powi(2)).sqrt();
                    distance <= sa.radius.max(sb.radius)
                }
            })
        })
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
        for i in 0..self.boundaries.len() {
            for j in (i + 1)..self.boundaries.len() {
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
                parent[rj] = ri;
                windows[ri] = (start, end);
            }
        }
        let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..self.boundaries.len() {
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
            for b in &boundaries {
                let rotation = self.families[self.boundaries[*b].family].rotation;
                let known = rotations.iter().any(|r| match (r, rotation) {
                    (None, None) => true,
                    (Some(x), Some(y)) => rotation_gap(*x, y) <= ROTATION_CLUSTER_DEG,
                    _ => false,
                });
                if !known {
                    rotations.push(rotation);
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
                camera_side: filters.len() >= 2 || rotations.len() >= 2,
                filters,
                rotations,
                dust,
                broad,
                spots: self.boundaries[strongest].spots.clone(),
                boundaries,
            });
        }
    }

    fn build_angle_terms(&mut self) {
        for i in 0..self.sessions.len() {
            for j in (i + 1)..self.sessions.len() {
                let (a, b) = (&self.sessions[i], &self.sessions[j]);
                if a.row.rig_uuid != b.row.rig_uuid || a.row.family_key != b.row.family_key {
                    continue;
                }
                let (Some(ra), Some(rb)) = (a.row.rotation, b.row.rotation) else {
                    continue;
                };
                if rotation_gap(ra, rb) <= ROTATION_CLUSTER_DEG {
                    continue;
                }
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

    /// Pairs the measurement pass should compare: within each family, every
    /// pair within twice the window and every pair of neighbours; and across
    /// rotation clusters of one filter, pairs shot within one window, which
    /// give the rig's angle term. Earlier first.
    pub fn pairs_to_compare(&self) -> Vec<(usize, usize)> {
        let mut pairs = BTreeSet::new();
        for family in &self.families {
            let Some(limits) = self.limits(&family.rig_uuid) else {
                continue;
            };
            for (position, a) in family.sessions.iter().enumerate() {
                for (other, b) in family.sessions.iter().enumerate().skip(position + 1) {
                    if self.compared_pair(*a, *b, other == position + 1, &limits) {
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
                    || a.row.rig_uuid != b.row.rig_uuid
                    || a.row.family_key != b.row.family_key
                {
                    continue;
                }
                let (Some(ra), Some(rb)) = (a.row.rotation, b.row.rotation) else {
                    continue;
                };
                let Some(limits) = self.limits(&a.row.rig_uuid) else {
                    continue;
                };
                if rotation_gap(ra, rb) > ROTATION_CLUSTER_DEG && b.first - a.first <= limits.window
                {
                    pairs.insert((i, j));
                }
            }
        }
        pairs.into_iter().collect()
    }

    /// Whether every session in `sessions` was measured and each pair was
    /// compared and found alike: then mixing them needs no warning.
    pub fn measured_alike(&self, sessions: &[usize]) -> bool {
        sessions.iter().enumerate().all(|(i, a)| {
            sessions.iter().skip(i + 1).all(|b| {
                let rig = &self.sessions[*a].row.rig_uuid;
                match (self.comparison(*a, *b), self.limits(rig)) {
                    (Some(c), Some(limits)) => limits.distance(c) <= 1.0,
                    _ => false,
                }
            })
        })
    }

    /// The imaging session a light shot at `at` belongs to.
    pub fn imaging_session(&self, at: i64) -> Option<&ImagingSession> {
        self.imaging
            .iter()
            .find(|session| session.start <= at && at <= session.end)
    }

    /// Where a light shot at `at` stands against an event.
    pub fn place(&self, event: &Event, at: i64) -> Placement {
        if let Some(side) = event.side_of(at) {
            return Placement::Time { side };
        }
        let session = self.imaging_session(at);
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
            if let Some(evidence) = self.evidence.get(&(session.key.clone(), event.key.clone())) {
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
    /// else (for a camera-side change) its strongest member.
    pub fn map_source(&self, event_index: usize, filter: Option<&str>) -> Option<(String, String)> {
        let event = self.events.get(event_index)?;
        let filter = filter.map(canonical_filter);
        let chosen = event
            .boundaries
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
                    event.boundaries.iter().copied().max_by(|a, b| {
                        let d = |i: usize| self.boundaries[i].dust.unwrap_or(0.0);
                        d(*a).total_cmp(&d(*b))
                    })
                })?
            })?;
        let boundary = &self.boundaries[chosen];
        Some((
            self.sessions[boundary.before].row.response_key.clone(),
            self.sessions[boundary.after].row.response_key.clone(),
        ))
    }

    /// Imaging sessions inside an event's window with no evidence yet: what
    /// the light-evidence pass should read.
    pub fn sessions_needing_evidence(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (event_index, event) in self.events.iter().enumerate() {
            let measured = self.limits(&event.rig_uuid).is_some_and(|l| l.measured);
            if !measured {
                continue;
            }
            for (session_index, session) in self.imaging.iter().enumerate() {
                if session.start > event.start
                    && session.end < event.end
                    && !self
                        .evidence
                        .contains_key(&(session.key.clone(), event.key.clone()))
                {
                    out.push((event_index, session_index));
                }
            }
        }
        out
    }
}

pub fn event_key(rig: &str, start: i64, end: i64) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(format!("{rig}\u{1f}{start}\u{1f}{end}").as_bytes());
    digest.iter().take(12).map(|b| format!("{b:02x}")).collect()
}

/// Continuous runs of lights per profile: `(id, profile, captured_at)`.
pub fn imaging_sessions(mut lights: Vec<(i64, Option<String>, i64)>) -> Vec<ImagingSession> {
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
        }
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
        let imaging = imaging_sessions(vec![
            (1, Some("p".into()), 300),
            (2, Some("p".into()), 3_000),
            (3, Some("p".into()), 2 * DAY),
            (4, Some("p".into()), 2 * DAY + 3_600),
        ]);
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
    fn pixel_evidence_places_a_session_inside_the_window() {
        let responses = vec![
            row("a", "R", "R", Some(3.2), 0),
            row("b", "R", "R", Some(3.2), 4 * DAY),
        ];
        let comparisons = vec![cmp("a", "b", 0.016, 0.0, vec![spot(1.0, true)])];
        let imaging = imaging_sessions(vec![
            (1, Some("p".into()), DAY),
            (2, Some("p".into()), DAY + 600),
        ]);
        let key = event_key("rig", 600, 4 * DAY);
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
        let book = Book::build(responses, comparisons, evidence, imaging, |_| rules());
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
}
