//! The Calibration page's Flat groups view: each rig's families of flat
//! sessions, the groups they form, why neighbouring groups split, the
//! rig's change events and its angle term; and the ratio picture of two
//! sessions, drawn by the shared preview queue.

use crate::calibration::flat_groups::{self, job, metric, model, overrides, store};
use crate::server::api::ApiResponse;
use crate::server::database_context::DatabaseContext;
use crate::server::extract::DbContext;
use crate::server::handlers::{generating_response, serve_cached_png, AppError};
use crate::server::preview_queue::{GenJob, GenKind, GenerationState};
use crate::server::state::AppState;
use crate::server::storage::CALIBRATION_PREVIEWS;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;
use rusqlite::OpenFlags;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// Bump when the ratio picture changes.
const RATIO_RENDER_VERSION: u32 = 1;

fn pct(value: f64) -> f64 {
    (value * 10_000.0).round() / 100.0
}

#[derive(Debug, Serialize)]
pub struct PairView {
    pub dust_pct: f64,
    pub broad_pct: f64,
}

#[derive(Debug, Serialize)]
pub struct SessionView {
    pub response_key: String,
    pub night: String,
    pub first: i64,
    pub last: i64,
    /// Flats in use in the session, and how many the response combines.
    pub frames: usize,
    pub combined: usize,
    pub noise_pct: Option<f64>,
    /// The response grid: sensor pixels binned by `bin` into
    /// `width` × `height`. Spots are in sensor pixels.
    pub bin: Option<usize>,
    pub width: Option<usize>,
    pub height: Option<usize>,
    /// Against the session before it in its family.
    pub previous: Option<PairView>,
    /// The Calibration page's sessions that hold its flats, so the Frames
    /// view can mark them with their group.
    pub sessions: Vec<String>,
}

/// One change of dust between two sessions, in sensor pixels: enough to
/// draw it at its shape and size.
#[derive(Debug, Serialize)]
pub struct SpotView {
    pub kind: metric::SpotKind,
    pub darker: bool,
    pub x: f32,
    pub y: f32,
    /// A disk's or donut's rim; a spot's or patch's, of a disc of its area.
    pub radius: f32,
    pub peak_pct: f64,
    /// `compact`, `irregular`, `disk`, `donut` or `patch`.
    pub shape: seiza_calibration::dust::DustShape,
    /// A donut's hole, as a fraction of `radius`.
    pub hole: Option<f32>,
    /// Left, top, right, bottom.
    pub bounds: [u32; 4],
    /// Dust both sessions hold that moved no further than focus travel
    /// explains: drawn, not counted.
    pub within_tolerance: bool,
    pub offset_px: Option<f32>,
}

impl SpotView {
    fn of(spot: &metric::Spot) -> Self {
        Self {
            kind: spot.kind,
            darker: spot.darker,
            x: spot.x,
            y: spot.y,
            radius: spot.radius,
            peak_pct: pct(f64::from(spot.peak)),
            shape: spot.shape,
            hole: spot.hole,
            bounds: spot.bounds,
            within_tolerance: spot.within_tolerance,
            offset_px: spot.offset,
        }
    }
}

/// The spots a split or change lists, deepest first, at most this many.
pub const LISTED_SPOTS: usize = 12;

/// Every spot of a split or change, counted before the list is cut to
/// [`LISTED_SPOTS`]: so a count reads the whole comparison, not the list.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct SpotTotals {
    /// Spots that count: not moved within focus travel.
    pub counted: usize,
    /// Dust both sessions hold that moved no further than focus travel
    /// explains: drawn, not counted.
    pub within_tolerance: usize,
    /// Of those that count, by `new`, `gone` or `moved`, and by shape.
    pub by_kind: BTreeMap<String, usize>,
    pub by_shape: BTreeMap<String, usize>,
    /// How many `spots` lists, of every spot.
    pub listed: usize,
}

impl SpotTotals {
    pub fn of(spots: &[metric::Spot]) -> Self {
        let word = |value: serde_json::Result<serde_json::Value>| {
            value
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".into())
        };
        let mut totals = Self {
            listed: spots.len().min(LISTED_SPOTS),
            ..Self::default()
        };
        for spot in spots {
            if spot.within_tolerance {
                totals.within_tolerance += 1;
                continue;
            }
            totals.counted += 1;
            *totals
                .by_kind
                .entry(word(serde_json::to_value(spot.kind)))
                .or_default() += 1;
            *totals
                .by_shape
                .entry(word(serde_json::to_value(spot.shape)))
                .or_default() += 1;
        }
        totals
    }
}

/// The deepest [`LISTED_SPOTS`] of a comparison's spots, as the view lists
/// them.
fn listed_spots(spots: &[metric::Spot]) -> Vec<SpotView> {
    spots.iter().take(LISTED_SPOTS).map(SpotView::of).collect()
}

/// A change of vignetting between two sessions: where, and how deep.
#[derive(Debug, Serialize)]
pub struct VignettingView {
    pub edge: seiza_calibration::dust::Edge,
    pub darker: bool,
    pub x: f32,
    pub y: f32,
    pub area: f32,
    pub depth_pct: f64,
}

impl VignettingView {
    fn of(change: &seiza_calibration::dust::VignettingChange) -> Self {
        let v = &change.vignetting;
        Self {
            edge: v.edge,
            darker: change.darker,
            x: v.x,
            y: v.y,
            area: v.area,
            depth_pct: pct(f64::from(v.depth)),
        }
    }
}

use flat_groups::{edge_words, vignetting_leads};

#[derive(Debug, Serialize)]
pub struct SplitView {
    pub before_response: String,
    pub after_response: String,
    pub before_night: String,
    pub after_night: String,
    pub start: i64,
    pub end: i64,
    pub dust_pct: Option<f64>,
    pub broad_pct: Option<f64>,
    /// The deepest change of vignetting, when the pair was compared, and
    /// the changes.
    pub vignetting_pct: Option<f64>,
    pub vignetting: Vec<VignettingView>,
    /// Why, in words, with the values and the limits.
    pub reason: String,
    /// The change event this split belongs to.
    pub event_key: Option<String>,
    /// The deepest [`LISTED_SPOTS`] spots, and every spot counted.
    pub spots: Vec<SpotView>,
    pub spot_totals: SpotTotals,
    /// Set when a person's choice made it (`split`, where the nights
    /// measure alike) or undid it (`joined`, a measured split inside a
    /// group now). The numbers above are the measurements either way.
    pub by_hand: Option<model::HandEdge>,
    /// The person's split or join at this edge, when there is one.
    pub choice: Option<OverrideView>,
}

/// A person's split or join, as the view shows it.
#[derive(Debug, Serialize)]
pub struct OverrideView {
    /// `split` or `join`.
    pub choice: Option<overrides::GroupChoice>,
    /// The edge in words when it was made.
    pub label: String,
    pub chosen_at: i64,
    /// The recorded edge: what a reset names.
    pub anchor_frame: String,
    /// In force: whether the measurements alone group the same.
    pub agrees: Option<bool>,
    /// Why it no longer applies, when its flats are gone or no longer one
    /// family. It stays until a person resets it.
    pub orphaned: Option<String>,
    /// A camera-side change it stands against; both stay in force.
    pub conflict: Option<ConflictView>,
}

/// A camera-side change a person's split or join stands against.
#[derive(Debug, Serialize)]
pub struct ConflictView {
    pub key: String,
    pub start: i64,
    pub end: i64,
    pub what: String,
    pub filters: Vec<String>,
}

impl OverrideView {
    fn of(book: &model::Book, state: &overrides::OverrideState) -> Self {
        let (agrees, orphaned) = match &state.status {
            overrides::OverrideStatus::Applied { agrees } => (Some(*agrees), None),
            overrides::OverrideStatus::Orphaned { reason } => (None, Some(reason.clone())),
        };
        Self {
            choice: state.row.choice,
            label: state.row.label.clone(),
            chosen_at: state.row.chosen_at,
            anchor_frame: state.row.anchor_frame.clone(),
            agrees,
            orphaned,
            conflict: state.conflict.as_ref().and_then(|key| {
                let event = book.events.iter().find(|event| &event.key == key)?;
                Some(ConflictView {
                    key: key.clone(),
                    start: event.start,
                    end: event.end,
                    what: flat_groups::describe_what(event),
                    filters: event.filters.iter().map(|f| f.to_uppercase()).collect(),
                })
            }),
        }
    }
}

/// Two sessions of one group measured against each other.
#[derive(Debug, Serialize)]
pub struct MemberPairView {
    pub earlier_response: String,
    pub later_response: String,
    pub earlier_night: String,
    pub later_night: String,
    pub dust_pct: f64,
    pub broad_pct: f64,
}

#[derive(Debug, Serialize)]
pub struct GroupView {
    /// A name for the group that outlives a regroup where it can
    /// ([`group_key`]). Any member session finds the group again.
    pub key: String,
    pub first_night: String,
    pub last_night: String,
    pub frames: usize,
    /// Newest first.
    pub sessions: Vec<SessionView>,
    /// Why they share a group: each session against the next in time, and
    /// the pairs that differ most in dust and in light pattern, oldest
    /// first. One per neighbour, so a long group stays small.
    pub pairs: Vec<MemberPairView>,
    /// The most any two of its sessions measured apart, over every pair
    /// measured, with how many were.
    pub widest: Option<WidestView>,
}

/// The largest differences within one group.
#[derive(Debug, Serialize)]
pub struct WidestView {
    pub dust_pct: f64,
    pub broad_pct: f64,
    /// Pairs of its sessions measured against each other.
    pub compared: usize,
}

#[derive(Debug, Serialize)]
pub struct FamilyView {
    pub key: String,
    pub filter: Option<String>,
    /// The middle angle of its largest rotation cluster.
    pub rotation: Option<f64>,
    /// Its measured angle range: the recorded angles of each rotation
    /// cluster that measured alike and joined it.
    pub angles: Vec<model::AngleSpan>,
    /// That range in words: "100.1–104.4°", "72.6° & 82.2°", "no angle".
    pub angle_label: String,
    /// Newest first; `splits[i]` lies between `groups[i + 1]` and
    /// `groups[i]`.
    pub groups: Vec<GroupView>,
    pub splits: Vec<SplitView>,
    /// Measured splits a person joined, newest first: each lies inside a
    /// group now, between `before_response` and `after_response`.
    pub joins: Vec<SplitView>,
}

#[derive(Debug, Serialize)]
pub struct EventView {
    pub key: String,
    pub start: i64,
    pub end: i64,
    pub what: String,
    pub dust_pct: f64,
    pub broad_pct: f64,
    pub vignetting_pct: f64,
    pub vignetting: Vec<VignettingView>,
    pub camera_side: bool,
    pub filters: Vec<String>,
    pub rotations: Vec<Option<f64>>,
    /// The deepest [`LISTED_SPOTS`] spots, and every spot counted.
    pub spots: Vec<SpotView>,
    pub spot_totals: SpotTotals,
    /// A before and an after session to compare.
    pub before_response: String,
    pub after_response: String,
    /// The window is shorter than one flat run: the change landed between
    /// one twilight's flats, or two changes met there.
    pub within_flat_run: bool,
}

#[derive(Debug, Serialize)]
pub struct UnmeasuredView {
    pub night: String,
    pub filter: Option<String>,
    pub rotation: Option<f64>,
    pub frames: usize,
    pub reason: String,
    /// The Calibration page's sessions that hold its flats.
    pub sessions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RigView {
    pub rig_uuid: String,
    pub name: String,
    /// Whether this rig's flats are grouped by measurement.
    pub measured: bool,
    pub dust_limit_pct: f64,
    pub pattern_limit_pct: f64,
    /// The flat-group settings this rig sets for itself; the rest it
    /// inherits from the global ones.
    pub own_limits: Vec<&'static str>,
    pub angle_term: Option<PairView>,
    pub angle_pairs: usize,
    /// The rig's clock over time ([`crate::calibration::night::clock_runs`]):
    /// its times are shown on it, and on the viewer's clock when it is
    /// empty.
    pub clock: Vec<crate::calibration::night::ClockRun>,
    /// Newest first.
    pub events: Vec<EventView>,
    pub families: Vec<FamilyView>,
    pub unmeasured: Vec<UnmeasuredView>,
    /// A person's splits and joins that find nothing to apply to now.
    pub orphaned: Vec<OverrideView>,
}

#[derive(Debug, Serialize)]
pub struct FlatGroupsView {
    /// Whether this catalog keeps flat grouping's tables at all.
    pub available: bool,
    pub rigs: Vec<RigView>,
}

fn split_reason(
    book: &model::Book,
    index: usize,
    boundary: &model::Boundary,
    limits: &model::Limits,
) -> String {
    let night = |index: usize| book.sessions[index].row.night.clone();
    let measured = |boundary: &model::Boundary| match (boundary.dust, boundary.broad) {
        (Some(dust), Some(broad)) => format!(
            "{:.2}% dust (limit {}%), {:.2}% light pattern (limit {}%)",
            dust * 100.0,
            pct(limits.dust),
            broad * 100.0,
            pct(limits.broad)
        ),
        _ => "the two nights were not compared".into(),
    };
    match boundary.by_hand {
        Some(model::HandEdge::Split) => {
            return format!("split by you; measured alike: {}", measured(boundary));
        }
        Some(model::HandEdge::Joined) => {
            return format!("joined by you; measured apart: {}", measured(boundary));
        }
        None => {}
    }
    if book.outliers.contains(&index) {
        return "one night unlike the nights either side of it, which measure alike: no \
                change, and no light is flagged for it"
            .into();
    }
    if let Some((a, b, distance)) = boundary.drift {
        return format!(
            "drifted: {} and {} differ by {:.1}× the limits, though each night is close to \
             the next",
            night(a),
            night(b),
            distance
        );
    }
    let dust = boundary.dust.unwrap_or(0.0);
    let broad = boundary.broad.unwrap_or(0.0);
    let vignetting = boundary.vignetting.unwrap_or(0.0);
    if vignetting_leads(dust, broad, vignetting, limits) {
        // A shadow from the border moved or deepened: the flats differ, but
        // no dust arrived.
        return format!(
            "vignetting changed at the {} by {:.1}% of the background (limit {}%): not dust",
            boundary
                .vignetting_changes
                .first()
                .map_or("border", |change| edge_words(change.vignetting.edge)),
            vignetting * 100.0,
            pct(limits.broad)
        );
    }
    if dust / limits.dust.max(1e-9) >= broad / limits.broad.max(1e-9) {
        format!(
            "{}, {:.1}% of the background (limit {}%)",
            flat_groups::describe_spots(&boundary.spots, dust, broad),
            dust * 100.0,
            pct(limits.dust)
        )
    } else {
        format!(
            "the light pattern changed by {:.1}% (limit {}%)",
            broad * 100.0,
            pct(limits.broad)
        )
    }
}

/// The settings that decide how a rig's flats group.
const LIMIT_KEYS: [&str; 3] = [
    "flat_grouping",
    "flat_split_dust_pct",
    "flat_split_pattern_pct",
];

/// The browser's sessions holding `uuids`, in their order, each once.
fn browser_sessions<'a>(
    uuids: impl IntoIterator<Item = &'a String>,
    session_of: &HashMap<&str, &str>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for uuid in uuids {
        if let Some(session) = session_of.get(uuid.as_str())
            && !out.iter().any(|seen| seen == session)
        {
            out.push((*session).to_string());
        }
    }
    out
}

/// A group's key: the first, by id, of the browser's sessions holding its
/// oldest member's flats, else that member's response key. It keeps off
/// the response key where it can, since that changes with the measuring
/// algorithm, and a regroup that keeps the oldest member keeps it. A
/// browser session that holds flats of two families (a night shot at two
/// gains, say) would name two groups, so there the key also carries a
/// digest of the family's key. `shared` holds such sessions.
pub(crate) fn group_key(
    oldest: &SessionView,
    family_key: &str,
    shared: &HashSet<String>,
) -> String {
    match oldest.sessions.iter().min() {
        Some(session) if shared.contains(session) => {
            let digest: String = Sha256::digest(family_key.as_bytes())[..4]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            format!("{session}~{digest}")
        }
        Some(session) => session.clone(),
        None => oldest.response_key.clone(),
    }
}

/// The pairs a group shows: each session against the next in time, and
/// the widest in dust and in light pattern; with the widest of all.
fn member_pairs(book: &model::Book, group: &[usize]) -> (Vec<MemberPairView>, Option<WidestView>) {
    let mut measured: Vec<(usize, usize, f64, f64)> = Vec::new();
    for (position, earlier) in group.iter().enumerate() {
        for later in &group[position + 1..] {
            if let Some(c) = book.comparison(*earlier, *later) {
                measured.push((*earlier, *later, c.dust, c.broad));
            }
        }
    }
    let widest = |of: fn(&(usize, usize, f64, f64)) -> f64| {
        measured
            .iter()
            .max_by(|a, b| of(a).total_cmp(&of(b)))
            .map(|pair| (pair.0, pair.1))
    };
    let keep: HashSet<(usize, usize)> = group
        .windows(2)
        .map(|pair| (pair[0], pair[1]))
        .chain(widest(|pair| pair.2))
        .chain(widest(|pair| pair.3))
        .collect();
    let pairs = measured
        .iter()
        .filter(|pair| keep.contains(&(pair.0, pair.1)))
        .map(|(earlier, later, dust, broad)| {
            let (a, b) = (&book.sessions[*earlier].row, &book.sessions[*later].row);
            MemberPairView {
                earlier_response: a.response_key.clone(),
                later_response: b.response_key.clone(),
                earlier_night: a.night.clone(),
                later_night: b.night.clone(),
                dust_pct: pct(*dust),
                broad_pct: pct(*broad),
            }
        })
        .collect();
    let summary = (!measured.is_empty()).then(|| WidestView {
        dust_pct: pct(measured.iter().map(|pair| pair.2).fold(0.0, f64::max)),
        broad_pct: pct(measured.iter().map(|pair| pair.3).fold(0.0, f64::max)),
        compared: measured.len(),
    });
    (pairs, summary)
}

fn read_only(path: &str) -> Result<rusqlite::Connection, AppError> {
    crate::server::database_context::open_scheduler_connection_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(AppError::db)
}

pub(crate) fn build_view(conn: &rusqlite::Connection) -> anyhow::Result<FlatGroupsView> {
    Ok(build_view_with_frames(conn)?.0)
}

/// Each group's frames, by UUID: the flats its measured sessions combine.
pub(crate) type FrameGroups = HashMap<String, String>;

/// [`build_view`], with the key of the group each measured flat is in.
pub(crate) fn build_view_with_frames(
    conn: &rusqlite::Connection,
) -> anyhow::Result<(FlatGroupsView, FrameGroups)> {
    let mut frame_groups = FrameGroups::new();
    if !store::tables_exist(conn) {
        return Ok((
            FlatGroupsView {
                available: false,
                rigs: Vec::new(),
            },
            frame_groups,
        ));
    }
    let book = flat_groups::load_book(conn)?;
    let names: HashMap<String, String> = crate::calibration::rig_listing(conn)
        .unwrap_or_default()
        .into_iter()
        .map(|rig| (rig.rig_uuid, rig.name))
        .collect();
    // The Calibration page's own sessions, to point each measured one at.
    let browsed = crate::calibration::browse::library_frames(conn)?;
    let session_of: HashMap<&str, &str> = browsed
        .frames
        .iter()
        .map(|row| (row.frame.frame_uuid.as_str(), row.session.as_str()))
        .collect();
    // Browser sessions holding flats of more than one family.
    let mut families_of: HashMap<&str, HashSet<&str>> = HashMap::new();
    for family in &book.families {
        for index in &family.sessions {
            for uuid in &book.sessions[*index].row.frame_uuids {
                if let Some(session) = session_of.get(uuid.as_str()) {
                    families_of
                        .entry(session)
                        .or_default()
                        .insert(family.key.as_str());
                }
            }
        }
    }
    let shared: HashSet<String> = families_of
        .into_iter()
        .filter(|(_, families)| families.len() > 1)
        .map(|(session, _)| session.to_string())
        .collect();
    let rule_book = crate::calibration::rules::rule_book();
    let nights = crate::calibration::Nights::load(conn);
    let mut rigs: BTreeMap<String, RigView> = BTreeMap::new();
    let rig_view = |rig: &str, book: &model::Book| {
        let rules = crate::calibration::rules_for_rig(rig);
        let limits = rules.grouping_limits();
        let term = book.angle_terms.get(rig).copied();
        RigView {
            rig_uuid: rig.to_string(),
            name: names.get(rig).cloned().unwrap_or_else(|| rig.to_string()),
            measured: limits.measured,
            dust_limit_pct: pct(limits.dust),
            pattern_limit_pct: pct(limits.broad),
            own_limits: LIMIT_KEYS
                .into_iter()
                .filter(|key| rule_book.rig_sets(rig, key))
                .collect(),
            angle_term: term.map(|t| PairView {
                dust_pct: pct(t.dust),
                broad_pct: pct(t.broad),
            }),
            angle_pairs: term.map_or(0, |t| t.pairs),
            clock: crate::calibration::night::clock_runs(conn, rig),
            events: Vec::new(),
            families: Vec::new(),
            unmeasured: Vec::new(),
            orphaned: Vec::new(),
        }
    };
    // Each edge's split or join, by the boundary it made or joined.
    let mut edge_choice: HashMap<usize, usize> = HashMap::new();
    for (index, state) in book.overrides.iter().enumerate() {
        for boundary in &state.boundaries {
            edge_choice.insert(*boundary, index);
        }
    }
    let boundary_event: HashMap<usize, String> = book
        .events
        .iter()
        .flat_map(|event| {
            event
                .boundaries
                .iter()
                .map(move |b| (*b, event.key.clone()))
        })
        .collect();
    for (family_index, family) in book.families.iter().enumerate() {
        let rig = rigs
            .entry(family.rig_uuid.clone())
            .or_insert_with(|| rig_view(&family.rig_uuid, &book));
        let limits = flat_groups::rig_limits(&family.rig_uuid);
        let mut previous: Option<usize> = None;
        let mut groups: Vec<GroupView> = Vec::new();
        for group in &family.groups {
            let mut sessions = Vec::new();
            for index in group {
                let session = &book.sessions[*index];
                let against = previous.and_then(|p| book.comparison(p, *index));
                sessions.push(SessionView {
                    response_key: session.row.response_key.clone(),
                    night: session.row.night.clone(),
                    first: session.first,
                    last: session.last,
                    frames: session.row.frame_uuids.len(),
                    combined: session.row.frames,
                    noise_pct: session.row.noise.map(pct),
                    bin: session.row.bin,
                    width: session.row.width,
                    height: session.row.height,
                    previous: against.map(|c| PairView {
                        dust_pct: pct(c.dust),
                        broad_pct: pct(c.broad),
                    }),
                    sessions: browser_sessions(&session.row.frame_uuids, &session_of),
                });
                previous = Some(*index);
            }
            let frames = group
                .iter()
                .map(|i| book.sessions[*i].row.frame_uuids.len())
                .sum();
            let (pairs, widest) = member_pairs(&book, group);
            let key = group_key(&sessions[0], &family.key, &shared);
            for index in group {
                for uuid in &book.sessions[*index].row.frame_uuids {
                    frame_groups.insert(uuid.clone(), key.clone());
                }
            }
            sessions.reverse();
            groups.push(GroupView {
                key,
                pairs,
                widest,
                first_night: book.sessions[group[0]].row.night.clone(),
                last_night: book.sessions[*group.last().expect("not empty")]
                    .row
                    .night
                    .clone(),
                frames,
                sessions,
            });
        }
        let split_view = |b: &usize| {
            let boundary = &book.boundaries[*b];
            // A split a person made where the measurements split too: the
            // edge is the measurements', the choice still shows.
            let choice = edge_choice.get(b).copied().or_else(|| {
                book.overrides.iter().position(|state| {
                    state.applied()
                        && state.row.choice == Some(overrides::GroupChoice::Split)
                        && state.after == Some(boundary.after)
                })
            });
            // A split by hand has no window: it falls where its later
            // night starts, as its change does.
            let window = match boundary_event.get(b) {
                Some(key) if boundary.by_hand == Some(model::HandEdge::Split) => book
                    .events
                    .iter()
                    .find(|event| &event.key == key)
                    .map_or((boundary.start, boundary.end), |event| {
                        (event.start, event.end)
                    }),
                _ => (boundary.start, boundary.end),
            };
            SplitView {
                before_response: book.sessions[boundary.before].row.response_key.clone(),
                after_response: book.sessions[boundary.after].row.response_key.clone(),
                before_night: book.sessions[boundary.before].row.night.clone(),
                after_night: book.sessions[boundary.after].row.night.clone(),
                start: window.0,
                end: window.1,
                dust_pct: boundary.dust.map(pct),
                broad_pct: boundary.broad.map(pct),
                vignetting_pct: boundary.vignetting.map(pct),
                vignetting: boundary
                    .vignetting_changes
                    .iter()
                    .map(VignettingView::of)
                    .collect(),
                reason: split_reason(&book, *b, boundary, &limits),
                event_key: boundary_event.get(b).cloned(),
                spots: listed_spots(&boundary.spots),
                spot_totals: SpotTotals::of(&boundary.spots),
                by_hand: boundary.by_hand,
                choice: choice.map(|index| OverrideView::of(&book, &book.overrides[index])),
            }
        };
        let mut splits: Vec<SplitView> = family.boundaries.iter().map(split_view).collect();
        let mut joins: Vec<SplitView> = family.joined.iter().map(split_view).collect();
        joins.reverse();
        groups.reverse();
        splits.reverse();
        rig.families.push(FamilyView {
            key: family.key.clone(),
            filter: family.filter.clone(),
            rotation: family.rotation,
            angles: family.angles.clone(),
            angle_label: book.angle_label(family_index),
            groups,
            splits,
            joins,
        });
    }
    // Splits and joins that find nothing now: shown under their rig.
    for state in book.overrides.iter().filter(|state| !state.applied()) {
        let rig = rigs
            .entry(state.row.rig_uuid.clone())
            .or_insert_with(|| rig_view(&state.row.rig_uuid, &book));
        rig.orphaned.push(OverrideView::of(&book, state));
    }
    for event in book.events.iter().rev() {
        let rig = rigs
            .entry(event.rig_uuid.clone())
            .or_insert_with(|| rig_view(&event.rig_uuid, &book));
        let index = book
            .events
            .iter()
            .position(|e| e.key == event.key)
            .unwrap_or(0);
        let (before_response, after_response) = book.map_source(index, None).unwrap_or_else(|| {
            let boundary = &book.boundaries[event.boundaries[0]];
            (
                book.sessions[boundary.before].row.response_key.clone(),
                book.sessions[boundary.after].row.response_key.clone(),
            )
        });
        rig.events.push(EventView {
            key: event.key.clone(),
            start: event.start,
            end: event.end,
            what: flat_groups::describe_what(event),
            dust_pct: pct(event.dust),
            broad_pct: pct(event.broad),
            vignetting_pct: pct(event.vignetting),
            vignetting: event
                .vignetting_changes
                .iter()
                .map(VignettingView::of)
                .collect(),
            camera_side: event.camera_side,
            filters: event.filters.iter().map(|f| f.to_uppercase()).collect(),
            rotations: event.rotations.clone(),
            spots: listed_spots(&event.spots),
            spot_totals: SpotTotals::of(&event.spots),
            before_response,
            after_response,
            within_flat_run: event.within_flat_run,
        });
    }
    // Sessions the pass could not measure, or has not reached.
    let library = crate::calibration::LibrarySnapshot::load(conn)?;
    for plan in job::plan_sessions(library.frames(), &nights) {
        let key = plan.response_key();
        let measured = book.session_by_response(&key).is_some();
        if measured {
            continue;
        }
        let rig = rigs
            .entry(plan.rig_uuid.clone())
            .or_insert_with(|| rig_view(&plan.rig_uuid, &book));
        rig.unmeasured.push(UnmeasuredView {
            night: plan.night.clone(),
            filter: plan.filter.clone(),
            rotation: plan.rotation,
            frames: plan.frames.len(),
            reason: if plan.frames.len() < metric::MIN_FRAMES {
                format!("needs {} flats", metric::MIN_FRAMES)
            } else {
                "not measured yet".into()
            },
            sessions: browser_sessions(plan.frames.iter().map(|f| &f.frame_uuid), &session_of),
        });
    }
    for rig in rigs.values_mut() {
        rig.unmeasured.sort_by(|a, b| b.night.cmp(&a.night));
        rig.families.sort_by(|a, b| {
            a.filter.cmp(&b.filter).then(
                a.rotation
                    .partial_cmp(&b.rotation)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
    }
    Ok((
        FlatGroupsView {
            available: true,
            rigs: rigs.into_values().collect(),
        },
        frame_groups,
    ))
}

/// Keep only the families a project's lights touch: those with a session
/// holding one of `sessions` (the Calibration page's sessions), the change
/// events found in them, and the unmeasured sessions among `sessions`. A
/// rig left with nothing goes.
fn scoped(view: FlatGroupsView, sessions: &BTreeSet<String>) -> FlatGroupsView {
    let touches = |held: &[String]| held.iter().any(|id| sessions.contains(id));
    let rigs = view
        .rigs
        .into_iter()
        .filter_map(|mut rig| {
            rig.families.retain(|family| {
                family
                    .groups
                    .iter()
                    .flat_map(|group| &group.sessions)
                    .any(|session| touches(&session.sessions))
            });
            let events: BTreeSet<&str> = rig
                .families
                .iter()
                .flat_map(|family| &family.splits)
                .filter_map(|split| split.event_key.as_deref())
                .collect();
            let events: BTreeSet<String> = events.into_iter().map(str::to_owned).collect();
            rig.events.retain(|event| events.contains(&event.key));
            rig.unmeasured.retain(|session| touches(&session.sessions));
            (!rig.families.is_empty() || !rig.unmeasured.is_empty()).then_some(rig)
        })
        .collect();
    FlatGroupsView {
        available: view.available,
        rigs,
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct FlatGroupsQuery {
    /// Keep only the families this project's lights touch.
    pub project: Option<i32>,
}

/// Each group's own key, and each of its sessions and response keys, to
/// the group's key: how a key a link carries finds its group after a
/// regroup, as the Calibration page finds it.
pub(crate) fn aliases_of(view: &FlatGroupsView) -> HashMap<String, String> {
    let mut aliases = HashMap::new();
    for rig in &view.rigs {
        for family in &rig.families {
            for group in &family.groups {
                for session in &group.sessions {
                    aliases
                        .entry(session.response_key.clone())
                        .or_insert_with(|| group.key.clone());
                    for id in &session.sessions {
                        aliases
                            .entry(id.clone())
                            .or_insert_with(|| group.key.clone());
                    }
                }
            }
        }
    }
    // A group's own key wins over a session that happens to share it.
    for rig in &view.rigs {
        for family in &rig.families {
            for group in &family.groups {
                aliases.insert(group.key.clone(), group.key.clone());
            }
        }
    }
    aliases
}

/// The browser's sessions of the group `key` names, by its key or one of
/// its sessions or response keys, as [`aliases_of`] finds it; none when no
/// group answers to it.
pub(crate) fn group_sessions(
    conn: &rusqlite::Connection,
    key: &str,
) -> anyhow::Result<Option<BTreeSet<String>>> {
    let view = build_view(conn)?;
    let Some(group) = aliases_of(&view).get(key).cloned() else {
        return Ok(None);
    };
    Ok(view
        .rigs
        .iter()
        .flat_map(|rig| &rig.families)
        .flat_map(|family| &family.groups)
        .find(|candidate| candidate.key == group)
        .map(|found| {
            found
                .sessions
                .iter()
                .flat_map(|session| session.sessions.iter().cloned())
                .collect()
        }))
}

/// Where one of the Calibration page's flat sessions would sit among the
/// flat groups, measured or not (its flats all kept out, say): the family
/// it would join, by the settings part of the family's key, and its night
/// and angle.
#[derive(Debug, Default, PartialEq, Serialize)]
pub struct SessionPlace {
    pub rig_uuid: Option<String>,
    /// The family's settings: a [`FamilyView::key`] begins with it and `:`.
    pub family_key: Option<String>,
    pub night: Option<String>,
    pub rotation: Option<f64>,
}

/// [`SessionPlace`] of the browser's session `session`: its flats planned
/// as the background pass plans sessions, as though none were kept out.
pub(crate) fn place_of(conn: &rusqlite::Connection, session: &str) -> anyhow::Result<SessionPlace> {
    let browsed = crate::calibration::browse::library_frames(conn)?;
    let wanted: HashSet<&str> = browsed
        .frames
        .iter()
        .filter(|row| {
            row.session == session && row.frame.kind == crate::calibration::CalibrationKind::Flat
        })
        .map(|row| row.frame.frame_uuid.as_str())
        .collect();
    if wanted.is_empty() {
        return Ok(SessionPlace::default());
    }
    let library = crate::calibration::LibrarySnapshot::load(conn)?;
    // A flat with no capture time is planned at its session's first, so
    // it is placed too; a session with none at all is placed in its
    // family, at no night.
    let first = library
        .frames()
        .iter()
        .filter(|frame| wanted.contains(frame.frame_uuid.as_str()))
        .filter_map(|frame| frame.captured_at)
        .min();
    let frames: Vec<crate::calibration::CalibrationFrame> = library
        .frames()
        .iter()
        .cloned()
        .map(|mut frame| {
            if wanted.contains(frame.frame_uuid.as_str()) {
                frame.kept_out = None;
                frame.captured_at = frame.captured_at.or(first).or(Some(0));
            }
            frame
        })
        .collect();
    let nights = crate::calibration::Nights::load(conn);
    Ok(job::plan_sessions(&frames, &nights)
        .into_iter()
        .find(|plan| {
            plan.frames
                .iter()
                .any(|frame| wanted.contains(frame.frame_uuid.as_str()))
        })
        .map(|plan| SessionPlace {
            rig_uuid: Some(plan.rig_uuid),
            family_key: Some(plan.family_key),
            night: first.map(|_| plan.night),
            rotation: plan.rotation,
        })
        .unwrap_or_default())
}

#[derive(Debug, Deserialize)]
pub struct PlaceQuery {
    pub session: String,
}

/// `GET /api/db/{db}/calibrations/flat-groups/place?session=` — the family,
/// night and angle a flat session would take among the groups, so a link
/// to a group by a session no group holds now still finds one.
pub async fn get_session_place(
    ctx: DbContext,
    Query(query): Query<PlaceQuery>,
) -> Result<Json<ApiResponse<SessionPlace>>, AppError> {
    let path = ctx.database_path.clone();
    let place = tokio::task::spawn_blocking(move || {
        let conn = read_only(&path)?;
        place_of(&conn, &query.session).map_err(AppError::db)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("placing a flat session: {error}")))??;
    Ok(Json(ApiResponse::success(place)))
}

/// `GET /api/db/{db}/calibrations/flat-groups` — every rig's flat groups,
/// splits, change events and angle term, from what the background pass
/// measured. Reads no pixels.
pub async fn get_flat_groups(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Query(query): Query<FlatGroupsQuery>,
) -> Result<Json<ApiResponse<FlatGroupsView>>, AppError> {
    let scope = match query.project {
        Some(project) => {
            Some(crate::server::calibration_scope::scope_for(&state, &ctx.0, project).await?)
        }
        None => None,
    };
    let path = ctx.database_path.clone();
    let view = tokio::task::spawn_blocking(move || {
        let conn = read_only(&path)?;
        let view = build_view(&conn).map_err(AppError::db)?;
        Ok::<_, AppError>(match scope {
            Some(scope) => scoped(view, &scope.sessions),
            None => view,
        })
    })
    .await
    .map_err(|error| AppError::InternalError(format!("reading flat groups: {error}")))??;
    Ok(Json(ApiResponse::success(view)))
}

/// What the ratio picture shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RatioStretch {
    /// ±3%.
    Wide,
    /// ±1%.
    #[default]
    Normal,
    /// ±0.5%.
    Tight,
}

impl RatioStretch {
    fn half_window(self) -> f32 {
        match self {
            Self::Wide => 0.03,
            Self::Normal => 0.01,
            Self::Tight => 0.005,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Wide => "wide",
            Self::Normal => "normal",
            Self::Tight => "tight",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RatioQuery {
    pub earlier: String,
    pub later: String,
    #[serde(default)]
    pub stretch: Option<RatioStretch>,
}

/// What the queue draws for a ratio picture.
#[derive(Debug, Clone)]
pub struct RatioSpec {
    pub dir: PathBuf,
    pub earlier: store::ResponseRow,
    pub later: store::ResponseRow,
    pub stretch: RatioStretch,
}

/// Draw `later / earlier`, stars at the neutral level, as a grey window
/// around 1.
pub fn render_ratio(spec: &RatioSpec, output: &std::path::Path) -> anyhow::Result<()> {
    let earlier = job::session_plane(&spec.dir, &spec.earlier)
        .ok_or_else(|| anyhow::anyhow!("the earlier session's response is not cached"))?;
    let later = job::session_plane(&spec.dir, &spec.later)
        .ok_or_else(|| anyhow::anyhow!("the later session's response is not cached"))?;
    anyhow::ensure!(
        earlier.width == later.width && earlier.height == later.height,
        "the two sessions differ in size"
    );
    let stars: Vec<bool> = metric::star_mask(&earlier)
        .into_iter()
        .zip(metric::star_mask(&later))
        .map(|(a, b)| a || b)
        .collect();
    let ratio: Vec<f32> = earlier
        .data
        .iter()
        .zip(&later.data)
        .map(|(a, b)| {
            if *a > 0.05 && *b > 0.05 {
                b / a
            } else {
                f32::NAN
            }
        })
        .collect();
    let level = metric::sampled_median(&ratio);
    let half = spec.stretch.half_window();
    let pixels: Vec<u8> = ratio
        .iter()
        .zip(&stars)
        .map(|(value, star)| {
            let value = if *star || !value.is_finite() {
                1.0
            } else {
                value / level
            };
            (((value - 1.0) / half + 1.0) * 127.5).clamp(0.0, 255.0) as u8
        })
        .collect();
    let image = image::GrayImage::from_raw(earlier.width as u32, earlier.height as u32, pixels)
        .ok_or_else(|| anyhow::anyhow!("could not lay out the picture"))?;
    image
        .save_with_format(output, image::ImageFormat::Png)
        .map_err(|error| anyhow::anyhow!("writing {}: {error}", output.display()))
}

fn ratio_job(ctx: &DatabaseContext, query: &RatioQuery) -> Result<GenJob, AppError> {
    let conn = read_only(&ctx.database_path)?;
    let rows = store::load_responses(&conn).map_err(AppError::db)?;
    let find = |key: &str| {
        rows.iter()
            .find(|row| row.response_key == key)
            .cloned()
            .ok_or_else(|| AppError::NotFoundMessage("No measured session has that key".into()))
    };
    let (earlier, later) = (find(&query.earlier)?, find(&query.later)?);
    let dir = ctx
        .cache()
        .ensure_category_dir(flat_groups::CACHE_CATEGORY)
        .map_err(AppError::db)?;
    let source = flat_groups::response_path(&dir, &earlier.response_key);
    if !source.is_file() {
        return Err(AppError::NotFoundMessage(
            "That session's response is not cached; the next grading pass measures it again".into(),
        ));
    }
    let folder = ctx
        .cache()
        .ensure_category_dir(CALIBRATION_PREVIEWS)
        .map_err(AppError::db)?;
    let stretch = query.stretch.unwrap_or_default();
    let key: String = Sha256::digest(
        format!(
            "{RATIO_RENDER_VERSION}:ratio:{}:{}:{}:{:?}:{:?}",
            earlier.response_key,
            later.response_key,
            stretch.as_str(),
            earlier.pedestal_adu,
            later.pedestal_adu
        )
        .as_bytes(),
    )
    .iter()
    .map(|b| format!("{b:02x}"))
    .collect();
    Ok(GenJob {
        cache_path: folder.join(format!("{key}.png")),
        fits_path: source,
        kind: GenKind::FlatRatio(Box::new(RatioSpec {
            dir,
            earlier,
            later,
            stretch,
        })),
        encoding: crate::preview_format::PreviewEncoding::png(),
    })
}

/// `GET /api/db/{db}/calibrations/flat-groups/ratio?earlier=&later=` — the
/// later session divided by the earlier, the picture when it is ready, 202
/// while it is queued. Never drawn in the request.
pub async fn get_ratio_picture(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Query(query): Query<RatioQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let ctx = Arc::clone(&ctx.0);
    let (status, cache_path) = tokio::task::spawn_blocking(move || {
        let job = ratio_job(&ctx, &query)?;
        let cache_path = job.cache_path.clone();
        Ok::<_, AppError>((
            crate::server::stack_preview::calibration_masters::generation_status(&state, job),
            cache_path,
        ))
    })
    .await
    .map_err(|error| AppError::InternalError(format!("queueing the picture: {error}")))??;
    match status.state {
        GenerationState::Ready => serve_cached_png(&cache_path, &headers).await,
        GenerationState::Generating => Ok(generating_response()),
        GenerationState::Error => Err(AppError::InternalError(
            status
                .error
                .unwrap_or_else(|| "The picture could not be drawn".into()),
        )),
    }
}

/// The queued job for a ratio picture, for the shared status poll.
pub fn status_job(
    ctx: &DatabaseContext,
    earlier: String,
    later: String,
    stretch: Option<RatioStretch>,
) -> Result<GenJob, AppError> {
    ratio_job(
        ctx,
        &RatioQuery {
            earlier,
            later,
            stretch,
        },
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::calibration::test_frames::write_frame;
    use crate::calibration::tests::{RulesInForce, POLICY_LOCK};

    /// A comparison of 15 spots lists the deepest 12 and counts all 15.
    #[test]
    fn spot_totals_count_every_spot_not_only_those_listed() {
        use seiza_calibration::dust::DustShape;
        let spot = |kind, shape, within_tolerance| metric::Spot {
            kind,
            darker: true,
            x: 1.0,
            y: 1.0,
            radius: 4.0,
            area: 50.0,
            peak: 0.01,
            shape,
            hole: None,
            bounds: [0; 4],
            within_tolerance,
            offset: None,
            distance_mm: None,
        };
        let mut spots = vec![spot(metric::SpotKind::New, DustShape::Compact, false); 12];
        spots.push(spot(metric::SpotKind::Gone, DustShape::Disk, false));
        spots.push(spot(metric::SpotKind::Moved, DustShape::Donut, false));
        spots.push(spot(metric::SpotKind::Moved, DustShape::Donut, true));
        let totals = SpotTotals::of(&spots);
        assert_eq!(listed_spots(&spots).len(), LISTED_SPOTS);
        assert_eq!(totals.listed, LISTED_SPOTS);
        assert_eq!(totals.counted, 14);
        assert_eq!(totals.within_tolerance, 1);
        assert_eq!(
            totals.by_kind,
            BTreeMap::from([("gone".into(), 1), ("moved".into(), 1), ("new".into(), 12)])
        );
        assert_eq!(
            totals.by_shape,
            BTreeMap::from([
                ("compact".into(), 12),
                ("disk".into(), 1),
                ("donut".into(), 1)
            ])
        );
    }

    /// Two mornings of three flats each, measured 2% apart.
    fn measured_catalog(dir: &std::path::Path) -> rusqlite::Connection {
        catalog_of(dir, &["2026-09-10T14:0", "2026-09-14T14:0"], |a, b| {
            if a.night == b.night {
                0.0
            } else {
                0.02
            }
        })
    }

    /// Mornings of three flats each, measured apart by `differ`.
    pub(crate) fn catalog_of(
        dir: &std::path::Path,
        mornings: &[&str],
        differ: impl Fn(&store::ResponseRow, &store::ResponseRow) -> f64,
    ) -> rusqlite::Connection {
        let metas: Vec<_> = mornings
            .iter()
            .flat_map(|morning| {
                (0..3).map(move |index| {
                    let path = dir.join(format!("{}-{index}.fits", &morning[..10]));
                    write_frame(
                        &path,
                        "FLAT",
                        (32, 32),
                        &format!("{morning}{index}:00"),
                        |_, _| 30_000.0,
                    );
                    crate::commands::import::headers::read_frame_meta(&path)
                })
            })
            .collect();
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        crate::calibration::import_calibration_frames(&tx, &metas, Some("profile")).unwrap();
        tx.commit().unwrap();
        flat_groups::testing::measure_as(&conn, differ);
        conn
    }

    #[test]
    fn a_group_is_keyed_by_its_oldest_session_and_lists_the_pairs_that_join_it() {
        let _lock = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        // 09-10 and 09-12 alike; 09-16 has new dust.
        let conn = catalog_of(
            temp.path(),
            &["2026-09-10T14:0", "2026-09-12T14:0", "2026-09-16T14:0"],
            |a, b| {
                if (a.night == "2026-09-16") != (b.night == "2026-09-16") {
                    0.02
                } else {
                    0.002
                }
            },
        );
        let library = crate::calibration::browse::library_frames(&conn).unwrap();
        let session_of = |night: &str| {
            library
                .frames
                .iter()
                .find(|row| row.night.as_deref() == Some(night))
                .map(|row| row.session.clone())
                .unwrap()
        };

        let view = build_view(&conn).unwrap();
        let family = &view.rigs[0].families[0];
        assert_eq!(family.groups.len(), 2);
        let (newer, older) = (&family.groups[0], &family.groups[1]);
        assert_eq!(older.key, session_of("2026-09-10"), "its oldest night");
        assert_eq!(newer.key, session_of("2026-09-16"));
        assert_eq!(older.pairs.len(), 1, "the two nights measured alike");
        let pair = &older.pairs[0];
        assert_eq!(
            (pair.earlier_night.as_str(), pair.later_night.as_str()),
            ("2026-09-10", "2026-09-12")
        );
        assert!((pair.dust_pct - 0.2).abs() < 1e-9, "{}", pair.dust_pct);
        assert!(newer.pairs.is_empty(), "one night has no pair");

        // A wider limit joins all three: the key stays with the oldest.
        let rig_uuid = library.frames[0].frame.rig_uuid.clone();
        let _rules = RulesInForce::with(
            &[],
            &[(
                rig_uuid.as_str(),
                "Scope",
                &[("flat_split_dust_pct", serde_json::json!(3.0))],
            )],
        );
        let view = build_view(&conn).unwrap();
        let groups = &view.rigs[0].families[0].groups;
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].key, session_of("2026-09-10"));
        assert!(groups[0].pairs.len() >= 2);
    }

    #[test]
    fn a_long_group_sends_its_neighbouring_pairs_and_the_widest() {
        let _lock = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        // Four alike nights; 09-10 and 09-14 differ most.
        let conn = catalog_of(
            temp.path(),
            &[
                "2026-09-10T14:0",
                "2026-09-11T14:0",
                "2026-09-12T14:0",
                "2026-09-13T14:0",
            ],
            |a, b| {
                let pair = [a.night.as_str(), b.night.as_str()];
                if pair.contains(&"2026-09-10") && pair.contains(&"2026-09-12") {
                    0.009
                } else {
                    0.002
                }
            },
        );
        let view = build_view(&conn).unwrap();
        let group = &view.rigs[0].families[0].groups[0];
        assert_eq!(group.sessions.len(), 4);
        let widest = group.widest.as_ref().expect("measured");
        assert!((widest.dust_pct - 0.9).abs() < 1e-9, "{}", widest.dust_pct);
        let nights: Vec<(&str, &str)> = group
            .pairs
            .iter()
            .map(|pair| (pair.earlier_night.as_str(), pair.later_night.as_str()))
            .collect();
        for adjacent in [
            ("2026-09-10", "2026-09-11"),
            ("2026-09-11", "2026-09-12"),
            ("2026-09-12", "2026-09-13"),
        ] {
            assert!(nights.contains(&adjacent), "{nights:?}");
        }
        assert!(nights.contains(&("2026-09-10", "2026-09-12")), "the widest");
        assert!(group.pairs.len() <= 5 && group.pairs.len() < widest.compared.max(6));
    }

    #[test]
    fn a_session_whose_flats_are_all_kept_out_is_placed_in_its_family() {
        let _lock = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let conn = catalog_of(
            temp.path(),
            &["2026-09-10T14:0", "2026-09-12T14:0", "2026-09-14T14:0"],
            |_, _| 0.002,
        );
        let library = crate::calibration::browse::library_frames(&conn).unwrap();
        let oldest = library
            .frames
            .iter()
            .find(|row| row.night.as_deref() == Some("2026-09-10"))
            .map(|row| row.session.clone())
            .unwrap();
        // Every flat of the oldest night kept out, and the groups worked
        // out again: no group holds that session any more.
        conn.execute(
            "INSERT OR REPLACE INTO psf_guard_calibration_frame_quality
                 (frame_uuid, override_choice, override_at, updated_at)
             SELECT frame_uuid, 'exclude', 1, 1 FROM psf_guard_calibration_frame
             WHERE frame_uuid IN (SELECT frame_uuid FROM psf_guard_calibration_frame
                                  WHERE captured_at < 1789100000)",
            [],
        )
        .unwrap();
        let rows = store::load_responses(&conn).unwrap();
        let rigs = rows.iter().map(|row| row.rig_uuid.clone()).collect();
        let keep: Vec<String> = rows
            .into_iter()
            .filter(|row| row.night != "2026-09-10")
            .map(|row| row.response_key)
            .collect();
        store::retain_responses(&conn, &rigs, &keep).unwrap();
        flat_groups::testing::measure_as(&conn, |_, _| 0.002);
        let view = build_view(&conn).unwrap();
        let family = &view.rigs[0].families[0];
        assert!(family
            .groups
            .iter()
            .flat_map(|group| &group.sessions)
            .all(|session| !session.sessions.contains(&oldest)));

        let place = place_of(&conn, &oldest).unwrap();
        assert_eq!(place.night.as_deref(), Some("2026-09-10"));
        let prefix = format!("{}:", place.family_key.expect("a family"));
        assert!(family.key.starts_with(&prefix), "{} / {prefix}", family.key);
        assert_eq!(
            place_of(&conn, "no such session").unwrap(),
            SessionPlace::default()
        );

        // A flat with no capture time is a session of its own, with no
        // night: it is still placed in its family.
        conn.execute(
            "UPDATE psf_guard_calibration_frame SET captured_at = NULL
             WHERE frame_uuid = (SELECT frame_uuid FROM psf_guard_calibration_frame
                                 WHERE captured_at > 1789100000 ORDER BY captured_at LIMIT 1)",
            [],
        )
        .unwrap();
        let undated = crate::calibration::browse::library_frames(&conn)
            .unwrap()
            .frames
            .into_iter()
            .find(|row| row.night.is_none())
            .map(|row| row.session)
            .expect("an undated session");
        let place = place_of(&conn, &undated).unwrap();
        assert_eq!(place.night, None);
        assert!(family
            .key
            .starts_with(&format!("{}:", place.family_key.expect("a family"))));
    }

    #[test]
    fn a_groups_sessions_are_found_by_its_key_or_any_of_its_sessions() {
        let _lock = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let conn = catalog_of(
            temp.path(),
            &["2026-09-10T14:0", "2026-09-12T14:0", "2026-09-16T14:0"],
            |a, b| {
                if (a.night == "2026-09-16") != (b.night == "2026-09-16") {
                    0.02
                } else {
                    0.002
                }
            },
        );
        let view = build_view(&conn).unwrap();
        let older = &view.rigs[0].families[0].groups[1];
        let members: BTreeSet<String> = older
            .sessions
            .iter()
            .flat_map(|session| session.sessions.iter().cloned())
            .collect();
        assert_eq!(members.len(), 2);
        assert_eq!(
            group_sessions(&conn, &older.key).unwrap(),
            Some(members.clone())
        );
        let newest_member = members.iter().max().unwrap();
        assert_eq!(
            group_sessions(&conn, newest_member).unwrap(),
            Some(members.clone())
        );
        let response = &older.sessions[0].response_key;
        assert_eq!(group_sessions(&conn, response).unwrap(), Some(members));
        assert_eq!(group_sessions(&conn, "gone").unwrap(), None);
    }

    #[test]
    fn a_session_two_families_share_keys_each_group_apart() {
        let session = |id: &str| SessionView {
            response_key: "r".into(),
            night: "2026-09-10".into(),
            first: 0,
            last: 0,
            frames: 3,
            combined: 3,
            noise_pct: None,
            bin: None,
            width: None,
            height: None,
            previous: None,
            sessions: vec![id.into(), "z".into()],
        };
        let none = HashSet::new();
        assert_eq!(group_key(&session("s1"), "R:3", &none), "s1");
        let shared = HashSet::from(["s1".to_string()]);
        let (a, b) = (
            group_key(&session("s1"), "R:3:g100", &shared),
            group_key(&session("s1"), "R:3:g0", &shared),
        );
        assert!(a.starts_with("s1~") && b.starts_with("s1~"));
        assert_ne!(a, b);
        assert_eq!(group_key(&session("s1"), "R:3:g100", &shared), a, "stable");
    }

    #[test]
    fn each_measured_session_names_the_browsers_session_and_the_rig_its_own_limits() {
        let _lock = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let conn = measured_catalog(temp.path());
        let library = crate::calibration::browse::library_frames(&conn).unwrap();
        let rig_uuid = library.frames[0].frame.rig_uuid.clone();

        let view = build_view(&conn).unwrap();
        let rig = &view.rigs[0];
        assert!(rig.own_limits.is_empty());
        let family = &rig.families[0];
        assert_eq!(family.groups.len(), 2, "split at the dust");
        for group in &family.groups {
            for session in &group.sessions {
                let expected: Vec<String> = library
                    .frames
                    .iter()
                    .filter(|row| row.night.as_deref() == Some(session.night.as_str()))
                    .map(|row| row.session.clone())
                    .take(1)
                    .collect();
                assert_eq!(session.sessions, expected, "{}", session.night);
            }
        }

        // A rig that sets its own dust limit says so.
        let _rules = RulesInForce::with(
            &[],
            &[(
                rig_uuid.as_str(),
                "Scope",
                &[("flat_split_dust_pct", serde_json::json!(3.0))],
            )],
        );
        let view = build_view(&conn).unwrap();
        assert_eq!(view.rigs[0].own_limits, vec!["flat_split_dust_pct"]);
        assert_eq!(view.rigs[0].families[0].groups.len(), 1, "2% is within 3%");
    }

    #[test]
    fn a_project_scope_keeps_only_the_families_its_sessions_touch() {
        let _lock = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let conn = measured_catalog(temp.path());
        let library = crate::calibration::browse::library_frames(&conn).unwrap();
        let first: BTreeSet<String> = library
            .frames
            .iter()
            .filter(|row| row.night.as_deref() == Some("2026-09-10"))
            .map(|row| row.session.clone())
            .collect();
        assert_eq!(first.len(), 1);

        let whole = build_view(&conn).unwrap();
        let events = whole.rigs[0].events.len();
        let view = scoped(build_view(&conn).unwrap(), &first);
        assert_eq!(view.rigs.len(), 1);
        assert_eq!(
            view.rigs[0].families.len(),
            1,
            "the family it touches, whole"
        );
        assert_eq!(view.rigs[0].families[0].groups.len(), 2);
        assert_eq!(
            view.rigs[0].events.len(),
            events,
            "its family's changes stay"
        );

        // Sessions no family holds leave nothing, not even the rig.
        let none = scoped(
            build_view(&conn).unwrap(),
            &BTreeSet::from(["elsewhere".to_string()]),
        );
        assert!(none.available && none.rigs.is_empty());
    }
}
