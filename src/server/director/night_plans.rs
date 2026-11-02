//! Night plans: for every rig, or chosen rigs, over a range of nights, each
//! night's darkness, Moon and availability, and the hours each plan the rig
//! still owes sits inside the rig's limits. Planning estimates from the
//! shared core, computed on demand; nothing is stored and nothing is
//! written. See `docs/design/night-plans.md`.
//!
//! A rig's work is what Target Scheduler would plan there: the active
//! projects of its database's N.I.N.A. profile, with each project's limits
//! as Target Scheduler applies them. Director plans reach a rig through the
//! same rows, so a rig's summary does not depend on which runtime drives it.

use super::ts_snapshot;
use super::*;
use psf_guard_director_core::{
    moon::MoonPolicy,
    night::{
        night_summary, NightDate, NightRequest, NightTarget, EARLIEST_START_MS, LATEST_START_MS,
        MAX_NIGHTS, MAX_TARGETS,
    },
    visibility::{AltitudeLimits, Horizon, IcrsPosition, Site},
    windows::Interval,
};
use psf_guard_director_meta::{
    night_availability::{Availability, MarkScope, NightMark},
    preferences::SchedulingValues,
    profile::{Limits, RigProfile},
    site_profile::RigSite,
};
use psf_guard_ts_planner::snapshot::{Project, ProjectPriority, ProjectState, Snapshot};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_NIGHTS: u32 = 7;
const STEP_MS: u64 = 300_000;
/// Summaries count darkness as feasibility does: the Sun below −12°. Each
/// runtime's schedule applies its own rule.
const DARK_BELOW_DEGREES: f64 = -12.0;
/// The overhead each exposure adds, as the program pull counts it.
const OVERHEAD_SECONDS: f64 = 15.0;
const MAX_RIGS: usize = 64;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    /// Rigs to plan; none or empty is every rig with a site.
    #[serde(default)]
    rigs: Option<Vec<Uuid>>,
    /// The first night, the same for every rig: the evening's date at each
    /// rig's site. Without it, each rig starts with the night under way
    /// there at `start_ms`.
    #[serde(default)]
    first_night: Option<NightDate>,
    /// Without `first_night`, the first night is the one under way at this
    /// instant at each rig; now by default.
    #[serde(default)]
    start_ms: Option<u64>,
    #[serde(default)]
    nights: Option<u32>,
    /// Schedule this many of the nights, from the first, with the frames
    /// planned each night counted on the next. 0, the default, schedules
    /// none.
    #[serde(default)]
    schedule_nights: Option<u32>,
    /// The scheduled night whose blocks carry the planner's reasons: every
    /// candidate it looked at, and their scores.
    #[serde(default)]
    explain: Option<NightDate>,
    /// The runtime to schedule with; `auto`, the default, is the rig's own.
    #[serde(default)]
    runtime: Option<RuntimeChoice>,
    /// How closely a Target Scheduler schedule follows Target Scheduler's
    /// own preview.
    #[serde(default)]
    fidelity: Option<Fidelity>,
    /// The site's clock, minutes east of UTC, which Target Scheduler plans
    /// its days by. By default, local mean time at the site's longitude.
    #[serde(default)]
    utc_offset_minutes: Option<i32>,
    /// Schedules plan nothing before this instant: now by default, so the
    /// night under way is planned from now with the rig's current counts.
    /// `start_ms` only chooses the first night.
    #[serde(default)]
    plan_from_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum RuntimeChoice {
    #[default]
    Auto,
    TargetScheduler,
    Director,
}

/// How faithfully a Target Scheduler schedule follows Target Scheduler.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Fidelity {
    /// As a live run is expected to go: live sampling, time for slews,
    /// filter changes, dithers and downloads, and frames accepted at each
    /// plan's own rate.
    #[default]
    Estimate,
    /// As Target Scheduler's Scheduler Preview: exposure time only, and
    /// every frame accepted when the project grades.
    TargetSchedulerPreview,
}

/// What one schedule run used and assumed.
#[derive(Clone, Copy)]
struct ScheduleOptions {
    nights: u32,
    plan_from_ms: u64,
    explain: Option<NightDate>,
    runtime: RuntimeChoice,
    fidelity: Fidelity,
    utc_offset_minutes: Option<i32>,
}

/// Which runtime picks the rig's work on the night.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Runtime {
    TargetScheduler,
    Director,
}

#[derive(Serialize)]
pub(super) struct View {
    start_ms: u64,
    /// Schedules plan nothing before this instant.
    plan_from_ms: u64,
    nights: u32,
    rigs: Vec<RigNights>,
    /// Rigs left out, and why.
    warnings: Vec<String>,
    assumptions: Assumptions,
}

#[derive(Serialize)]
struct Assumptions {
    /// Sun altitude below which a summary counts the sky dark.
    dark_below_degrees: f64,
    /// Seconds each exposure adds to the hours a plan owes.
    overhead_seconds_per_exposure: f64,
    sample_minutes: u64,
}

#[derive(Serialize)]
struct RigNights {
    rig: NamedIdentity,
    catalog_slug: String,
    catalog_name: String,
    /// The planning site the rig names, whose marks it shares.
    planning_site: Option<NamedIdentity>,
    location: Site,
    runtime: Runtime,
    /// The N.I.N.A. profile planned in the rig's database.
    profile_id: Option<String>,
    plans: Vec<PlanView>,
    nights: Vec<RigNight>,
    /// What the rig would shoot, night by night, when schedules were asked
    /// for.
    schedule: Option<ScheduleView>,
    /// Revisions to send back with a mark: the rig's own, and its site's.
    mark_revisions: MarkRevisions,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct ScheduleView {
    /// The runtime the schedule follows.
    runtime: Runtime,
    /// One per scheduled night, in order, starting with the first.
    nights: Vec<ScheduledNight>,
    /// Each exposure plan's frames over the scheduled nights, and the night
    /// it completes.
    progress: Vec<psf_guard_ts_planner::PlanProgress>,
    assumptions: ScheduleAssumptions,
}

#[derive(Serialize)]
struct ScheduledNight {
    date: NightDate,
    #[serde(flatten)]
    schedule: psf_guard_ts_planner::NightSchedule,
}

#[derive(Serialize)]
struct ScheduleAssumptions {
    /// The Target Scheduler release the emulation follows.
    target_scheduler_version: Option<&'static str>,
    fidelity: Fidelity,
    overheads: Option<psf_guard_ts_planner::Overheads>,
    /// How planned frames count against a goal: `plan_rate`, each plan's
    /// acceptance so far, or `grader`, every frame when the project grades.
    acceptance: &'static str,
    /// Minutes east of UTC the planner's days follow.
    utc_offset_minutes: i32,
}

#[derive(Serialize)]
struct MarkRevisions {
    rig: u64,
    site: Option<u64>,
}

#[derive(Serialize)]
struct PlanView {
    /// The Target Scheduler project.
    project_id: i64,
    guid: Option<String>,
    name: String,
    priority: ProjectPriority,
    limits: PlanLimits,
    targets: Vec<PlanTarget>,
}

/// The limits a plan's hours were counted with: the rig's and the
/// project's, the tighter of each, as Target Scheduler applies them.
#[derive(Serialize)]
struct PlanLimits {
    minimum_altitude_degrees: f64,
    maximum_altitude_degrees: f64,
    horizon_offset_degrees: f64,
    custom_horizon: bool,
    meridian_window_minutes: u32,
}

#[derive(Serialize)]
struct PlanTarget {
    target_id: i64,
    name: String,
    position: IcrsPosition,
    /// Frames still owed over the target's enabled exposure plans.
    frames_owed: u32,
    /// Those frames' exposure and overhead time.
    hours_owed: f64,
    filters: Vec<String>,
}

#[derive(Serialize)]
struct RigNight {
    date: NightDate,
    noon_ms: u64,
    dusk_ms: Option<u64>,
    dawn_ms: Option<u64>,
    dark_hours: f64,
    available_dark_hours: f64,
    availability: NightAvailability,
    moon_illumination: f64,
    moon_hours_up_in_dark: f64,
    targets: Vec<TargetHours>,
    load: Load,
}

/// A night's effective availability and the marks that set it.
#[derive(Serialize)]
struct NightAvailability {
    state: Availability,
    /// The clear hours of a limited night: the site's and the rig's windows
    /// met. Empty otherwise.
    windows: Vec<Interval>,
    marks: Vec<AppliedMark>,
}

#[derive(Serialize)]
struct AppliedMark {
    scope: MarkScope,
    state: Availability,
    windows: Vec<Interval>,
    note: String,
}

#[derive(Serialize)]
struct TargetHours {
    project_id: i64,
    target_id: i64,
    /// Dark hours inside the limits.
    hours_up: f64,
    /// The same, inside the night's availability.
    hours_up_available: f64,
    /// Of those, the hours no owed filter's Moon rules allow.
    hours_moon_blocked: f64,
    max_altitude_degrees: f64,
    transit_ms: Option<u64>,
}

/// What the rig owes against what the night offers. A rough measure: the
/// targets' usable hours overlap, which only a schedule sorts out.
#[derive(Serialize)]
struct Load {
    /// Owed hours that fit the night: each target's owed time, up to the
    /// hours it is up, available and allowed by the Moon.
    demand_hours: f64,
    available_hours: f64,
    idle_hours: f64,
    overbooked_hours: f64,
}

pub(super) enum NightPlanError {
    Api(Error),
    Rejected(String),
}
impl From<Error> for NightPlanError {
    fn from(error: Error) -> Self {
        Self::Api(error)
    }
}
impl From<StoreError> for NightPlanError {
    fn from(error: StoreError) -> Self {
        Self::Api(error.into())
    }
}
impl IntoResponse for NightPlanError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(error) => error.into_response(),
            Self::Rejected(message) => (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::error(message)),
            )
                .into_response(),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// What one rig's nights are computed from, read before the pool takes over.
struct RigInput {
    rig: NamedIdentity,
    schedule: ScheduleOptions,
    /// The rig's Target Scheduler database, read once the store is let go.
    database_path: String,
    /// An instant inside the rig's first night.
    start_ms: u64,
    /// What a Director schedule runs from, when one is wanted.
    director: Option<Result<DirectorInput, String>>,
    catalog_slug: String,
    catalog_name: String,
    profile: RigProfile,
    placed: RigSite,
    runtime: Runtime,
    snapshot: Option<Snapshot>,
    dates: Vec<NightDate>,
    rig_marks: Vec<NightMark>,
    site_marks: Vec<NightMark>,
    mark_revisions: MarkRevisions,
    warnings: Vec<String>,
}

pub(super) async fn plan(
    State(state): State<Arc<AppState>>,
    Json(request): Json<Request>,
) -> Result<Json<ApiResponse<View>>, NightPlanError> {
    let service = enabled(&state)?;
    let nights = request.nights.unwrap_or(DEFAULT_NIGHTS);
    if nights == 0 || nights > MAX_NIGHTS {
        return Err(NightPlanError::Rejected(format!(
            "nights must be from 1 to {MAX_NIGHTS}."
        )));
    }
    let start_ms = request.start_ms.unwrap_or_else(now_ms);
    let options = ScheduleOptions {
        nights: request.schedule_nights.unwrap_or(0),
        plan_from_ms: request.plan_from_ms.unwrap_or_else(now_ms),
        explain: request.explain,
        runtime: request.runtime.unwrap_or_default(),
        fidelity: request.fidelity.unwrap_or_default(),
        utc_offset_minutes: request.utc_offset_minutes,
    };
    if options.nights > nights
        || request
            .utc_offset_minutes
            .is_some_and(|m| !(-14 * 60..=14 * 60).contains(&m))
    {
        return Err(NightPlanError::Rejected(
            "schedule_nights must be at most nights, and utc_offset_minutes within 14 hours of UTC.".into(),
        ));
    }
    if !(EARLIEST_START_MS..=LATEST_START_MS).contains(&start_ms)
        || !(EARLIEST_START_MS..=LATEST_START_MS).contains(&options.plan_from_ms)
    {
        return Err(NightPlanError::Rejected(
            "start_ms and plan_from_ms must fall between 1970-01-02 and 2099-01-01.".into(),
        ));
    }
    let first_night = request.first_night;
    let chosen: Option<std::collections::BTreeSet<Uuid>> = request
        .rigs
        .filter(|rigs| !rigs.is_empty())
        .map(|rigs| rigs.into_iter().collect());
    if chosen.as_ref().is_some_and(|rigs| rigs.len() > MAX_RIGS) {
        return Err(Error::Invalid.into());
    }
    let catalogs: Vec<_> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let gathered = service
        .clone()
        .with_reader(move |store| {
            let mut warnings = Vec::new();
            let found = identified_catalogs(&catalogs, service.instance_id);
            warnings.extend(found.duplicates.iter().cloned());
            let mut inputs = Vec::new();
            let mut seen = std::collections::BTreeSet::new();
            for (identity, catalog) in found.iter() {
                let Some(binding) = store.catalog_rig(identity.id)? else {
                    continue;
                };
                let rig = binding.rig;
                if chosen.as_ref().is_some_and(|rigs| !rigs.contains(&rig.id)) || !seen.insert(rig.id) {
                    continue;
                }
                let stored = store.rig_profile(rig.id)?;
                let placed = store.rig_site(rig.id, stored.as_ref())?;
                let Some(location) = placed.location else {
                    warnings.push(format!(
                        "{}: no site location; pick a site with one under Setup, type the rig's own, or let the plugin report it.",
                        catalog.name
                    ));
                    continue;
                };
                // Open question 1 of the design, until rig modes exist: a rig
                // that reported its equipment and runs an enabled workload
                // policy is Director's; every other rig is Target Scheduler's.
                let runtime = match (&stored, store.workload_policy(rig.id)?) {
                    (Some(profile), Some(policy)) if profile.configuration.is_some() && policy.enabled => Runtime::Director,
                    _ => Runtime::TargetScheduler,
                };
                let profile = stored.unwrap_or_else(|| RigProfile::empty(rig.id, 0));
                let rig_warnings = Vec::new();
                let wanted = options.nights > 0
                    && match options.runtime {
                        RuntimeChoice::Auto => runtime == Runtime::Director,
                        RuntimeChoice::Director => true,
                        RuntimeChoice::TargetScheduler => false,
                    };
                let director = wanted.then(|| director_input(store, &catalogs, service.instance_id, rig.id, identity.id));
                let first = match first_night {
                    Some(date) => date,
                    None => NightDate::at(start_ms, location.longitude_degrees).ok_or(Error::Invalid)?,
                };
                let mut dates = vec![first];
                while dates.len() < nights as usize {
                    let next = dates.last().and_then(|date| date.next()).ok_or(Error::Invalid)?;
                    dates.push(next);
                }
                let last = *dates.last().unwrap_or(&first);
                let rig_marks = store.night_marks(MarkScope::Rig, rig.id, first, last)?;
                let site_marks = placed
                    .site
                    .as_ref()
                    .map(|site| store.night_marks(MarkScope::Site, site.id, first, last))
                    .transpose()?;
                inputs.push(RigInput {
                    schedule: options,
                    director,
                    database_path: catalog.database_path.clone(),
                    // An hour after the night's noon: inside it at any longitude.
                    start_ms: first.span(location.longitude_degrees).start_ms + 3_600_000,
                    catalog_slug: catalog.id.clone(),
                    catalog_name: catalog.name.clone(),
                    profile,
                    placed,
                    runtime,
                    snapshot: None,
                    dates,
                    mark_revisions: MarkRevisions {
                        rig: rig_marks.revision,
                        site: site_marks.as_ref().map(|marks| marks.revision),
                    },
                    rig_marks: rig_marks.marks,
                    site_marks: site_marks.map(|marks| marks.marks).unwrap_or_default(),
                    warnings: rig_warnings,
                    rig,
                });
            }
            if let Some(chosen) = &chosen {
                for missing in chosen.iter().filter(|rig| !seen.contains(rig)) {
                    warnings.push(format!("Rig {missing} is not a registered rig, so it is left out."));
                }
            }
            Ok::<_, NightPlanError>((inputs, warnings))
        })
        .await?;
    // The rest reads each rig's database and computes, with the store's
    // read slot let go: each rig, and each of its requests and nights, on
    // the shared pool.
    let (inputs, mut warnings) = gathered;
    let view = blocking(move || {
        use rayon::prelude::*;
        let outcomes: Vec<Result<RigNights, String>> = inputs
            .into_par_iter()
            .map(|mut input| {
                match ts_snapshot::read(&input.database_path) {
                    Ok(Some(read)) => {
                        input.warnings.extend(read.warnings);
                        input.snapshot = Some(read.snapshot);
                    }
                    Ok(None) => input.warnings.push(
                        "The database has no N.I.N.A. profile yet, so there is nothing to plan.".into(),
                    ),
                    Err(error) => {
                        tracing::warn!(%error, catalog = %input.catalog_slug, "night plans could not read a rig database");
                        input
                            .warnings
                            .push("The rig's database could not be read, so its plans are left out.".into());
                    }
                }
                rig_nights(input, nights)
            })
            .collect();
        let mut rigs = Vec::new();
        for outcome in outcomes {
            match outcome {
                Ok(rig) => rigs.push(rig),
                Err(warning) => warnings.push(warning),
            }
        }
        rigs.sort_by(|a, b| a.catalog_name.cmp(&b.catalog_name));
        if rigs.is_empty() && warnings.is_empty() {
            warnings.push("No rig has a site yet, so there are no nights to plan.".into());
        }
        View {
            start_ms,
            plan_from_ms: options.plan_from_ms,
            nights,
            rigs,
            warnings,
            assumptions: Assumptions {
                dark_below_degrees: DARK_BELOW_DEGREES,
                overhead_seconds_per_exposure: OVERHEAD_SECONDS,
                sample_minutes: STEP_MS / 60_000,
            },
        }
    })
    .await?;
    Ok(Json(ApiResponse::success(view)))
}

/// The program the rig would pull, and each goal's limits.
struct DirectorInput {
    gathered: super::program::Gathered,
    /// Each goal's plan limits, by goal id: the plan's resolved scheduling
    /// limits, as feasibility applies them.
    limits: BTreeMap<String, SchedulingValues>,
}

/// One owed target of one project, ready for the core.
struct Owed {
    project_id: i64,
    target: NightTarget,
    target_id: i64,
    hours_owed: f64,
}

/// One core request: a project's limits and up to [`MAX_TARGETS`] of its
/// targets.
struct Chunk {
    request: NightRequest,
    owed: Vec<Owed>,
}

fn rig_nights(input: RigInput, nights: u32) -> Result<RigNights, String> {
    let RigInput {
        rig,
        schedule: options,
        director,
        database_path: _,
        start_ms,
        catalog_slug,
        catalog_name,
        profile,
        placed,
        runtime,
        snapshot,
        dates,
        rig_marks,
        site_marks,
        mark_revisions,
        mut warnings,
    } = input;
    let location = placed
        .location
        .ok_or_else(|| format!("{catalog_name}: no site location."))?;
    let availability: Vec<(Option<Vec<Interval>>, NightAvailability)> = dates
        .iter()
        .map(|date| effective(*date, location.longitude_degrees, &site_marks, &rig_marks))
        .collect();
    let available = if availability.iter().all(|(windows, _)| windows.is_none()) {
        None
    } else {
        Some(
            availability
                .iter()
                .zip(&dates)
                .flat_map(|((windows, _), date)| {
                    windows
                        .clone()
                        .unwrap_or_else(|| vec![date.span(location.longitude_degrees)])
                })
                .collect::<Vec<_>>(),
        )
    };
    let mut plans = Vec::new();
    let mut chunks = Vec::new();
    for project in snapshot
        .as_ref()
        .map(|s| s.projects.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|project| project.state == ProjectState::Active)
    {
        let Some(snapshot) = snapshot.as_ref() else {
            break;
        };
        let (limits, applied) = match project_limits(&profile.limits.value, project) {
            Ok(limits) => limits,
            Err(range) => {
                warnings.push(format!(
                    "{}: the altitude limits leave nothing to image ({range}).",
                    project.name
                ));
                continue;
            }
        };
        let horizon = if project.use_custom_horizon {
            placed.horizon.clone()
        } else {
            Horizon::FixedMinimum {}
        };
        let mut owed_targets = Vec::new();
        let mut targets = Vec::new();
        for target in project.targets.iter().filter(|t| t.active) {
            let mut frames = 0u32;
            let mut seconds = 0.0;
            let mut filters = Vec::new();
            let mut moon: Vec<MoonPolicy> = Vec::new();
            for plan in target.exposure_plans.iter().filter(|p| p.enabled) {
                let Some(template) = snapshot.templates.iter().find(|t| t.id == plan.template_id)
                else {
                    continue;
                };
                let left = plan.remaining(
                    project.enable_grader,
                    snapshot.preferences.exposure_throttle_percent,
                );
                if left == 0 {
                    continue;
                }
                frames += left;
                seconds += f64::from(left) * (plan.exposure_seconds(template) + OVERHEAD_SECONDS);
                if !filters.contains(&template.filter_name) {
                    filters.push(template.filter_name.clone());
                }
                if !moon.contains(&template.moon) {
                    moon.push(template.moon.clone());
                }
            }
            if frames == 0 {
                continue;
            }
            // A filter with Moon avoidance off can shoot whenever the
            // target is up, so it alone decides the Moon-free hours.
            if moon.iter().any(|policy| !policy.enabled) {
                moon.clear();
            }
            let hours_owed = seconds / 3600.0;
            targets.push(PlanTarget {
                target_id: target.id,
                name: target.name.clone(),
                position: target.position,
                frames_owed: frames,
                hours_owed,
                filters,
            });
            owed_targets.push(Owed {
                project_id: project.id,
                target_id: target.id,
                hours_owed,
                target: NightTarget {
                    id: target.id.to_string(),
                    position: target.position,
                    moon,
                },
            });
        }
        if targets.is_empty() {
            continue;
        }
        plans.push(PlanView {
            project_id: project.id,
            guid: project.guid.clone(),
            name: project.name.clone(),
            priority: project.priority,
            limits: PlanLimits {
                custom_horizon: matches!(horizon, Horizon::Custom { .. }),
                ..applied
            },
            targets,
        });
        let mut owed_targets = owed_targets.into_iter().peekable();
        while owed_targets.peek().is_some() {
            let owed: Vec<Owed> = owed_targets.by_ref().take(MAX_TARGETS).collect();
            chunks.push(Chunk {
                request: NightRequest {
                    site: location,
                    horizon: horizon.clone(),
                    limits,
                    targets: owed.iter().map(|o| o.target.clone()).collect(),
                    start_ms,
                    nights,
                    step_ms: STEP_MS,
                    dark_below_degrees: DARK_BELOW_DEGREES,
                    meridian_exclusion: profile.limits.value.meridian_exclusion,
                    meridian_window_ms: u64::from(project.meridian_window_minutes) * 60_000,
                    available: available.clone(),
                },
                owed,
            });
        }
    }
    // A night with no plans still has darkness and a Moon.
    if chunks.is_empty() {
        chunks.push(Chunk {
            request: NightRequest {
                site: location,
                horizon: Horizon::FixedMinimum {},
                limits: project_limits(&profile.limits.value, &bare_project())
                    .map(|(limits, _)| limits)
                    .map_err(|range| format!("{catalog_name}: the rig's altitude limits leave nothing to image ({range})."))?,
                targets: vec![],
                start_ms,
                nights,
                step_ms: STEP_MS,
                dark_below_degrees: DARK_BELOW_DEGREES,
                meridian_exclusion: profile.limits.value.meridian_exclusion,
                meridian_window_ms: 0,
                available: available.clone(),
            },
            owed: vec![],
        });
    }
    use rayon::prelude::*;
    let computed: Vec<Result<psf_guard_director_core::night::Night, _>> = (0..chunks.len()
        * nights as usize)
        .into_par_iter()
        .map(|index| {
            night_summary(
                &chunks[index / nights as usize].request,
                (index % nights as usize) as u32,
            )
        })
        .collect();
    let mut by_night: Vec<Vec<psf_guard_director_core::night::Night>> =
        (0..nights).map(|_| Vec::new()).collect();
    for (index, night) in computed.into_iter().enumerate() {
        let night = night.map_err(|error| {
            format!("{catalog_name}: the nights could not be computed ({error:?}).")
        })?;
        by_night[index % nights as usize].push(night);
    }
    let mut out = Vec::with_capacity(nights as usize);
    for ((summaries, (_, availability)), date) in by_night.into_iter().zip(availability).zip(&dates)
    {
        let Some(first) = summaries.first() else {
            return Err(format!("{catalog_name}: the nights could not be computed."));
        };
        if first.date != date.to_string() {
            return Err(format!("{catalog_name}: the nights did not line up."));
        }
        let mut targets = Vec::new();
        let mut demand_hours = 0.0;
        for (chunk, summary) in chunks.iter().zip(&summaries) {
            for (owed, hours) in chunk.owed.iter().zip(&summary.targets) {
                let usable = hours.hours_up_moon_allowed;
                demand_hours += owed.hours_owed.min(usable);
                targets.push(TargetHours {
                    project_id: owed.project_id,
                    target_id: owed.target_id,
                    hours_up: hours.hours_up,
                    hours_up_available: hours.hours_up_available,
                    hours_moon_blocked: (hours.hours_up_available - usable).max(0.0),
                    max_altitude_degrees: hours.max_altitude_degrees,
                    transit_ms: hours.transit_ms,
                });
            }
        }
        let available_hours = first.available_dark_hours;
        out.push(RigNight {
            date: *date,
            noon_ms: first.noon_ms,
            dusk_ms: first.dusk_ms,
            dawn_ms: first.dawn_ms,
            dark_hours: first.dark_hours,
            available_dark_hours: available_hours,
            availability,
            moon_illumination: first.moon_illumination,
            moon_hours_up_in_dark: first.moon_hours_up_in_dark,
            targets,
            load: Load {
                demand_hours,
                available_hours,
                idle_hours: (available_hours - demand_hours).max(0.0),
                overbooked_hours: (demand_hours - available_hours).max(0.0),
            },
        });
    }
    let schedule = if options.nights == 0 {
        None
    } else {
        let used = match options.runtime {
            RuntimeChoice::Auto => runtime,
            RuntimeChoice::TargetScheduler => Runtime::TargetScheduler,
            RuntimeChoice::Director => Runtime::Director,
        };
        match (used, snapshot.as_ref()) {
            (Runtime::TargetScheduler, Some(snapshot)) => Some(target_scheduler_schedule(
                snapshot,
                &profile,
                &placed,
                location,
                &dates[..options.nights as usize],
                available.as_deref(),
                options.plan_from_ms,
                options,
            )),
            (Runtime::TargetScheduler, None) => None,
            (Runtime::Director, _) => match director {
                Some(Ok(input)) => match director_schedule(
                    &input,
                    snapshot.as_ref(),
                    &profile,
                    &placed,
                    location,
                    &rig,
                    &dates[..options.nights as usize],
                    &out[..options.nights as usize],
                    available.as_deref(),
                    options.plan_from_ms,
                    options,
                ) {
                    Ok(view) => {
                        if input.gathered.preview_configuration {
                            warnings.push("The rig has not reported its equipment, so the Director schedule uses the filters, binning and camera settings its templates name.".into());
                        }
                        warnings.extend(input.gathered.omitted().iter().cloned());
                        Some(view)
                    }
                    Err(problem) => {
                        warnings.push(problem);
                        None
                    }
                },
                Some(Err(problem)) => {
                    warnings.push(problem);
                    None
                }
                None => None,
            },
        }
    };
    Ok(RigNights {
        rig,
        catalog_slug,
        catalog_name,
        planning_site: placed.site,
        location,
        runtime,
        profile_id: snapshot.map(|s| s.profile_id),
        plans,
        nights: out,
        schedule,
        mark_revisions,
        warnings,
    })
}

/// What Target Scheduler would shoot on the rig over the scheduled nights,
/// from its database's rows and the rig's N.I.N.A. settings.
#[allow(clippy::too_many_arguments)]
fn target_scheduler_schedule(
    snapshot: &Snapshot,
    profile: &RigProfile,
    placed: &RigSite,
    location: Site,
    dates: &[NightDate],
    available: Option<&[Interval]>,
    start_ms: u64,
    options: ScheduleOptions,
) -> ScheduleView {
    use psf_guard_ts_planner::{simulate, Clock, Estimate, Mode, NinaProfile, Overheads, Request};
    let offset_minutes = options
        .utc_offset_minutes
        .unwrap_or_else(|| (location.longitude_degrees / 15.0 * 60.0).round() as i32);
    let exclusion = profile.limits.value.meridian_exclusion;
    let mode = match options.fidelity {
        Fidelity::TargetSchedulerPreview => Mode::Preview,
        Fidelity::Estimate => Mode::Estimate(Estimate {
            overheads: Overheads::default(),
            acceptance: acceptance_rates(snapshot),
        }),
    };
    let overheads = match &mode {
        Mode::Estimate(estimate) => Some(estimate.overheads.clone()),
        Mode::Preview => None,
    };
    let simulation = simulate(&Request {
        snapshot,
        profile: NinaProfile {
            site: location,
            horizon: placed.horizon.clone(),
            pause_before_meridian_minutes: exclusion.before_ms as f64 / 60_000.0,
            minutes_after_meridian: exclusion.after_ms as f64 / 60_000.0,
        },
        clock: Clock {
            utc_offset_ms: i64::from(offset_minutes) * 60_000,
        },
        nights: dates
            .iter()
            .map(|date| date.span(location.longitude_degrees))
            .collect(),
        start_ms,
        available,
        mode,
    });
    let nights = dates
        .iter()
        .zip(simulation.nights)
        .map(|(date, mut schedule)| {
            if options.explain != Some(*date) {
                for block in &mut schedule.blocks {
                    block.decision.candidates.clear();
                }
            }
            ScheduledNight {
                date: *date,
                schedule,
            }
        })
        .collect();
    ScheduleView {
        runtime: Runtime::TargetScheduler,
        nights,
        progress: simulation.progress,
        assumptions: ScheduleAssumptions {
            target_scheduler_version: Some(psf_guard_ts_planner::TARGET_SCHEDULER_VERSION),
            fidelity: options.fidelity,
            overheads,
            acceptance: match options.fidelity {
                Fidelity::Estimate => "plan_rate",
                Fidelity::TargetSchedulerPreview => "grader",
            },
            utc_offset_minutes: offset_minutes,
        },
    }
}

/// Gather the rig's program as the pull would, for a Director schedule.
fn director_input(
    store: &MetaStore,
    catalogs: &[Arc<DatabaseContext>],
    instance: Uuid,
    rig: Uuid,
    catalog: Uuid,
) -> Result<DirectorInput, String> {
    let gathered = match super::program::gather(store, catalogs, instance, rig, catalog, true) {
        Ok(gathered) => gathered,
        Err(super::program::PullError::NotReady(reason)) => {
            return Err(format!("No Director schedule: {reason}"))
        }
        Err(_) => return Err("No Director schedule: the rig's program could not be read.".into()),
    };
    let mut limits = BTreeMap::new();
    let mut by_project: BTreeMap<Uuid, SchedulingValues> = BTreeMap::new();
    for link in gathered.goal_links() {
        let values = match by_project.get(&link.project_id) {
            Some(values) => values.clone(),
            None => {
                let values = store
                    .effective_observing_preferences(rig, Some(link.project_id))
                    .map_err(|_| {
                        "No Director schedule: the plan's limits could not be read.".to_string()
                    })?
                    .scheduling
                    .values;
                by_project.insert(link.project_id, values.clone());
                values
            }
        };
        limits.insert(link.goal_id.to_owned(), values);
    }
    Ok(DirectorInput { gathered, limits })
}

/// What Director would shoot on the rig: its own selection, stepped through
/// each night's dark hours.
#[allow(clippy::too_many_arguments)]
fn director_schedule(
    input: &DirectorInput,
    snapshot: Option<&Snapshot>,
    profile: &RigProfile,
    placed: &RigSite,
    location: Site,
    rig: &NamedIdentity,
    dates: &[NightDate],
    summaries: &[RigNight],
    available: Option<&[Interval]>,
    start_ms: u64,
    options: ScheduleOptions,
) -> Result<ScheduleView, String> {
    use psf_guard_director_core::{
        geometry::{Constraints, GoalLimits, RigConstraints, CONSTRAINTS_VERSION},
        night::planning_orientation,
        schedule::{simulate, GapReason as CoreGap, Overheads, ScheduleNight, ScheduleRequest},
    };
    use psf_guard_ts_planner::{
        Block, Decision, DecisionKind, FilterRun, Gap, GapReason, NightSchedule, PlanProgress,
        TargetRef,
    };
    let gathered = &input.gathered;
    if gathered.goals().is_empty() {
        return Err(format!(
            "No Director schedule: no activated plan on the rig has work left. {}",
            gathered.omitted().join("; ")
        ));
    }
    let nights: Vec<ScheduleNight> = dates
        .iter()
        .zip(summaries)
        .map(|(date, summary)| ScheduleNight {
            span: date.span(location.longitude_degrees),
            dark: summary
                .dusk_ms
                .zip(summary.dawn_ms)
                .map(|(start_ms, end_ms)| Interval { start_ms, end_ms }),
        })
        .collect();
    let (Some(first), Some(last)) = (nights.first(), nights.last()) else {
        return Err("No Director schedule: no nights.".into());
    };
    let limits = profile.limits.value;
    let constraints = Constraints {
        schema_version: CONSTRAINTS_VERSION,
        rig: RigConstraints {
            rig_id: rig.id.to_string(),
            configuration_id: gathered.configuration.id.clone(),
            revision: profile.revision.max(1),
            site: location,
            orientation: planning_orientation(first.span.start_ms, last.span.end_ms),
            horizon: placed.horizon.clone(),
            minimum_altitude_degrees: limits.minimum_altitude_degrees,
            maximum_altitude_degrees: limits.maximum_altitude_degrees,
            meridian_exclusion: limits.meridian_exclusion,
        },
        goals: gathered
            .goals()
            .iter()
            .map(|goal| {
                let plan = input.limits.get(&goal.id);
                GoalLimits {
                    goal_id: goal.id.clone(),
                    minimum_altitude_degrees: plan.map_or(0.0, |p| p.minimum_altitude_degrees),
                    maximum_altitude_degrees: plan
                        .map(|p| p.maximum_altitude_degrees)
                        .filter(|m| *m > 0.0)
                        .unwrap_or(90.0),
                    horizon_offset_degrees: plan.map_or(0.0, |p| p.horizon_offset_degrees.max(0.0)),
                }
            })
            .collect(),
    };
    let program = gathered.program(rig.id, first.span, "night-plan-preview".into());
    // Rows by GUID, for names and ids that match the summaries.
    let rows = snapshot.map(|snapshot| {
        let mut projects = BTreeMap::new();
        let mut targets = BTreeMap::new();
        let mut plans = BTreeMap::new();
        for project in &snapshot.projects {
            if let Some(guid) = &project.guid {
                projects.insert(
                    guid.to_ascii_lowercase(),
                    (project.id, project.name.clone()),
                );
            }
            for target in &project.targets {
                if let Some(guid) = &target.guid {
                    targets.insert(guid.to_ascii_lowercase(), (target.id, target.name.clone()));
                }
                for plan in &target.exposure_plans {
                    let filter = snapshot
                        .templates
                        .iter()
                        .find(|t| t.id == plan.template_id)
                        .map(|t| t.filter_name.clone());
                    if let Some(guid) = &plan.guid {
                        plans.insert(guid.to_ascii_lowercase(), (plan.id, filter, plan));
                    }
                }
            }
        }
        (projects, targets, plans)
    });
    let mut acceptance = BTreeMap::new();
    let rates = snapshot.map(acceptance_rates).unwrap_or_default();
    let mut references: BTreeMap<String, (TargetRef, i64, Option<String>)> = BTreeMap::new();
    for link in gathered.goal_links() {
        let project = rows
            .as_ref()
            .and_then(|(p, _, _)| p.get(&link.source_project_guid.to_string()).cloned());
        let target = rows
            .as_ref()
            .and_then(|(_, t, _)| t.get(&link.target_guid.to_string()).cloned());
        let plan = rows
            .as_ref()
            .and_then(|(_, _, p)| p.get(&link.exposureplan_guid.to_string()));
        // Director has no preview mode: frames always count at each plan's
        // own rate.
        if let Some((id, _, _)) = plan
            && let Some(rate) = rates.get(id)
        {
            acceptance.insert(link.goal_id.to_owned(), *rate);
        }
        references.insert(
            link.goal_id.to_owned(),
            (
                TargetRef {
                    project_id: project.as_ref().map_or(0, |p| p.0),
                    project_name: project.map_or_else(|| link.project_name.to_owned(), |p| p.1),
                    target_id: target.as_ref().map_or(0, |t| t.0),
                    target_name: target.map_or_else(|| link.target_guid.to_string(), |t| t.1),
                },
                plan.map_or(0, |p| p.0),
                plan.and_then(|p| p.1.clone()),
            ),
        );
    }
    let overheads = Overheads::default();
    let schedule = simulate(&ScheduleRequest {
        program: &program,
        constraints: &constraints,
        nights: &nights,
        start_ms,
        available,
        overheads,
        acceptance: &acceptance,
    })
    .map_err(|error| {
        format!("No Director schedule: the program did not bind for these nights ({error:?}).")
    })?;
    let reference = |goal: &str| references.get(goal).cloned();
    let mut scheduled = Vec::with_capacity(schedule.nights.len());
    for ((date, night), span) in dates.iter().zip(schedule.nights).zip(&nights) {
        let mut blocks: Vec<Block> = Vec::new();
        let mut frames = 0;
        let mut hours = 0.0;
        for exposure in &night.exposures {
            let Some((target, plan_id, filter)) = reference(&exposure.goal_id) else {
                continue;
            };
            frames += 1;
            hours += exposure.exposure_ms as f64 / 3_600_000.0;
            let filter = filter.unwrap_or_else(|| exposure.filter_id.clone());
            let continues = !exposure.target_change
                && blocks
                    .last()
                    .is_some_and(|b| b.target == target && b.end_ms == exposure.start_ms);
            if !continues {
                blocks.push(Block {
                    target: target.clone(),
                    start_ms: exposure.start_ms,
                    end_ms: exposure.end_ms,
                    slew: exposure.target_change,
                    runs: Vec::new(),
                    decision: Decision {
                        kind: DecisionKind::Ranked,
                        candidates: Vec::new(),
                        note: Some(format!(
                            "{}, priority {}",
                            exposure.reason.replace('_', " "),
                            exposure.priority
                        )),
                    },
                });
            }
            let Some(block) = blocks.last_mut() else {
                continue;
            };
            block.end_ms = exposure.end_ms;
            match block.runs.last_mut() {
                Some(run) if run.exposure_plan_id == plan_id && run.end_ms == exposure.start_ms => {
                    run.frames += 1;
                    run.dithers += u32::from(exposure.dither);
                    run.end_ms = exposure.end_ms;
                }
                _ => block.runs.push(FilterRun {
                    exposure_plan_id: plan_id,
                    filter,
                    exposure_seconds: exposure.exposure_ms as f64 / 1000.0,
                    frames: 1,
                    dithers: u32::from(exposure.dither),
                    start_ms: exposure.start_ms,
                    end_ms: exposure.end_ms,
                }),
            }
        }
        let gaps = night
            .gaps
            .iter()
            .map(|gap| Gap {
                start_ms: gap.start_ms,
                end_ms: gap.end_ms,
                reason: match gap.reason {
                    CoreGap::Twilight => GapReason::Twilight,
                    CoreGap::Unavailable => GapReason::Unavailable,
                    CoreGap::Moon => GapReason::Moon,
                    CoreGap::BelowLimits => GapReason::BelowLimits,
                    CoreGap::Complete => GapReason::Complete,
                    CoreGap::NothingEligible => GapReason::NothingEligible,
                },
                waiting_for: None,
            })
            .collect();
        scheduled.push(ScheduledNight {
            date: *date,
            schedule: NightSchedule {
                start_ms: span.span.start_ms,
                end_ms: span.span.end_ms,
                blocks,
                gaps,
                frames,
                exposure_hours: hours,
                note: None,
            },
        });
    }
    let progress = schedule
        .progress
        .iter()
        .filter_map(|goal| {
            let (target, plan_id, filter) = reference(&goal.goal_id)?;
            Some(PlanProgress {
                project_id: target.project_id,
                target_id: target.target_id,
                exposure_plan_id: plan_id,
                filter: filter.unwrap_or_default(),
                desired: goal.requested,
                accepted_before: f64::from(goal.accepted_before),
                acquired_before: f64::from(goal.accepted_before),
                planned: goal.planned.clone(),
                completes_on: goal.completes_on,
                complete_before: goal.accepted_before >= goal.requested,
            })
        })
        .collect();
    Ok(ScheduleView {
        runtime: Runtime::Director,
        nights: scheduled,
        progress,
        assumptions: ScheduleAssumptions {
            target_scheduler_version: None,
            fidelity: Fidelity::Estimate,
            overheads: Some(psf_guard_ts_planner::Overheads {
                per_exposure_seconds: 15.0,
                target_change_seconds: overheads.target_change_ms as f64 / 1000.0,
                filter_change_seconds: overheads.filter_change_ms as f64 / 1000.0,
                dither_seconds: overheads.dither_ms as f64 / 1000.0,
            }),
            acceptance: "plan_rate",
            utc_offset_minutes: options
                .utc_offset_minutes
                .unwrap_or_else(|| (location.longitude_degrees / 15.0 * 60.0).round() as i32),
        },
    })
}

/// Each graded exposure plan's share of frames accepted so far, once it has
/// a few frames to judge by.
fn acceptance_rates(snapshot: &Snapshot) -> std::collections::BTreeMap<i64, f64> {
    let mut rates = std::collections::BTreeMap::new();
    for project in snapshot.projects.iter().filter(|p| p.enable_grader) {
        for target in &project.targets {
            for plan in target.exposure_plans.iter().filter(|p| p.acquired >= 5) {
                rates.insert(
                    plan.id,
                    (f64::from(plan.accepted) / f64::from(plan.acquired)).clamp(0.05, 1.0),
                );
            }
        }
    }
    rates
}

/// A project with no limits of its own, for a rig's bare nights.
fn bare_project() -> Project {
    Project {
        id: 0,
        guid: None,
        name: String::new(),
        state: ProjectState::Active,
        priority: ProjectPriority::Normal,
        minimum_time_minutes: 0,
        minimum_altitude_degrees: 0.0,
        maximum_altitude_degrees: 0.0,
        use_custom_horizon: false,
        horizon_offset_degrees: 0.0,
        meridian_window_minutes: 0,
        filter_switch_frequency: 0,
        dither_every: 0,
        enable_grader: false,
        is_mosaic: false,
        smart_exposure_order: false,
        rule_weights: vec![],
        targets: vec![],
    }
}

/// The rig profile's limits and the project's together, by the rule
/// feasibility applies a plan's.
fn project_limits(rig: &Limits, project: &Project) -> Result<(AltitudeLimits, PlanLimits), String> {
    let altitude = super::feasibility::combined_limits(
        rig,
        project.minimum_altitude_degrees,
        project.maximum_altitude_degrees,
        project.horizon_offset_degrees,
    )?;
    let applied = PlanLimits {
        minimum_altitude_degrees: altitude
            .rig_minimum_degrees
            .max(altitude.project_minimum_degrees),
        maximum_altitude_degrees: altitude
            .rig_maximum_degrees
            .min(altitude.project_maximum_degrees),
        horizon_offset_degrees: altitude.horizon_offset_degrees,
        custom_horizon: project.use_custom_horizon,
        meridian_window_minutes: project.meridian_window_minutes,
    };
    Ok((altitude, applied))
}

/// A night's availability from the site's and the rig's marks: `None` for
/// the whole night, else the clear windows, empty when none.
fn effective(
    date: NightDate,
    longitude_degrees: f64,
    site: &[NightMark],
    rig: &[NightMark],
) -> (Option<Vec<Interval>>, NightAvailability) {
    let span = date.span(longitude_degrees);
    let mut marks = Vec::new();
    let mut windows: Option<Vec<Interval>> = None;
    for (scope, list) in [(MarkScope::Site, site), (MarkScope::Rig, rig)] {
        let Some(mark) = list.iter().find(|mark| mark.date == date) else {
            continue;
        };
        marks.push(AppliedMark {
            scope,
            state: mark.state,
            windows: mark.windows.clone(),
            note: mark.note.clone(),
        });
        let clear = match mark.state {
            Availability::Available => continue,
            Availability::Unavailable => vec![],
            Availability::Limited => mark
                .windows
                .iter()
                .filter_map(|w| {
                    let start_ms = w.start_ms.max(span.start_ms);
                    let end_ms = w.end_ms.min(span.end_ms);
                    (start_ms < end_ms).then_some(Interval { start_ms, end_ms })
                })
                .collect(),
        };
        windows = Some(match windows {
            None => clear,
            Some(previous) => intersect(&previous, &clear),
        });
    }
    let state = match &windows {
        None => Availability::Available,
        Some(clear) if clear.is_empty() => Availability::Unavailable,
        Some(_) => Availability::Limited,
    };
    let shown = match (&windows, state) {
        (Some(clear), Availability::Limited) => clear.clone(),
        _ => vec![],
    };
    (
        windows,
        NightAvailability {
            state,
            windows: shown,
            marks,
        },
    )
}

/// The instants in both sorted lists of apart windows.
fn intersect(a: &[Interval], b: &[Interval]) -> Vec<Interval> {
    let mut result = Vec::new();
    for x in a {
        for y in b {
            let start_ms = x.start_ms.max(y.start_ms);
            let end_ms = x.end_ms.min(y.end_ms);
            if start_ms < end_ms {
                result.push(Interval { start_ms, end_ms });
            }
        }
    }
    result.sort_by_key(|w| w.start_ms);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(date: NightDate, state: Availability, windows: Vec<Interval>) -> NightMark {
        NightMark {
            date,
            state,
            windows,
            note: String::new(),
            revision: 1,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn site_and_rig_marks_meet_and_a_mark_elsewhere_changes_nothing() {
        let date: NightDate = "2026-11-03".parse().unwrap();
        let span = date.span(-118.3);
        let hour = 3_600_000;
        let at = |from: u64, to: u64| Interval {
            start_ms: span.start_ms + from * hour,
            end_ms: span.start_ms + to * hour,
        };
        let (none, shown) = effective(date, -118.3, &[], &[]);
        assert_eq!((none, shown.state), (None, Availability::Available));
        // Clear sky, rig down: out.
        let (out, shown) = effective(
            date,
            -118.3,
            &[mark(date, Availability::Available, vec![])],
            &[mark(date, Availability::Unavailable, vec![])],
        );
        assert_eq!(out, Some(vec![]));
        assert_eq!(shown.state, Availability::Unavailable);
        assert_eq!(shown.marks.len(), 2);
        // The sky clears from 8 to 12 hours after noon and the rig can run
        // from 10 to 14: two hours.
        let (both, shown) = effective(
            date,
            -118.3,
            &[mark(date, Availability::Limited, vec![at(8, 12)])],
            &[mark(date, Availability::Limited, vec![at(10, 14)])],
        );
        assert_eq!(both, Some(vec![at(10, 12)]));
        assert_eq!(
            (shown.state, shown.windows),
            (Availability::Limited, vec![at(10, 12)])
        );
        // Windows that do not meet leave nothing.
        let (apart, shown) = effective(
            date,
            -118.3,
            &[mark(date, Availability::Limited, vec![at(8, 9)])],
            &[mark(date, Availability::Limited, vec![at(10, 14)])],
        );
        assert_eq!(
            (apart, shown.state),
            (Some(vec![]), Availability::Unavailable)
        );
        let other = date.next().unwrap();
        let (elsewhere, _) = effective(
            date,
            -118.3,
            &[],
            &[mark(other, Availability::Unavailable, vec![])],
        );
        assert_eq!(elsewhere, None);
    }

    #[test]
    fn frames_owed_follow_the_grader_and_the_exposure_throttle() {
        let plan = |desired, acquired, accepted| psf_guard_ts_planner::snapshot::ExposurePlan {
            id: 1,
            guid: None,
            template_id: 1,
            exposure_seconds: 300.0,
            desired,
            acquired,
            accepted,
            enabled: true,
        };
        // With the grader on, accepted frames count, however many were taken.
        assert_eq!(plan(10, 4, 1).remaining(true, 125.0), 9);
        assert_eq!(plan(10, 30, 5).remaining(true, 125.0), 5);
        // Without it, acquisitions count up to 125 % of the goal, truncated.
        assert_eq!(plan(10, 4, 1).remaining(false, 125.0), 8);
        assert_eq!(plan(10, 12, 0).remaining(false, 125.0), 0);
        assert_eq!(plan(3, 2, 0).remaining(false, 125.0), 1);
    }
}
