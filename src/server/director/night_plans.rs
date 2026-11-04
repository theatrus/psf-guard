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

use super::rig_availability::{NightAvailability, RigConditions, RigForecast};
use super::ts_snapshot;
use super::*;
use crate::concurrency::Stop;
use psf_guard_director_core::{
    ephemeris::{moon_at_site, separation_degrees},
    moon::MoonPolicy,
    night::{
        night_summary, planning_orientation, NightDate, NightRequest, NightTarget,
        EARLIEST_START_MS, LATEST_START_MS, MAX_NIGHTS, MAX_TARGETS,
    },
    visibility::{observe, AltitudeLimits, Horizon, IcrsPosition, Site},
    windows::Interval,
};
use psf_guard_director_meta::night_override::{check, NightOverride};
use psf_guard_director_meta::{
    preferences::SchedulingValues,
    profile::{Limits, RigProfile},
    site_forecast::ForecastHour,
    site_profile::RigSite,
};
use psf_guard_ts_planner::snapshot::{Project, ProjectPriority, ProjectState, Snapshot};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

mod cache;
pub(super) use cache::{Cache, REFRESH_EVERY};

const DEFAULT_NIGHTS: u32 = 7;
const STEP_MS: u64 = 300_000;
/// Summaries count darkness as feasibility does: the Sun below −12°. Each
/// runtime's schedule applies its own rule.
const DARK_BELOW_DEGREES: f64 = -12.0;
/// The overhead each exposure adds, as the program pull counts it.
const OVERHEAD_SECONDS: f64 = 15.0;
const MAX_RIGS: usize = 64;

#[derive(Deserialize, Default, Clone)]
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
    /// Whether each night carries the Moon's altitude curve for drawing;
    /// on by default. The response leaves it out when false, so it does
    /// not change what is kept.
    #[serde(default)]
    moon_altitudes: Option<bool>,
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
    /// An unsaved edit of one Director rig's overrides for one night,
    /// planned in place of the night's stored list. Nothing is stored.
    #[serde(default)]
    override_draft: Option<OverrideDraft>,
    /// Unsaved changes to one Target Scheduler rig's projects and targets,
    /// planned in place of what its database holds. Nothing is written.
    #[serde(default)]
    ts_draft: Option<TsDraft>,
    /// Compute the plans again, whatever is kept.
    #[serde(default)]
    refresh: bool,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(super) struct TsDraft {
    rig: Uuid,
    changes: super::ts_levers::TsChanges,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(super) struct OverrideDraft {
    rig: Uuid,
    date: NightDate,
    entries: Vec<NightOverride>,
}

/// A night's overrides as a schedule applied them.
#[derive(Clone, Serialize)]
struct AppliedOverrides {
    /// The stored list's revision; `None` for an unsaved draft.
    revision: Option<u64>,
    entries: Vec<NightOverride>,
    /// Whether this is the request's unsaved draft rather than the stored
    /// list.
    draft: bool,
}

/// A goal of a Director schedule, by the ids its blocks use and the GUIDs
/// an override names it by.
#[derive(Serialize)]
struct DirectorGoal {
    goal_id: String,
    /// The plan, as overrides and plan orders name it.
    plan_id: Uuid,
    plan_name: String,
    target_guid: Uuid,
    exposureplan_guid: Uuid,
    /// The rig database's rows, as the schedule's blocks name them.
    project_id: i64,
    target_id: i64,
    exposure_plan_id: i64,
    target_name: String,
    filter: Option<String>,
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
    /// The rig databases the plans read, by registry id: a change to one
    /// of them changes the plans.
    #[serde(skip)]
    catalogs: Vec<String>,
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
    /// The cloud forecast the rig's planning site takes, if it takes one.
    forecast: Option<RigForecast>,
    /// The planning site's IANA time zone, in which marks and overrides are
    /// entered and shown; `None` uses the browser's.
    time_zone: Option<String>,
    /// Every project of the planned profile, in any state, as the rig's
    /// database holds it, for the night's Target Scheduler controls.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ts_projects: Vec<super::ts_levers::TsProjectView>,
    /// Where saving those controls writes, or why it cannot.
    ts_save: super::ts_levers::TsSaveView,
    warnings: Vec<String>,
    /// Every project of the planned profile, owed or not, for projections.
    #[serde(skip)]
    projects: Vec<KnownProject>,
}

/// A project in the rig's database, as a projection looks it up.
struct KnownProject {
    id: i64,
    guid: Option<Uuid>,
    active: bool,
    /// Its exposure plans, enabled or not.
    plans: Vec<KnownPlan>,
}

struct KnownPlan {
    id: i64,
    guid: Option<Uuid>,
    /// Frames still owed, as Target Scheduler counts them.
    remaining: u32,
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
    /// A Director schedule's goals, so an override can name what a block
    /// runs. Empty for Target Scheduler.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    goals: Vec<DirectorGoal>,
}

#[derive(Serialize)]
struct ScheduledNight {
    date: NightDate,
    #[serde(flatten)]
    schedule: psf_guard_ts_planner::NightSchedule,
    /// The Director rig's overrides the night was planned with.
    #[serde(skip_serializing_if = "Option::is_none")]
    overrides: Option<AppliedOverrides>,
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
    /// The Moon's altitude from noon to noon, for its rise and set.
    moon_altitudes: MoonAltitudes,
    targets: Vec<TargetHours>,
    load: Load,
    /// The forecast's cloud cover through the night; empty without one.
    clouds: Vec<ForecastHour>,
    /// How many overrides a Director rig has for the night; 0 on any other
    /// rig.
    override_count: usize,
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

#[derive(Clone, Debug)]
pub(super) enum NightPlanError {
    Api(Error),
    Rejected(String),
    /// Stopped from the header, or replaced by a newer request for the
    /// same edited night.
    Stopped,
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
            Self::Stopped => (
                StatusCode::CONFLICT,
                Json(ApiResponse::<()>::error(
                    "The night plans were stopped, or replaced by a newer request.".into(),
                )),
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
    /// The marks and forecast the nights are planned against.
    conditions: RigConditions,
    /// A Director rig's overrides by night, with any draft in place of its
    /// night's stored list.
    overrides: BTreeMap<NightDate, AppliedOverrides>,
    time_zone: Option<String>,
    /// The request's unsaved Target Scheduler changes for this rig.
    ts_changes: Option<super::ts_levers::TsChanges>,
    ts_projects: Vec<super::ts_levers::TsProjectView>,
    ts_save: super::ts_levers::TsSaveView,
    warnings: Vec<String>,
}

/// Who picks the rig's work: the mode set in Rig setup, or by pairing a
/// Director plugin. Until one is set, a guess (open question 1 of the
/// design): a rig that reported its equipment and runs an enabled workload
/// policy is Director's, and every other rig Target Scheduler's. Overrides
/// and Target Scheduler settings follow it.
pub(super) fn runtime_of(
    store: &MetaStore,
    rig: Uuid,
    profile: Option<&RigProfile>,
) -> Result<Runtime, StoreError> {
    match profile.and_then(|profile| profile.mode) {
        Some(psf_guard_director_meta::profile::RigMode::Director) => return Ok(Runtime::Director),
        Some(psf_guard_director_meta::profile::RigMode::TargetScheduler) => {
            return Ok(Runtime::TargetScheduler)
        }
        None => {}
    }
    Ok(match (profile, store.workload_policy(rig)?) {
        (Some(profile), Some(policy)) if profile.configuration.is_some() && policy.enabled => {
            Runtime::Director
        }
        _ => Runtime::TargetScheduler,
    })
}

/// The night plans, when they were computed, and whether they are being
/// computed again.
#[derive(Serialize)]
pub(super) struct Planned {
    #[serde(flatten)]
    view: SharedView,
    computed_at_ms: u64,
    refreshing: bool,
}

/// A kept view, written as the view itself.
struct SharedView(Arc<View>);

impl Serialize for SharedView {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

pub(super) async fn plan(
    State(state): State<Arc<AppState>>,
    Json(request): Json<Request>,
) -> Result<Response, NightPlanError> {
    let curves = request.moon_altitudes != Some(false);
    let kept = planned(&state, request).await?;
    let planned = Planned {
        view: SharedView(kept.view),
        computed_at_ms: kept.computed_at_ms,
        refreshing: kept.refreshing,
    };
    if curves {
        return Ok(Json(ApiResponse::success(planned)).into_response());
    }
    let value = serde_json::to_value(&planned).map_err(|_| NightPlanError::Api(Error::Internal))?;
    Ok(Json(ApiResponse::success(without_moon_altitudes(value))).into_response())
}

/// The night plans without each night's Moon curve, for a reader that does
/// not draw it.
fn without_moon_altitudes(mut value: serde_json::Value) -> serde_json::Value {
    let rigs = value
        .get_mut("rigs")
        .and_then(serde_json::Value::as_array_mut);
    for rig in rigs.into_iter().flatten() {
        let nights = rig
            .get_mut("nights")
            .and_then(serde_json::Value::as_array_mut);
        for night in nights.into_iter().flatten() {
            if let Some(night) = night.as_object_mut() {
                night.remove("moon_altitudes");
            }
        }
    }
    value
}

/// The Moon's altitude at even steps from `start_ms`.
#[derive(Serialize)]
struct MoonAltitudes {
    start_ms: u64,
    step_ms: u64,
    /// Degrees, to a tenth.
    degrees: Vec<f64>,
}

const MOON_ALTITUDE_STEP_MS: u64 = 15 * 60_000;

/// The Moon's altitude as the site sees it, parallax included, every
/// quarter hour from one noon to the next, for the night view's Moon
/// strip. Empty when the ephemeris cannot place it.
fn moon_altitudes(noon_ms: u64, site: Site) -> MoonAltitudes {
    let degrees = (0..24 * 3_600_000 / MOON_ALTITUDE_STEP_MS)
        .map(|step| {
            let at = noon_ms + step * MOON_ALTITUDE_STEP_MS;
            let moon = moon_at_site(at, site)?;
            let seen = observe(moon, site, planning_orientation(at, at), at).ok()?;
            Some((seen.altitude_degrees * 10.0).round() / 10.0)
        })
        .collect::<Option<Vec<f64>>>()
        .unwrap_or_default();
    MoonAltitudes {
        start_ms: noon_ms,
        step_ms: MOON_ALTITUDE_STEP_MS,
        degrees,
    }
}

/// The night plans a request asks for, kept or computed now as a person's
/// work; projections use it too.
async fn planned(state: &Arc<AppState>, request: Request) -> Result<cache::Kept, NightPlanError> {
    let service = enabled(state)?;
    let refresh = request.refresh;
    service
        .night_plans
        .plan(state, &service, request, refresh)
        .await
}

/// Compute again the kept views someone read lately, when what they were
/// computed from changed or they aged.
pub(super) async fn refresh_kept(state: Arc<AppState>) {
    if let Ok(service) = enabled(&state) {
        service.night_plans.refresh(&state, &service).await;
    }
}

/// Compute the night plans a request asks for. `stop`, set from the
/// header or by a newer request, ends the work early.
async fn compute(
    state: &Arc<AppState>,
    request: Request,
    stop: &Stop,
) -> Result<View, NightPlanError> {
    let service = enabled(state)?;
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
    let draft = request.override_draft;
    let ts_draft = request.ts_draft;
    let database_management = state.database_management_allowed();
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
                if chosen.as_ref().is_some_and(|rigs| !rigs.contains(&rig.id))
                    || !seen.insert(rig.id)
                {
                    continue;
                }
                let stored = store.rig_profile(rig.id)?;
                let runtime = runtime_of(store, rig.id, stored.as_ref())?;
                let draft = draft.as_ref().filter(|draft| draft.rig == rig.id);
                if draft.is_some() && runtime != Runtime::Director {
                    return Err(Error::Mode(NOT_DIRECTOR.into()).into());
                }
                let placed = store.rig_site(rig.id, stored.as_ref())?;
                let Some(location) = placed.location else {
                    warnings.push(format!(
                        "{} has no location. Choose a site in Rig setup.",
                        catalog.name
                    ));
                    continue;
                };
                let ts_save = super::ts_levers::TsSaveView::of(
                    catalog,
                    stored.as_ref().and_then(|p| p.peer_id.as_deref()),
                    database_management,
                );
                let ts_changes = ts_draft
                    .as_ref()
                    .filter(|draft| draft.rig == rig.id)
                    .map(|draft| draft.changes.clone());
                let profile = stored.unwrap_or_else(|| RigProfile::empty(rig.id, 0));
                let rig_warnings = Vec::new();
                let wanted = options.nights > 0
                    && match options.runtime {
                        RuntimeChoice::Auto => runtime == Runtime::Director,
                        RuntimeChoice::Director => true,
                        RuntimeChoice::TargetScheduler => false,
                    };
                let director = wanted.then(|| {
                    director_input(store, &catalogs, service.instance_id, rig.id, identity.id)
                });
                let first = match first_night {
                    Some(date) => date,
                    None => {
                        NightDate::at(start_ms, location.longitude_degrees).ok_or(Error::Invalid)?
                    }
                };
                let mut dates = vec![first];
                while dates.len() < nights as usize {
                    let next = dates
                        .last()
                        .and_then(|date| date.next())
                        .ok_or(Error::Invalid)?;
                    dates.push(next);
                }
                let last = *dates.last().unwrap_or(&first);
                let conditions = RigConditions::read(store, rig.id, &placed, first, last)?;
                let time_zone = match &placed.site {
                    Some(site) => store
                        .site_profile(site.id)?
                        .and_then(|profile| profile.time_zone),
                    None => None,
                };
                let mut overrides = BTreeMap::new();
                if runtime == Runtime::Director {
                    for night in store.night_overrides(rig.id, first, last)? {
                        overrides.insert(
                            night.date,
                            AppliedOverrides {
                                revision: Some(night.revision),
                                entries: night.entries,
                                draft: false,
                            },
                        );
                    }
                }
                if let Some(draft) = draft {
                    if !dates.contains(&draft.date) {
                        return Err(NightPlanError::Rejected(
                            "override_draft must be for one of the nights planned.".into(),
                        ));
                    }
                    check(&draft.entries, draft.date.span(location.longitude_degrees))
                        .map_err(|refusal| Error::Refused(refusal.to_string()))?;
                    overrides.insert(
                        draft.date,
                        AppliedOverrides {
                            revision: None,
                            entries: draft.entries.clone(),
                            draft: true,
                        },
                    );
                }
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
                    conditions,
                    overrides,
                    time_zone,
                    ts_changes,
                    ts_projects: Vec::new(),
                    ts_save,
                    warnings: rig_warnings,
                    rig,
                });
            }
            if draft.as_ref().is_some_and(|draft| {
                !inputs
                    .iter()
                    .any(|input: &RigInput| input.rig.id == draft.rig)
            }) {
                return Err(NightPlanError::Rejected(
                    "override_draft must be for a rig the request plans, with a site location."
                        .into(),
                ));
            }
            if let Some(chosen) = &chosen {
                for missing in chosen.iter().filter(|rig| !seen.contains(rig)) {
                    warnings.push(format!(
                        "Rig {missing} is not a registered rig, so it is left out."
                    ));
                }
            }
            Ok::<_, NightPlanError>((inputs, warnings))
        })
        .await?;
    // The rest reads each rig's database and computes, with the store's
    // read slot let go: each rig, and each of its requests and nights, on
    // the shared pool.
    let (inputs, mut warnings) = gathered;
    let catalogs: Vec<String> = inputs
        .iter()
        .map(|input| input.catalog_slug.clone())
        .collect();
    let stopped = stop.clone();
    let view = blocking(move || {
        use rayon::prelude::*;
        let outcomes: Vec<Result<RigNights, String>> = inputs
            .into_par_iter()
            .map(|mut input| {
                if stopped.requested() {
                    return Err(String::new());
                }
                match ts_snapshot::read(&input.database_path) {
                    Ok(Some(read)) => {
                        input.warnings.extend(read.warnings);
                        let mut snapshot = read.snapshot;
                        // The controls show the rows as saved; the plans,
                        // with any unsaved changes.
                        input.ts_projects = super::ts_levers::views(&snapshot);
                        if let Some(changes) = &input.ts_changes {
                            input.warnings.extend(changes.apply(&mut snapshot));
                        }
                        input.snapshot = Some(snapshot);
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
            warnings.push("No rig has a location yet. Choose a site in Rig setup.".into());
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
            catalogs,
        }
    })
    .await?;
    if stop.requested() {
        return Err(NightPlanError::Stopped);
    }
    Ok(view)
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
        conditions,
        overrides,
        time_zone,
        ts_changes: _,
        ts_projects,
        ts_save,
        mut warnings,
    } = input;
    let location = placed
        .location
        .ok_or_else(|| format!("{catalog_name} has no location. Choose a site in Rig setup."))?;
    let super::rig_availability::Nights {
        nights: availability,
        clouds: cloud_hours,
        available,
        forecast,
    } = conditions.nights(&dates, location, now_ms());
    let projects = snapshot
        .as_ref()
        .map(|snapshot| {
            let parse =
                |guid: &Option<String>| guid.as_deref().and_then(|guid| Uuid::parse_str(guid).ok());
            snapshot
                .projects
                .iter()
                .map(|project| KnownProject {
                    id: project.id,
                    guid: parse(&project.guid),
                    active: project.state == ProjectState::Active,
                    plans: project
                        .targets
                        .iter()
                        .flat_map(|target| &target.exposure_plans)
                        .map(|plan| KnownPlan {
                            id: plan.id,
                            guid: parse(&plan.guid),
                            remaining: plan.remaining(
                                project.enable_grader,
                                snapshot.preferences.exposure_throttle_percent,
                            ),
                        })
                        .collect(),
                })
                .collect()
        })
        .unwrap_or_default();
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
    for (((summaries, (_, availability)), date), clouds) in by_night
        .into_iter()
        .zip(availability)
        .zip(&dates)
        .zip(cloud_hours)
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
            moon_altitudes: moon_altitudes(first.noon_ms, location),
            targets,
            load: Load {
                demand_hours,
                available_hours,
                idle_hours: (available_hours - demand_hours).max(0.0),
                overbooked_hours: (demand_hours - available_hours).max(0.0),
            },
            clouds,
            override_count: overrides
                .get(date)
                .filter(|applied| !applied.draft)
                .map_or(0, |applied| applied.entries.len()),
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
                    &overrides,
                    options.plan_from_ms,
                    options,
                ) {
                    Ok(view) => {
                        if input.gathered.preview_configuration {
                            warnings.push("The rig has not reported its equipment, so the schedule uses the filter and camera settings in its exposure templates.".into());
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
        mark_revisions: MarkRevisions {
            rig: conditions.rig_revision,
            site: conditions.site_revision,
        },
        forecast,
        time_zone,
        ts_projects,
        ts_save,
        warnings,
        projects,
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
                overrides: None,
            }
        })
        .collect();
    ScheduleView {
        runtime: Runtime::TargetScheduler,
        goals: Vec::new(),
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
    overrides: &BTreeMap<NightDate, AppliedOverrides>,
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
    let mut goals = Vec::new();
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
        let reference = TargetRef {
            project_id: project.as_ref().map_or(0, |p| p.0),
            project_name: project.map_or_else(|| link.project_name.to_owned(), |p| p.1),
            target_id: target.as_ref().map_or(0, |t| t.0),
            target_name: target.map_or_else(|| link.target_guid.to_string(), |t| t.1),
        };
        goals.push(DirectorGoal {
            goal_id: link.goal_id.to_owned(),
            plan_id: link.project_id,
            plan_name: link.project_name.to_owned(),
            target_guid: link.target_guid,
            exposureplan_guid: link.exposureplan_guid,
            project_id: reference.project_id,
            target_id: reference.target_id,
            exposure_plan_id: plan.map_or(0, |p| p.0),
            target_name: reference.target_name.clone(),
            filter: plan.and_then(|p| p.1.clone()),
        });
        references.insert(
            link.goal_id.to_owned(),
            (
                reference,
                plan.map_or(0, |p| p.0),
                plan.and_then(|p| p.1.clone()),
            ),
        );
    }
    let narrowings = dates
        .iter()
        .zip(&nights)
        .map(|(date, night)| {
            overrides
                .get(date)
                .map(|applied| gathered.narrowing(&applied.entries, night.span))
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| {
            "No Director schedule: the night's overrides could not be applied.".to_string()
        })?;
    let overheads = Overheads::default();
    let schedule = simulate(&ScheduleRequest {
        program: &program,
        constraints: &constraints,
        nights: &nights,
        start_ms,
        available,
        overheads,
        acceptance: &acceptance,
        narrowings: &narrowings,
    })
    .map_err(|error| {
        format!("No Director schedule: the rig's program could not be scheduled for these nights ({error:?}).")
    })?;
    let reference = |goal: &str| references.get(goal).cloned();
    let target_positions: BTreeMap<&str, IcrsPosition> = program
        .targets
        .iter()
        .map(|target| (target.id.as_str(), target.position()))
        .collect();
    let goal_positions: BTreeMap<&str, IcrsPosition> = program
        .bindings
        .iter()
        .filter_map(|binding| {
            let position = target_positions.get(binding.target_id.as_str())?;
            Some((binding.goal_id.as_str(), *position))
        })
        .collect();
    let mut scheduled = Vec::with_capacity(schedule.nights.len());
    for ((date, night), span) in dates.iter().zip(schedule.nights).zip(&nights) {
        let mut blocks: Vec<Block> = Vec::new();
        // Each run's target position, block by block.
        let mut positions: Vec<Vec<Option<IcrsPosition>>> = Vec::new();
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
                positions.push(Vec::new());
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
                _ => {
                    if let Some(runs) = positions.last_mut() {
                        runs.push(goal_positions.get(exposure.goal_id.as_str()).copied());
                    }
                    block.runs.push(FilterRun {
                        exposure_plan_id: plan_id,
                        filter,
                        exposure_seconds: exposure.exposure_ms as f64 / 1000.0,
                        frames: 1,
                        dithers: u32::from(exposure.dither),
                        start_ms: exposure.start_ms,
                        end_ms: exposure.end_ms,
                        moon: None,
                    });
                }
            }
        }
        place_moon(&mut blocks, &positions, location);
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
                    CoreGap::Held => GapReason::Held,
                    CoreGap::Excluded => GapReason::Excluded,
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
            overrides: overrides.get(date).cloned(),
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
        goals,
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

/// Fill in each run's Moon on a Director rig: the Moon as the site sees
/// it, parallax included, like the night's Moon strip. `positions` holds
/// each run's target position, block by block. Runs that meet share their
/// samples. A run without a position, or outside the ephemeris's years,
/// gets no Moon.
fn place_moon(
    blocks: &mut [psf_guard_ts_planner::Block],
    positions: &[Vec<Option<IcrsPosition>>],
    site: Site,
) {
    use psf_guard_ts_planner::RunMoon;
    debug_assert_eq!(blocks.len(), positions.len());
    let mut moons: BTreeMap<u64, Option<(IcrsPosition, f64)>> = BTreeMap::new();
    for (block, positions) in blocks.iter_mut().zip(positions) {
        debug_assert_eq!(block.runs.len(), positions.len());
        for (run, position) in block.runs.iter_mut().zip(positions) {
            let Some(target) = *position else {
                continue;
            };
            let samples: Vec<(f64, f64)> = RunMoon::sample_times(run.start_ms, run.end_ms)
                .into_iter()
                .filter_map(|at| {
                    let (moon, altitude) = (*moons.entry(at).or_insert_with(|| {
                        let moon = moon_at_site(at, site)?;
                        observe(moon, site, planning_orientation(at, at), at)
                            .ok()
                            .map(|observed| (moon, observed.altitude_degrees))
                    }))?;
                    Some((altitude, separation_degrees(moon, target)))
                })
                .collect();
            run.moon = RunMoon::over(samples);
        }
    }
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

/// Nights a projection schedules by default.
const PROJECTION_NIGHTS: u32 = 14;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct ProjectionRequest {
    /// Nights to schedule from the night under way; 14 by default.
    #[serde(default)]
    nights: Option<u32>,
    /// Plan from this instant instead of now.
    #[serde(default)]
    start_ms: Option<u64>,
    /// The runtime to schedule with; `auto`, the default, is each rig's own.
    #[serde(default)]
    runtime: Option<RuntimeChoice>,
}

/// When each rig the plan is activated on finishes it, by the rig's own
/// schedule: its runtime ranks the plan against the rig's other work, on
/// nights its marks and forecast allow.
#[derive(Serialize)]
pub(super) struct Projection {
    nights: u32,
    /// One per rig the plan is activated on.
    rigs: Vec<RigProjection>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct RigProjection {
    rig_id: Uuid,
    catalog_name: Option<String>,
    /// The runtime whose schedule this is; `None` when none was run.
    runtime: Option<Runtime>,
    outcome: Outcome,
    /// The scheduled nights, in order.
    dates: Vec<NightDate>,
    /// Frames the schedule plans each night for the plan's exposure plans
    /// on this rig.
    planned: Vec<u32>,
    /// The rig's other plans scheduled while the plan still owes frames,
    /// up to its last block: the most hours first.
    others: Vec<OtherPlan>,
    /// What the rig's night plans noted, such as goals its program leaves
    /// out, and a push not yet applied.
    notes: Vec<String>,
}

/// What the rig's schedule says of the plan.
#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Outcome {
    /// The schedule plans the plan's frames: `completes_on` is the night
    /// the last is planned, `None` when not within the nights.
    /// `unscheduled` counts the plan's exposure plans that owe frames but
    /// that the schedule leaves out.
    Scheduled {
        completes_on: Option<NightDate>,
        unscheduled: u32,
    },
    /// The rig has every frame the plan needs from it.
    Done,
    /// No projection for the rig, and why.
    Skipped { reason: String },
}

#[derive(Serialize, Debug, PartialEq)]
struct OtherPlan {
    name: String,
    hours: f64,
}

pub(super) async fn projection(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(request): Json<ProjectionRequest>,
) -> Result<Json<ApiResponse<Projection>>, NightPlanError> {
    let service = enabled(&state)?;
    let nights = request.nights.unwrap_or(PROJECTION_NIGHTS);
    if nights == 0 || nights > MAX_NIGHTS {
        return Err(NightPlanError::Rejected(format!(
            "nights must be from 1 to {MAX_NIGHTS}."
        )));
    }
    let activation = service
        .query(move |store| {
            store.project(id)?.ok_or(StoreError::NotFound)?;
            store.activation(id)
        })
        .await?;
    let Some(activation) = activation.filter(|a| !a.rigs.is_empty()) else {
        return Ok(Json(ApiResponse::success(Projection {
            nights,
            rigs: vec![],
            warnings: vec!["The plan is not activated on any rig yet.".into()],
        })));
    };
    let view = planned(
        &state,
        Request {
            rigs: Some(activation.rigs.iter().map(|rig| rig.rig_id).collect()),
            nights: Some(nights),
            schedule_nights: Some(nights),
            start_ms: request.start_ms,
            plan_from_ms: request.start_ms,
            runtime: request.runtime,
            ..Default::default()
        },
    )
    .await?
    .view;
    let rigs = activation
        .rigs
        .iter()
        .map(|activated| {
            let pushed = activation
                .pushes
                .iter()
                .find(|push| push.rig_id == activated.rig_id && !push.applied);
            project_rig(
                activated,
                view.rigs.iter().find(|rig| rig.rig.id == activated.rig_id),
                pushed,
            )
        })
        .collect();
    Ok(Json(ApiResponse::success(Projection {
        nights,
        rigs,
        warnings: view.warnings.clone(),
    })))
}

/// One activated rig's projection, from its night plans.
fn project_rig(
    activated: &psf_guard_director_meta::activation::ActivatedRig,
    planned: Option<&RigNights>,
    pushed: Option<&psf_guard_director_meta::activation::RemotePush>,
) -> RigProjection {
    let skipped = |rig: Option<&RigNights>, reason: String| RigProjection {
        rig_id: activated.rig_id,
        catalog_name: rig.map(|rig| rig.catalog_name.clone()),
        runtime: None,
        outcome: Outcome::Skipped { reason },
        dates: vec![],
        planned: vec![],
        others: vec![],
        notes: rig.map(|rig| rig.warnings.clone()).unwrap_or_default(),
    };
    let Some(rig) = planned else {
        return skipped(
            None,
            "The rig has no location, or its database is not registered on this server.".into(),
        );
    };
    if rig.projects.is_empty() {
        return skipped(
            Some(rig),
            "The rig's database could not be read, or has no N.I.N.A. profile yet.".into(),
        );
    }
    if rig.profile_id.as_deref() != Some(activated.profile_id.as_str()) {
        return skipped(
            Some(rig),
            format!(
                "The plan's project is in N.I.N.A. profile {}. Night plans use profile {}, which has the most projects.",
                activated.profile_id,
                rig.profile_id.as_deref().unwrap_or("none")
            ),
        );
    }
    let Some(project) = rig
        .projects
        .iter()
        .find(|p| p.guid == Some(activated.project_guid))
    else {
        return skipped(
            Some(rig),
            "The rig's database has no project for the plan. Activate the plan on the rig again."
                .into(),
        );
    };
    if !project.active {
        return skipped(
            Some(rig),
            "The plan's project is not active in the rig's database. Set it active in Target Scheduler to schedule it.".into(),
        );
    }
    // The plan's own exposure plans, as activated: plans added to the
    // project by hand are not the plan's.
    let mut owed = Vec::new();
    let mut missing = 0u32;
    for plan in &activated.plans {
        match project
            .plans
            .iter()
            .find(|p| p.guid == Some(plan.exposureplan_guid))
        {
            Some(known) if known.remaining == 0 => {}
            Some(known) => owed.push(known.id),
            None => missing += 1,
        }
    }
    let mut notes = rig.warnings.clone();
    if let Some(push) = pushed {
        notes.push(format!(
            "The plan has not reached {} yet, so the rig may not run this schedule.",
            push.peer_name
        ));
    }
    if owed.is_empty() && missing == 0 {
        return RigProjection {
            rig_id: activated.rig_id,
            catalog_name: Some(rig.catalog_name.clone()),
            runtime: rig.schedule.as_ref().map(|s| s.runtime),
            outcome: Outcome::Done,
            dates: vec![],
            planned: vec![],
            others: vec![],
            notes,
        };
    }
    let Some(schedule) = &rig.schedule else {
        return skipped(
            Some(rig),
            "No schedule could be computed. The notes explain why.".into(),
        );
    };
    let progress: Vec<_> = schedule
        .progress
        .iter()
        .filter(|p| owed.contains(&p.exposure_plan_id))
        .collect();
    let unscheduled = missing
        + owed
            .iter()
            .filter(|id| !progress.iter().any(|p| p.exposure_plan_id == **id))
            .count() as u32;
    let completes = (unscheduled == 0
        && progress
            .iter()
            .all(|p| p.complete_before || p.completes_on.is_some()))
    .then(|| progress.iter().filter_map(|p| p.completes_on).max())
    .flatten();
    let dates: Vec<NightDate> = schedule.nights.iter().map(|n| n.date).collect();
    RigProjection {
        rig_id: activated.rig_id,
        catalog_name: Some(rig.catalog_name.clone()),
        runtime: Some(schedule.runtime),
        outcome: Outcome::Scheduled {
            completes_on: completes.and_then(|night| dates.get(night).copied()),
            unscheduled,
        },
        planned: (0..dates.len())
            .map(|night| {
                progress
                    .iter()
                    .map(|p| p.planned.get(night).copied().unwrap_or(0))
                    .sum()
            })
            .collect(),
        others: others_first(&schedule.nights, project.id, &owed, completes),
        dates,
        notes,
    }
}

/// The other plans a schedule shoots while the plan still owes frames: on
/// each night before it completes, and on the night it completes, until its
/// last block there ends. Over all the nights when it does not complete.
fn others_first(
    nights: &[ScheduledNight],
    project_id: i64,
    owed: &[i64],
    completes: Option<usize>,
) -> Vec<OtherPlan> {
    let ours = |block: &psf_guard_ts_planner::Block| {
        block
            .runs
            .iter()
            .any(|run| owed.contains(&run.exposure_plan_id))
    };
    let mut hours: BTreeMap<String, f64> = BTreeMap::new();
    for (index, night) in nights.iter().enumerate() {
        if completes.is_some_and(|last| index > last) {
            break;
        }
        let until = match completes {
            Some(last) if index == last => night
                .schedule
                .blocks
                .iter()
                .filter(|block| ours(block))
                .map(|block| block.end_ms)
                .max()
                .unwrap_or(0),
            _ => u64::MAX,
        };
        for block in night
            .schedule
            .blocks
            .iter()
            .filter(|b| b.target.project_id != project_id)
        {
            let end = block.end_ms.min(until);
            if end > block.start_ms {
                *hours.entry(block.target.project_name.clone()).or_default() +=
                    (end - block.start_ms) as f64 / 3_600_000.0;
            }
        }
    }
    let mut others: Vec<OtherPlan> = hours
        .into_iter()
        .map(|(name, hours)| OtherPlan { name, hours })
        .collect();
    others.sort_by(|a, b| {
        b.hours
            .total_cmp(&a.hours)
            .then_with(|| a.name.cmp(&b.name))
    });
    others
}

#[cfg(test)]
mod tests {
    use super::*;
    use psf_guard_ts_planner::{Block, Decision, DecisionKind, FilterRun, TargetRef};

    fn block(project_id: i64, plan: i64, start_h: u64, end_h: u64) -> Block {
        let hour = 3_600_000;
        Block {
            target: TargetRef {
                project_id,
                project_name: format!("Plan {project_id}"),
                target_id: project_id,
                target_name: String::new(),
            },
            start_ms: start_h * hour,
            end_ms: end_h * hour,
            slew: false,
            runs: vec![FilterRun {
                exposure_plan_id: plan,
                filter: "Ha".into(),
                exposure_seconds: 300.0,
                frames: 1,
                dithers: 0,
                start_ms: start_h * hour,
                end_ms: end_h * hour,
                moon: None,
            }],
            decision: Decision {
                kind: DecisionKind::Scored,
                candidates: vec![],
                note: None,
            },
        }
    }

    fn night(date: &str, blocks: Vec<Block>) -> ScheduledNight {
        ScheduledNight {
            date: date.parse().unwrap(),
            schedule: psf_guard_ts_planner::NightSchedule {
                start_ms: 0,
                end_ms: 0,
                blocks,
                gaps: vec![],
                frames: 0,
                exposure_hours: 0.0,
                note: None,
            },
            overrides: None,
        }
    }

    #[test]
    fn a_director_run_takes_the_closest_moon_and_its_highest_altitude() {
        // 00:00 to 02:00 in Los Angeles on the night of the 2026-09-26 full Moon.
        let hour = 1_790_409_600 / 3600 - 1;
        let (start, end) = (hour * 3_600_000, (hour + 2) * 3_600_000);
        let site = Site {
            latitude_degrees: 34.05,
            longitude_degrees: -118.25,
            elevation_meters: 100.0,
        };
        let altitude = |at: u64| {
            observe(
                moon_at_site(at, site).unwrap(),
                site,
                planning_orientation(at, at),
                at,
            )
            .unwrap()
            .altitude_degrees
        };
        // A target where the Moon stands at 01:00, and one opposite it.
        let moon = moon_at_site(start + 3_600_000, site).unwrap();
        let opposite = IcrsPosition {
            ra_degrees: (moon.ra_degrees + 180.0).rem_euclid(360.0),
            dec_degrees: -moon.dec_degrees,
        };
        let mut blocks = vec![
            block(1, 1, hour, hour + 2),
            block(2, 2, hour, hour + 2),
            block(3, 3, hour, hour + 2),
        ];
        let positions = [vec![Some(moon)], vec![Some(opposite)], vec![None]];
        place_moon(&mut blocks, &positions, site);
        let near = blocks[0].runs[0].moon.unwrap();
        assert!(near.min_separation_degrees < 0.01, "{near:?}");
        let highest = altitude(start)
            .max(altitude(start + 3_600_000))
            .max(altitude(end));
        assert!(near.max_altitude_degrees >= highest, "{near:?}");
        assert!(near.max_altitude_degrees > 30.0, "{near:?}");
        let far = blocks[1].runs[0].moon.unwrap();
        assert!(far.min_separation_degrees > 178.0, "{far:?}");
        assert_eq!(far.max_altitude_degrees, near.max_altitude_degrees);
        assert_eq!(blocks[2].runs[0].moon, None);
    }

    #[test]
    fn the_moon_curve_rises_and_sets_once_on_a_full_moon_night() {
        // Noon in Los Angeles on 2026-09-26, the day of the full Moon.
        let noon = 1_790_449_200_000;
        let site = Site {
            latitude_degrees: 34.05,
            longitude_degrees: -118.25,
            elevation_meters: 100.0,
        };
        let curve = moon_altitudes(noon, site);
        assert_eq!(curve.start_ms, noon);
        assert_eq!(curve.step_ms, 15 * 60_000);
        assert_eq!(curve.degrees.len(), 96);
        let crossings = curve
            .degrees
            .windows(2)
            .filter(|pair| (pair[0] > 0.0) != (pair[1] > 0.0))
            .count();
        assert_eq!(crossings, 2, "{:?}", curve.degrees);
        // Up around midnight.
        assert!(curve.degrees[48] > 30.0, "{:?}", curve.degrees);
    }

    #[test]
    fn a_reader_can_leave_the_moon_curve_out() {
        let value = serde_json::json!({
            "rigs": [{ "nights": [{ "date": "2026-09-26", "moon_altitudes": { "degrees": [1.0] } }] }],
            "computed_at_ms": 1,
        });
        let trimmed = without_moon_altitudes(value);
        assert_eq!(
            trimmed,
            serde_json::json!({ "rigs": [{ "nights": [{ "date": "2026-09-26" }] }], "computed_at_ms": 1 })
        );
    }

    #[test]
    fn other_plans_count_only_while_the_plan_still_owes_frames() {
        // The plan (project 1, exposure plan 10) completes on the second
        // night at 22 h; plan 2 shoots before it each night and after it on
        // the second; plan 3 shoots only on the third night.
        let nights = vec![
            night(
                "2026-11-01",
                vec![block(2, 20, 18, 20), block(1, 10, 20, 24)],
            ),
            night(
                "2026-11-02",
                vec![
                    block(2, 20, 42, 44),
                    block(1, 10, 44, 46),
                    block(2, 20, 46, 50),
                ],
            ),
            night("2026-11-03", vec![block(3, 30, 66, 70)]),
        ];
        assert_eq!(
            others_first(&nights, 1, &[10], Some(1)),
            vec![OtherPlan {
                name: "Plan 2".into(),
                hours: 4.0
            }]
        );
        // Not completing, every night counts.
        let all = others_first(&nights, 1, &[10], None);
        assert_eq!(
            all.iter()
                .map(|o| (o.name.as_str(), o.hours))
                .collect::<Vec<_>>(),
            vec![("Plan 2", 8.0), ("Plan 3", 4.0)]
        );
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
