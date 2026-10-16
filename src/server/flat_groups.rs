//! The Calibration page's Flat groups view: each rig's families of flat
//! sessions, the groups they form, why neighbouring groups split, the
//! rig's change events and its angle term; and the ratio picture of two
//! sessions, drawn by the shared preview queue.

use crate::calibration::flat_groups::{self, job, metric, model, store};
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
use std::collections::{BTreeMap, BTreeSet, HashMap};
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

#[derive(Debug, Serialize)]
pub struct SpotView {
    pub kind: metric::SpotKind,
    pub darker: bool,
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub peak_pct: f64,
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
        }
    }
}

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
    /// Why, in words, with the values and the limits.
    pub reason: String,
    /// The change event this split belongs to.
    pub event_key: Option<String>,
    pub spots: Vec<SpotView>,
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
    /// Every pair of its sessions measured against each other, oldest
    /// first: why they share a group.
    pub pairs: Vec<MemberPairView>,
}

#[derive(Debug, Serialize)]
pub struct FamilyView {
    pub key: String,
    pub filter: Option<String>,
    pub rotation: Option<f64>,
    /// Newest first; `splits[i]` lies between `groups[i + 1]` and
    /// `groups[i]`.
    pub groups: Vec<GroupView>,
    pub splits: Vec<SplitView>,
}

#[derive(Debug, Serialize)]
pub struct EventView {
    pub key: String,
    pub start: i64,
    pub end: i64,
    pub what: String,
    pub dust_pct: f64,
    pub broad_pct: f64,
    pub camera_side: bool,
    pub filters: Vec<String>,
    pub rotations: Vec<Option<f64>>,
    pub spots: Vec<SpotView>,
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
    /// Newest first.
    pub events: Vec<EventView>,
    pub families: Vec<FamilyView>,
    pub unmeasured: Vec<UnmeasuredView>,
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
/// algorithm, and a regroup that keeps the oldest member keeps it.
pub(crate) fn group_key(oldest: &SessionView) -> String {
    oldest
        .sessions
        .iter()
        .min()
        .cloned()
        .unwrap_or_else(|| oldest.response_key.clone())
}

fn read_only(path: &str) -> Result<rusqlite::Connection, AppError> {
    crate::server::database_context::open_scheduler_connection_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(AppError::db)
}

pub(crate) fn build_view(conn: &rusqlite::Connection) -> anyhow::Result<FlatGroupsView> {
    if !store::tables_exist(conn) {
        return Ok(FlatGroupsView {
            available: false,
            rigs: Vec::new(),
        });
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
    let rule_book = crate::calibration::rules::rule_book();
    let mut rigs: BTreeMap<String, RigView> = BTreeMap::new();
    let rig_view = |rig: &str, book: &model::Book| {
        let rules = crate::calibration::rules_for_rig(rig);
        let limits = flat_groups::limits_of(&rules);
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
            events: Vec::new(),
            families: Vec::new(),
            unmeasured: Vec::new(),
        }
    };
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
    for family in &book.families {
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
            let mut pairs = Vec::new();
            for (position, earlier) in group.iter().enumerate() {
                for later in &group[position + 1..] {
                    if let Some(c) = book.comparison(*earlier, *later) {
                        let (a, b) = (&book.sessions[*earlier].row, &book.sessions[*later].row);
                        pairs.push(MemberPairView {
                            earlier_response: a.response_key.clone(),
                            later_response: b.response_key.clone(),
                            earlier_night: a.night.clone(),
                            later_night: b.night.clone(),
                            dust_pct: pct(c.dust),
                            broad_pct: pct(c.broad),
                        });
                    }
                }
            }
            let key = group_key(&sessions[0]);
            sessions.reverse();
            groups.push(GroupView {
                key,
                pairs,
                first_night: book.sessions[group[0]].row.night.clone(),
                last_night: book.sessions[*group.last().expect("not empty")]
                    .row
                    .night
                    .clone(),
                frames,
                sessions,
            });
        }
        let mut splits: Vec<SplitView> = family
            .boundaries
            .iter()
            .map(|b| {
                let boundary = &book.boundaries[*b];
                SplitView {
                    before_response: book.sessions[boundary.before].row.response_key.clone(),
                    after_response: book.sessions[boundary.after].row.response_key.clone(),
                    before_night: book.sessions[boundary.before].row.night.clone(),
                    after_night: book.sessions[boundary.after].row.night.clone(),
                    start: boundary.start,
                    end: boundary.end,
                    dust_pct: boundary.dust.map(pct),
                    broad_pct: boundary.broad.map(pct),
                    reason: split_reason(&book, *b, boundary, &limits),
                    event_key: boundary_event.get(b).cloned(),
                    spots: boundary.spots.iter().take(12).map(SpotView::of).collect(),
                }
            })
            .collect();
        groups.reverse();
        splits.reverse();
        rig.families.push(FamilyView {
            key: family.key.clone(),
            filter: family.filter.clone(),
            rotation: family.rotation,
            groups,
            splits,
        });
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
            what: flat_groups::describe_spots(&event.spots, event.dust, event.broad),
            dust_pct: pct(event.dust),
            broad_pct: pct(event.broad),
            camera_side: event.camera_side,
            filters: event.filters.iter().map(|f| f.to_uppercase()).collect(),
            rotations: event.rotations.clone(),
            spots: event.spots.iter().take(12).map(SpotView::of).collect(),
            before_response,
            after_response,
            within_flat_run: event.within_flat_run,
        });
    }
    // Sessions the pass could not measure, or has not reached.
    let library = crate::calibration::LibrarySnapshot::load(conn)?;
    let boundary = crate::calibration::catalog_night_boundary(conn);
    for plan in job::plan_sessions(library.frames(), boundary) {
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
    Ok(FlatGroupsView {
        available: true,
        rigs: rigs.into_values().collect(),
    })
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
