//! Night-by-night feasibility for a project at each rig that could shoot
//! it: dark hours, hours the target sits inside the limits Target Scheduler
//! will apply there, the Moon, and the curves behind the first night.
//! Planning estimates from the shared core; the rig still decides at
//! dispatch time.

use super::rig_availability::{NightAvailability, RigConditions};
use super::*;
use psf_guard_director_core::{
    night::{
        night_curve, night_summary, Night, NightCurve, NightDate, NightRequest, NightTarget,
        EARLIEST_START_MS, LATEST_START_MS,
    },
    visibility::{AltitudeLimits, Horizon, IcrsPosition, Site},
    windows::MeridianExclusion,
};
use psf_guard_director_meta::{
    activation::ActivatedRig,
    plan::Goal,
    preferences::SchedulingValues,
    profile::{Limits, RigProfile},
};
use rusqlite::{OpenFlags, OptionalExtension};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_NIGHTS: u32 = 7;
const STEP_MS: u64 = 300_000;
const DARK_BELOW_DEGREES: f64 = -12.0;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    #[serde(default)]
    nights: Option<u32>,
    /// The center to evaluate; defaults to the saved framing's center. A
    /// rig framed on a center of its own is timed there instead.
    #[serde(default)]
    center: Option<IcrsPosition>,
    /// Centers of rigs framed on a center of their own, as the framing view
    /// has them now, saved or not; a rig not named here uses its saved one.
    #[serde(default)]
    rig_centers: BTreeMap<Uuid, IcrsPosition>,
    #[serde(default)]
    start_ms: Option<u64>,
}

/// The limits a rig's nights were timed with: the rig profile's and the
/// plan's resolved scheduling limits, the tighter of each.
#[derive(Serialize)]
struct AppliedLimits {
    minimum_altitude_degrees: f64,
    maximum_altitude_degrees: f64,
    /// The rig's pause around the meridian.
    meridian_exclusion: MeridianExclusion,
    /// Degrees the plan adds to the custom horizon.
    horizon_offset_degrees: f64,
    /// Minutes either side of the meridian the plan images in; 0 is off.
    meridian_window_minutes: u32,
}

#[derive(Serialize)]
struct RigFeasibility {
    rig: NamedIdentity,
    catalog_name: String,
    site: Site,
    /// Where this rig was timed: its own framing's center, else the shared one.
    center: IcrsPosition,
    /// Whether the nights follow a custom horizon: the plan turns it on and
    /// the rig or its site has one.
    custom_horizon: bool,
    limits: AppliedLimits,
    nights: Vec<Night>,
    /// Each night's availability: the marks for the rig and its site, and
    /// the site's forecast. Alongside `nights`, one for one.
    availability: Vec<NightAvailability>,
    /// The first night in full, for the altitude chart.
    curve: NightCurve,
    /// Hours this rig still owes the plan: its enabled contributions' goals
    /// less the frames already accepted.
    hours_needed: Option<f64>,
    /// How many nights until it is done, counting each night's available
    /// hours with the target up, and nights past these as clear. The rig's
    /// other plans are not counted; `/projection` counts them.
    nights_to_complete: Option<u32>,
    /// Whether the plan has this rig ticked; unticked rigs still show.
    in_plan: bool,
}

#[derive(Serialize)]
pub(super) struct View {
    center: IcrsPosition,
    target_name: String,
    nights: u32,
    rigs: Vec<RigFeasibility>,
    warnings: Vec<String>,
}

pub(super) enum FeasibilityError {
    Api(Error),
    Rejected(String),
    NotReady(String),
}
impl From<Error> for FeasibilityError {
    fn from(error: Error) -> Self {
        Self::Api(error)
    }
}
impl From<StoreError> for FeasibilityError {
    fn from(error: StoreError) -> Self {
        Self::Api(error.into())
    }
}
impl IntoResponse for FeasibilityError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(error) => error.into_response(),
            Self::Rejected(message) => (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::error(message)),
            )
                .into_response(),
            Self::NotReady(message) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ApiResponse::<()>::error(message)),
            )
                .into_response(),
        }
    }
}

pub(super) async fn evaluate(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(request): Json<Request>,
) -> Result<Json<ApiResponse<View>>, FeasibilityError> {
    let service = enabled(&state)?;
    let nights = request.nights.unwrap_or(DEFAULT_NIGHTS);
    if nights == 0 || nights > 14 {
        return Err(Error::Invalid.into());
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let start_ms = request.start_ms.unwrap_or(now_ms);
    // The core times nights only in these years; past them the sums overflow.
    if !(EARLIEST_START_MS..=LATEST_START_MS).contains(&start_ms) {
        return Err(FeasibilityError::Rejected(
            "start_ms must fall between 1970-01-02 and 2099-01-01.".into(),
        ));
    }
    let catalogs: Vec<_> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let view = service.clone().with_reader(move |store| {
        // This holds one reader slot for as long as the night curves take;
        // writes and other reads go on beside it.
        store.project(id)?.ok_or(Error::Missing)?;
        let framing = store.framing_draft(id)?;
        let center = request
            .center
            .or_else(|| framing.as_ref().map(|f| f.center))
            .ok_or_else(|| FeasibilityError::NotReady("Frame the project or send a center first.".into()))?;
        let in_range = |c: &IcrsPosition| (0.0..360.0).contains(&c.ra_degrees) && (-90.0..=90.0).contains(&c.dec_degrees);
        if !in_range(&center) || request.rig_centers.len() > 64 || !request.rig_centers.values().all(in_range) {
            return Err(Error::Invalid.into());
        }
        let target_name = framing
            .as_ref()
            .map(|f| f.target_name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Target".into());
        let plan = store.plan_draft(id)?;
        let activation = store.activation(id)?;
        // A contribution covers every panel unless it names some; the rig
        // shoots each panel's frames, so the hours owed scale with panels,
        // its own grid when it is framed on its own.
        let panels_for = |rig_id: Uuid| -> Vec<String> {
            let Some(framing) = framing.as_ref() else {
                return vec!["r1c1".into()];
            };
            let layout = framing
                .rig_framing(rig_id)
                .map_or(&framing.layout, |own| &own.layout);
            layout.panels.iter().map(|panel| panel.id.clone()).collect()
        };
        // Every rig with a site; the plan's rigs first.
        let mut warnings = Vec::new();
        let mut rigs = Vec::new();
        let mut names = std::collections::BTreeMap::new();
        let found = identified_catalogs(&catalogs, service.instance_id);
        warnings.extend(found.duplicates.iter().cloned());
        for (identity, catalog) in found.iter() {
            if let Some(binding) = store.catalog_rig(identity.id)? {
                names.insert(binding.rig.id, (binding.rig, catalog.name.clone()));
            }
        }
        let mut inputs = Vec::new();
        for (rig_id, (rig, catalog_name)) in names {
            // A rig with no profile yet can still take its site's location
            // and horizon; its limits are the defaults until it has one.
            let profile = store.rig_profile(rig_id)?;
            let placed = store.rig_site(rig_id, profile.as_ref())?;
            let profile = profile.unwrap_or_else(|| RigProfile::empty(rig_id, 0));
            // Global, then this rig's site, the rig, and this project: the
            // limits activation writes into the rig's Target Scheduler project.
            let scheduling = store
                .effective_observing_preferences(rig_id, Some(id))?
                .scheduling
                .values;
            let contributions: Vec<_> = plan
                .as_ref()
                .map(|p| {
                    p.contributions
                        .iter()
                        .filter(|c| c.enabled && c.rig_id == rig_id)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            // Frames already accepted count against the goal. Before this rig
            // is activated none can have been taken for the plan.
            let activated = activation
                .as_ref()
                .and_then(|a| a.rigs.iter().find(|entry| entry.rig_id == rig_id));
            let accepted = match activated {
                Some(activated) if !contributions.is_empty() => found
                    .get(activated.catalog_id)
                    .and_then(|context| accepted_frames(context, activated).ok()),
                _ => Some(BTreeMap::new()),
            };
            let accepted = accepted.unwrap_or_else(|| {
                warnings.push(format!(
                    "{catalog_name}: the frames already taken could not be read, so the hours left count the whole goal."
                ));
                BTreeMap::new()
            });
            // A rig framed on its own center: where the view has it now,
            // else where it was saved.
            let own = framing.as_ref().and_then(|f| f.rig_framing(rig_id));
            let rig_center = own
                .and_then(|own| own.center.map(|_| request.rig_centers.get(&rig_id).copied()))
                .flatten()
                .or_else(|| own.and_then(|own| own.center))
                .unwrap_or(center);
            if placed.location.is_some()
                && !scheduling.use_custom_horizon
                && matches!(placed.horizon, Horizon::Custom { .. })
            {
                warnings.push(format!(
                    "{catalog_name}: Custom horizon is off in Scheduling limits, so Target Scheduler does not use the rig's horizon."
                ));
            }
            // The nights the summaries cover, and their marks and forecast.
            let mut dates = Vec::new();
            let mut conditions = None;
            if let Some(site) = placed.location
                && let Some(first) = NightDate::at(start_ms, site.longitude_degrees)
            {
                dates.push(first);
                while dates.len() < nights as usize {
                    let Some(next) = dates.last().and_then(|date| date.next()) else { break };
                    dates.push(next);
                }
                let last = *dates.last().unwrap_or(&first);
                conditions = Some(RigConditions::read(store, rig_id, &placed, first, last)?);
            }
            inputs.push(RigInput {
                rig,
                catalog_name,
                profile,
                placed,
                dates,
                conditions,
                scheduling,
                contributions,
                accepted,
                center: rig_center,
                panels: panels_for(rig_id),
            });
        }
        // Everything below is arithmetic on what was read: each rig, and
        // each of its nights, on its own thread of the shared pool, since a
        // night is thousands of SOFA transforms and the browser is waiting.
        use rayon::prelude::*;
        let outcomes: Vec<Result<RigFeasibility, String>> = inputs.into_par_iter().map(|input| {
            let RigInput { rig, catalog_name, profile, placed, dates, conditions, scheduling, contributions, accepted, center, panels } = input;
            let Some(site) = placed.location else {
                return Err(format!("{catalog_name} has no location. Choose a site in Rig setup."));
            };
            let available = conditions
                .as_ref()
                .filter(|_| dates.len() == nights as usize)
                .map(|conditions| conditions.nights(&dates, site, now_ms));
            let (altitude, applied) = match applied_limits(&profile.limits.value, &scheduling) {
                Ok(limits) => limits,
                Err(limits) => return Err(format!("{catalog_name}: the altitude limits leave nothing to image ({limits}).")),
            };
            // Target Scheduler follows the horizon only when the plan says so.
            let horizon = if scheduling.use_custom_horizon {
                placed.horizon
            } else {
                Horizon::FixedMinimum {}
            };
            let night_request = NightRequest {
                site,
                horizon: horizon.clone(),
                limits: altitude,
                targets: vec![NightTarget {
                    id: "center".into(),
                    position: center,
                    moon: vec![],
                }],
                start_ms,
                nights,
                step_ms: STEP_MS,
                dark_below_degrees: DARK_BELOW_DEGREES,
                meridian_exclusion: applied.meridian_exclusion,
                meridian_window_ms: u64::from(applied.meridian_window_minutes) * 60_000,
                available: available.as_ref().and_then(|nights| nights.available.clone()),
            };
            let computed: Vec<Result<(Night, Option<NightCurve>), _>> = (0..nights)
                .into_par_iter()
                .map(|index| if index == 0 {
                    night_curve(&night_request, 0).map(|curve| (curve.night.clone(), Some(curve)))
                } else {
                    night_summary(&night_request, index).map(|night| (night, None))
                })
                .collect();
            let mut nights_out = Vec::with_capacity(computed.len());
            let mut curve = None;
            for entry in computed {
                match entry {
                    Ok((night, drawn)) => {
                        if drawn.is_some() {
                            curve = drawn;
                        }
                        nights_out.push(night);
                    }
                    Err(error) => return Err(format!("{catalog_name}: feasibility could not be computed ({error:?}).")),
                }
            }
            let Some(curve) = curve else {
                return Err(format!("{catalog_name}: tonight's curve could not be computed."));
            };
            // The marks line up with the summaries by date; without marks,
            // each night is available in full.
            let mut marked = available.map(|nights| nights.nights.into_iter()).into_iter().flatten();
            let mut availability = Vec::with_capacity(nights_out.len());
            for (index, night) in nights_out.iter().enumerate() {
                let entry = marked.next().filter(|_| dates.get(index).is_some_and(|date| date.to_string() == night.date));
                availability.push(entry.map_or_else(NightAvailability::whole, |(_, shown)| shown));
            }
            let in_plan = !contributions.is_empty();
            let hours_needed = plan.as_ref().and_then(|p| {
                let mut total = 0.0;
                for c in &contributions {
                    let objective = p.objectives.iter().find(|o| o.id == c.objective_id)?;
                    let frames = match c.goal_for(objective) {
                        Goal::Hours { value } => (value * 3600.0 / c.exposure_seconds).ceil(),
                        Goal::Frames { value } => f64::from(value),
                    };
                    let shot = accepted.get(&c.id);
                    let left: f64 = if c.panel_ids.is_empty() { &panels } else { &c.panel_ids }
                        .iter()
                        .map(|panel| {
                            let taken = shot.and_then(|by_panel| by_panel.get(panel)).copied().unwrap_or(0);
                            (frames - f64::from(taken)).max(0.0)
                        })
                        .sum();
                    total += left * c.exposure_seconds / 3600.0;
                }
                in_plan.then_some(total)
            });
            // Each night's hours up and clear; tonight's from now on, since
            // the hours already past are gone.
            let mut usable: Vec<f64> = nights_out
                .iter()
                .map(|n| n.targets.first().map_or(0.0, |t| t.hours_up_available))
                .collect();
            if let Some(tonight) = usable.first_mut() {
                let windows = night_request.available.as_deref();
                let left = curve
                    .samples
                    .iter()
                    .filter(|s| {
                        s.t_ms >= start_ms
                            && s.sun_altitude_degrees < DARK_BELOW_DEGREES
                            && s.targets.first().is_some_and(|t| t.allowed && !t.meridian_blocked)
                            && windows.is_none_or(|w| w.iter().any(|w| s.t_ms >= w.start_ms && s.t_ms < w.end_ms))
                    })
                    .count() as f64
                    * STEP_MS as f64
                    / 3_600_000.0;
                *tonight = tonight.min(left);
            }
            // Between dawn and noon the night under way has ended: the count
            // starts with the coming night.
            let ended = nights_out
                .first()
                .is_some_and(|night| night.dawn_ms.is_some_and(|dawn| dawn <= start_ms)) as usize;
            let nights_to_complete = hours_needed
                .and_then(|needed| nights_until(needed, &usable[ended..], &nights_out[ended..]));
            Ok(RigFeasibility {
                rig,
                catalog_name,
                site,
                center,
                custom_horizon: matches!(horizon, Horizon::Custom { .. }),
                limits: applied,
                nights: nights_out,
                availability,
                curve,
                hours_needed,
                nights_to_complete,
                in_plan,
            })
        }).collect();
        for outcome in outcomes {
            match outcome {
                Ok(rig) => rigs.push(rig),
                Err(warning) => warnings.push(warning),
            }
        }
        rigs.sort_by(|a, b| b.in_plan.cmp(&a.in_plan).then_with(|| a.catalog_name.cmp(&b.catalog_name)));
        if rigs.is_empty() {
            warnings.push("No rig has a location yet.".into());
        }
        Ok::<_, FeasibilityError>(View {
            center,
            target_name,
            nights,
            rigs,
            warnings,
        })
    })
    .await?;
    Ok(Json(ApiResponse::success(view)))
}

/// What one rig's nights are timed from, read before the pool takes over.
struct RigInput {
    rig: NamedIdentity,
    catalog_name: String,
    profile: RigProfile,
    placed: psf_guard_director_meta::site_profile::RigSite,
    /// The nights the summaries cover, at the rig's longitude; empty
    /// without a location.
    dates: Vec<NightDate>,
    conditions: Option<RigConditions>,
    scheduling: SchedulingValues,
    contributions: Vec<psf_guard_director_meta::plan::Contribution>,
    /// Accepted frames by contribution, then panel.
    accepted: BTreeMap<Uuid, BTreeMap<String, u32>>,
    center: IcrsPosition,
    /// The panels of the grid this rig shoots.
    panels: Vec<String>,
}

/// How many nights until `needed` hours are shot: each night gives its
/// `usable` hours, up and clear. Past the nights summarised, nights count as
/// clear, at the average hours up of those nights. `None` when the target
/// is never up.
fn nights_until(needed: f64, usable: &[f64], nights: &[Night]) -> Option<u32> {
    if needed <= 0.0 {
        return Some(0);
    }
    let mut shot = 0.0;
    for (index, hours) in usable.iter().enumerate() {
        shot += hours;
        if shot + 1e-9 >= needed {
            return Some(index as u32 + 1);
        }
    }
    let clear = nights
        .iter()
        .map(|n| n.targets.first().map_or(0.0, |t| t.hours_up))
        .sum::<f64>()
        / nights.len().max(1) as f64;
    (clear > 0.05).then(|| nights.len() as u32 + ((needed - shot) / clear).ceil() as u32)
}

/// The rig profile's limits and the plan's scheduling limits together, as
/// Target Scheduler applies the plan's: the higher minimum, the lower
/// maximum (a plan maximum of 0 is none), the offset on the custom horizon,
/// and the meridian window. Err names the limits when they are not a range.
fn applied_limits(
    rig: &Limits,
    plan: &SchedulingValues,
) -> Result<(AltitudeLimits, AppliedLimits), String> {
    let altitude = combined_limits(
        rig,
        plan.minimum_altitude_degrees,
        plan.maximum_altitude_degrees,
        plan.horizon_offset_degrees,
    )?;
    let applied = AppliedLimits {
        minimum_altitude_degrees: altitude
            .rig_minimum_degrees
            .max(altitude.project_minimum_degrees),
        maximum_altitude_degrees: altitude
            .rig_maximum_degrees
            .min(altitude.project_maximum_degrees),
        meridian_exclusion: rig.meridian_exclusion,
        horizon_offset_degrees: altitude.horizon_offset_degrees,
        meridian_window_minutes: plan.meridian_window_minutes,
    };
    Ok((altitude, applied))
}

/// A rig profile's limits with one plan's, as Target Scheduler applies a
/// project's: the higher minimum, the lower maximum (a plan maximum of 0 is
/// none) and the offset on the custom horizon. Feasibility and night plans
/// count a plan's hours with it. Err names the limits when they are not a
/// range.
pub(super) fn combined_limits(
    rig: &Limits,
    minimum_degrees: f64,
    maximum_degrees: f64,
    horizon_offset_degrees: f64,
) -> Result<AltitudeLimits, String> {
    let altitude = AltitudeLimits {
        rig_minimum_degrees: rig.minimum_altitude_degrees,
        rig_maximum_degrees: rig.maximum_altitude_degrees,
        project_minimum_degrees: minimum_degrees,
        project_maximum_degrees: if maximum_degrees > 0.0 {
            maximum_degrees
        } else {
            90.0
        },
        // The core never lowers the custom horizon, so a negative offset
        // counts as none and can only undercount the hours.
        horizon_offset_degrees: horizon_offset_degrees.max(0.0),
    };
    altitude.validate().map_err(|_| {
        format!(
            "minimum {}°, maximum {}°",
            altitude
                .rig_minimum_degrees
                .max(altitude.project_minimum_degrees),
            altitude
                .rig_maximum_degrees
                .min(altitude.project_maximum_degrees)
        )
    })?;
    Ok(altitude)
}

/// Frames already accepted at one rig, by contribution and panel: the
/// Target Scheduler rows its activation wrote, found by GUID, counted as the
/// program the rig pulls counts them.
fn accepted_frames(
    context: &DatabaseContext,
    activated: &ActivatedRig,
) -> rusqlite::Result<BTreeMap<Uuid, BTreeMap<String, u32>>> {
    let connection = crate::server::database_context::open_scheduler_connection_with_flags(
        FilePath::new(&context.database_path),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let panels: BTreeMap<Uuid, &str> = activated
        .targets
        .iter()
        .map(|target| (target.target_guid, target.panel_id.as_str()))
        .collect();
    let mut statement =
        connection.prepare("SELECT IFNULL(accepted, 0) FROM exposureplan WHERE guid=?1")?;
    let mut found: BTreeMap<Uuid, BTreeMap<String, u32>> = BTreeMap::new();
    // A plan that sums several objectives' frames hands its accepted frames
    // out in record order, each taking up to its own frames, so they are
    // counted once.
    let mut left: BTreeMap<Uuid, u32> = BTreeMap::new();
    for plan in &activated.plans {
        let Some(panel) = panels.get(&plan.target_guid) else {
            continue;
        };
        let pool = match left.get(&plan.exposureplan_guid) {
            Some(pool) => *pool,
            None => {
                // A row deleted in Target Scheduler leaves nothing accepted.
                let accepted: i64 = statement
                    .query_row([plan.exposureplan_guid.to_string()], |row| row.get(0))
                    .optional()?
                    .unwrap_or(0);
                u32::try_from(accepted.max(0)).unwrap_or(u32::MAX)
            }
        };
        let shares = activated
            .plans
            .iter()
            .filter(|other| other.exposureplan_guid == plan.exposureplan_guid)
            .count();
        let mine = if shares > 1 {
            pool.min(plan.required_frames)
        } else {
            pool
        };
        left.insert(plan.exposureplan_guid, pool - mine);
        let count = found
            .entry(plan.contribution_id)
            .or_default()
            .entry((*panel).to_owned())
            .or_default();
        *count = count.saturating_add(mine);
    }
    Ok(found)
}
