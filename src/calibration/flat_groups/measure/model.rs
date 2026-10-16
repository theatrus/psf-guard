//! Flat groups, change events and where each light stands, worked out from
//! what the catalog keeps. Nothing here reads pixels.
//!
//! - A **family** is the measured sessions of one rig, filter, sensor mode
//!   and rotation cluster: the sessions that could ever share a master. A
//!   rotation cluster is single-linkage: angles split only where a gap
//!   between them is wider than the rig's rotation tolerance. A session
//!   whose flats record no angle, on a rig whose other flats do, joins the
//!   cluster it measures alike with, else stays on its own.
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

/// The limits one rig's sessions are held to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub measured: bool,
    /// Fractions of the background.
    pub dust: f64,
    pub broad: f64,
    /// The flat session window, in seconds: a master spans at most twice it.
    pub window: i64,
    /// The rig's rotation tolerance, degrees: a gap in its flats' angles
    /// wider than this separates two rotation clusters.
    pub rotation: f64,
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
    pub rotations: Vec<Option<f64>>,
    /// Seen in two filters or at two angles: on the camera side, so it
    /// applies to every filter of the rig.
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

/// A family of sessions: one rig, filter, sensor mode and rotation cluster.
#[derive(Debug, Clone, Serialize)]
pub struct Family {
    pub key: String,
    pub rig_uuid: String,
    /// The sessions' family key: rig, filter, optics and sensor mode,
    /// rotation aside.
    pub family_key: String,
    pub filter: Option<String>,
    /// The cluster's middle angle; absent for flats that record none.
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
    /// The rigs its profile shot with: only their lights it can place.
    pub rigs: Vec<String>,
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
    /// Boundaries on either side of one night unlike the nights around it,
    /// which measure alike with each other: a hazy dawn, not a change.
    /// They split groups but make no event, so no light is flagged for them.
    pub outliers: BTreeSet<usize>,
    pub angle_terms: HashMap<String, AngleTerm>,
    pub imaging: Vec<ImagingSession>,
    /// Each session's family, by index.
    family_of: Vec<usize>,
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
        self.family_of = vec![usize::MAX; self.sessions.len()];
        for key in keys {
            let members = by_key.remove(&key).unwrap_or_default();
            let tolerance = self
                .limits(&key.0)
                .map_or(ROTATION_CLUSTER_DEG, |limits| limits.rotation);
            let (angled, unknown): (Vec<usize>, Vec<usize>) = members
                .into_iter()
                .partition(|index| self.angle(*index).is_some());
            let mut clusters: Vec<(Option<f64>, Vec<usize>)> = rotation_clusters(
                angled
                    .into_iter()
                    .filter_map(|index| Some((index, self.angle(index)?)))
                    .collect(),
                tolerance,
            )
            .into_iter()
            .map(|cluster| {
                let middle = cluster[cluster.len() / 2].1;
                (Some(middle), cluster.into_iter().map(|(i, _)| i).collect())
            })
            .collect();
            // Flats that record no angle join the cluster they measure
            // alike with; the rest stay together on their own.
            let mut alone = Vec::new();
            for index in unknown {
                match self.measured_cluster(index, &clusters) {
                    Some(cluster) => {
                        self.sessions[index].inferred_rotation = clusters[cluster].0;
                        clusters[cluster].1.push(index);
                    }
                    None => alone.push(index),
                }
            }
            if !alone.is_empty() {
                clusters.push((None, alone));
            }
            for (rotation, mut sessions) in clusters {
                sessions.sort_by_key(|index| (self.sessions[*index].first, *index));
                let filter = self.sessions[sessions[0]].row.filter.clone();
                for index in &sessions {
                    self.family_of[*index] = self.families.len();
                }
                self.families.push(Family {
                    key: format!(
                        "{}:{}",
                        key.1,
                        rotation.map_or_else(|| "none".to_string(), |r| format!("{r:.1}"))
                    ),
                    rig_uuid: key.0.clone(),
                    family_key: key.1.clone(),
                    filter,
                    rotation,
                    sessions,
                    groups: Vec::new(),
                    boundaries: Vec::new(),
                });
            }
        }
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

    /// For a session whose flats record no angle: the rotation cluster one
    /// of whose neighbours in time it measures alike with, the closest
    /// when several do.
    fn measured_cluster(
        &self,
        session: usize,
        clusters: &[(Option<f64>, Vec<usize>)],
    ) -> Option<usize> {
        let limits = self.limits(&self.sessions[session].row.rig_uuid)?;
        clusters
            .iter()
            .enumerate()
            .filter_map(|(c, (_, members))| {
                self.neighbours_in(session, members)
                    .into_iter()
                    .filter_map(|n| self.comparison(session, n))
                    .map(|comparison| limits.distance(comparison))
                    .min_by(f64::total_cmp)
                    .map(|distance| (c, distance))
            })
            .filter(|(_, distance)| *distance <= 1.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(c, _)| c)
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
        Self::spots_meet(
            &a.spots.iter().collect::<Vec<_>>(),
            &b.spots.iter().collect::<Vec<_>>(),
        )
    }

    /// Whether two boundaries share a shadow: a spot at least half as deep
    /// as each one's deepest. A comparison lists faint spots down to 5σ,
    /// and one of them landing near another's is no shared shadow.
    fn shares_shadow(a: &Boundary, b: &Boundary) -> bool {
        Self::spots_meet(&Self::strong_spots(a), &Self::strong_spots(b))
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
                    .comparison(*before, *after)
                    .is_some_and(|c| limits.distance(c) <= 1.0)
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
        // Groups with a split on either side: each proves two changes, one
        // in either split. `(split before, split after)`.
        let mut bounded: Vec<(usize, usize)> = Vec::new();
        for family in &self.families {
            for g in 1..family.groups.len().saturating_sub(1) {
                let (left, right) = (family.boundaries[g - 1], family.boundaries[g]);
                if !self.outliers.contains(&left) && !self.outliers.contains(&right) {
                    bounded.push((left, right));
                }
            }
        }
        for i in 0..self.boundaries.len() {
            for j in (i + 1)..self.boundaries.len() {
                if self.outliers.contains(&i) || self.outliers.contains(&j) {
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
                // splits inside the span the members reach over (give or
                // take one flat run, as the filters of one morning's flats
                // are shot minutes apart), and the members showing the
                // shadows of both. Then some member spans both changes.
                let span = (spans[ri].0.min(spans[rj].0), spans[ri].1.max(spans[rj].1));
                let members: Vec<usize> = merged[ri].iter().chain(&merged[rj]).copied().collect();
                let seen = |split: usize| {
                    members
                        .iter()
                        .any(|m| Self::shares_shadow(&self.boundaries[*m], &self.boundaries[split]))
                };
                if bounded.iter().any(|(before, after)| {
                    let (before_split, after_split) =
                        (&self.boundaries[*before], &self.boundaries[*after]);
                    self.families[before_split.family].rig_uuid == fa.rig_uuid
                        && before_split.start >= span.0 - FLAT_RUN_SECONDS
                        && after_split.end <= span.1 + FLAT_RUN_SECONDS
                        && seen(*before)
                        && seen(*after)
                }) {
                    continue;
                }
                parent[rj] = ri;
                windows[ri] = (start, end);
                spans[ri] = span;
                merged[ri] = members;
            }
        }
        let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in (0..self.boundaries.len()).filter(|i| !self.outliers.contains(i)) {
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
                within_flat_run: end - start < FLAT_RUN_SECONDS,
            });
        }
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

    /// Whether two sessions are of one rig and family key but different
    /// rotation clusters, each with its angle recorded: a pair at two
    /// angles.
    fn across_clusters(&self, a: usize, b: usize) -> bool {
        let (sa, sb) = (&self.sessions[a].row, &self.sessions[b].row);
        sa.rig_uuid == sb.rig_uuid
            && sa.family_key == sb.family_key
            && self.family_of[a] != self.family_of[b]
            && self.angle(a).is_some()
            && self.angle(b).is_some()
    }

    /// Pairs the measurement pass should compare: within each family, every
    /// pair within twice the window and every pair of neighbours; across
    /// rotation clusters of one filter, pairs shot within one window, which
    /// give the rig's angle term; and a session whose flats record no angle
    /// against its neighbours in time in every cluster of its filter, which
    /// tell its angle. Earlier first.
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
        for session in (0..self.sessions.len()).filter(|i| self.angle(*i).is_none()) {
            let row = &self.sessions[session].row;
            for family in &self.families {
                if family.rotation.is_none()
                    || family.rig_uuid != row.rig_uuid
                    || family.family_key != row.family_key
                {
                    continue;
                }
                for neighbour in self.neighbours_in(session, &family.sessions) {
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
                match (self.comparison(*a, *b), self.limits(rig)) {
                    (Some(c), Some(limits)) => limits.distance(c) <= 1.0,
                    _ => false,
                }
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
                if session.rigs.contains(&event.rig_uuid)
                    && session.start > event.start
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
            cmp(
                "r104-may",
                "r104-aug25",
                0.021,
                0.008,
                vec![second.clone(), first],
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
