//! Automatic stack previews.
//!
//! Once a project has stack previews, PSF Guard can keep them current on its
//! own. Frames that arrive by import, remote upload, or scheduler sync, and
//! grades that change, queue a refresh of the project's remembered cards.
//! Arrivals settle for a few minutes first, so a night's stream of uploads is
//! stacked in batches rather than after every frame; grading is interactive,
//! so a grade change waits longer, and every further change pushes the
//! refresh out again, up to a cap. A refresh runs on the same single stacking
//! worker as a build the user asks for and steps aside for one: an
//! interactive build cancels a running automatic one, whose checkpoint picks
//! up where it stopped.
//!
//! A refresh asks for exactly what the cards remember: the same targets and
//! channels, the same Accepted-only policy, order, scoring, and calibration
//! choices, over the project's current frames, one build per channel as a
//! person's Build stacks makes them. The job identity covers the frames and
//! their grades, so a channel that would rebuild nothing new is a cache hit
//! and starts no work. With new channels on, channels with no stack yet and
//! a recent frame are stacked too. Color follows each target's last channel.

use super::color::{self, StackColorJob, StackColorRequest};
use super::{
    current_latest_stacks, current_project_latest_stacks, enqueue_job, latest_path, manifest_path,
    prepare_channels, read_latest_indices, validate_request, CalibrationOverride,
    LatestStackPreviews, StackChannelKey, StackGroupState, StackJobState, StackPreviewJob,
    StackPreviewRequest, StackScoringSettings,
};
use crate::db::Database;
use crate::models::AcquiredImage;
use crate::server::api::ScoringOverrideQuery;
use crate::server::database_context::DatabaseContext;
use crate::server::handlers::AppError;
use crate::server::state::AppState;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Minutes an arrival or sync settles before the refresh runs.
pub const DEFAULT_ARRIVAL_DELAY_MINUTES: u32 = 5;
/// Minutes a grade change settles before the refresh runs.
pub const DEFAULT_GRADE_DELAY_MINUTES: u32 = 15;
/// The longest either delay may be set to: a day.
pub const MAX_DELAY_MINUTES: u32 = 24 * 60;
/// A stream of touches pushes a refresh out, but not forever: it runs at the
/// latest this many delays after the first touch.
const MAX_DEFERRALS: u32 = 4;
/// How often the scheduler looks for due refreshes.
const TICK: Duration = Duration::from_secs(15);
/// Granularity of journaled due times, in seconds.
const JOURNAL_DUE_STEP: i64 = 30;

/// False until the job journal has been restored, so the scheduler cannot
/// queue a refresh ahead of the builds that were waiting before a restart.
static READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Let the scheduler start refreshes.
pub fn mark_ready() {
    READY.store(true, Ordering::Relaxed);
}
/// How long a due refresh waits when the stacker or the user is busy.
const BUSY_RETRY: Duration = Duration::from_secs(30);

static ENABLED: AtomicBool = AtomicBool::new(false);
static ARRIVAL_DELAY_MINUTES: AtomicU32 = AtomicU32::new(DEFAULT_ARRIVAL_DELAY_MINUTES);
static GRADE_DELAY_MINUTES: AtomicU32 = AtomicU32::new(DEFAULT_GRADE_DELAY_MINUTES);
static BUILD_NEW_CHANNELS: AtomicBool = AtomicBool::new(false);
/// Stamps every touch, so a refresh that replaces one that just ran never
/// reuses its predecessor's stamp.
static TOUCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// By default a channel with no stack yet is built only when one of its
/// frames was captured this recently, so turning the option on does not
/// stack a whole old catalog at once.
pub const DEFAULT_NEW_CHANNEL_WINDOW_DAYS: u32 = 7;
/// The longest window: past ten years, choose any age.
pub const MAX_NEW_CHANNEL_WINDOW_DAYS: u32 = 3650;
/// The window a person chose, in days; 0 takes channels of any age.
static NEW_CHANNEL_WINDOW_DAYS: AtomicU32 = AtomicU32::new(DEFAULT_NEW_CHANNEL_WINDOW_DAYS);

/// The capture time a channel with no stack yet needs a frame at or after,
/// or `None` when any age will do.
fn new_channel_since() -> Option<i64> {
    match NEW_CHANNEL_WINDOW_DAYS.load(Ordering::Relaxed) {
        0 => None,
        days => Some(chrono::Utc::now().timestamp() - i64::from(days) * 86_400),
    }
}

fn default_new_channel_window_days() -> u32 {
    DEFAULT_NEW_CHANNEL_WINDOW_DAYS
}

/// What the operator chose, process-wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationPolicy {
    pub enabled: bool,
    pub arrival_delay_minutes: u32,
    pub grade_delay_minutes: u32,
    /// Also stack channels that have no stack yet, as their frames arrive.
    #[serde(default)]
    pub build_new_channels: bool,
    /// Days back a new channel's frames may reach; 0 takes any age.
    #[serde(default = "default_new_channel_window_days")]
    pub new_channel_window_days: u32,
}

impl Default for AutomationPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            arrival_delay_minutes: DEFAULT_ARRIVAL_DELAY_MINUTES,
            grade_delay_minutes: DEFAULT_GRADE_DELAY_MINUTES,
            build_new_channels: false,
            new_channel_window_days: DEFAULT_NEW_CHANNEL_WINDOW_DAYS,
        }
    }
}

/// Apply a policy for every refresh this process schedules from now on.
pub fn configure(policy: AutomationPolicy) {
    ENABLED.store(policy.enabled, Ordering::Relaxed);
    BUILD_NEW_CHANNELS.store(policy.build_new_channels, Ordering::Relaxed);
    NEW_CHANNEL_WINDOW_DAYS.store(
        policy
            .new_channel_window_days
            .min(MAX_NEW_CHANNEL_WINDOW_DAYS),
        Ordering::Relaxed,
    );
    ARRIVAL_DELAY_MINUTES.store(
        policy.arrival_delay_minutes.clamp(1, MAX_DELAY_MINUTES),
        Ordering::Relaxed,
    );
    GRADE_DELAY_MINUTES.store(
        policy.grade_delay_minutes.clamp(1, MAX_DELAY_MINUTES),
        Ordering::Relaxed,
    );
}

/// The registry's stored choice, or the default when it stores none.
pub fn configure_from_registry(settings: Option<&crate::db_registry::StackAutomationSettings>) {
    super::method::configure(
        settings
            .and_then(|settings| settings.method)
            .unwrap_or_default(),
    );
    configure(
        settings
            .map(|settings| settings.policy())
            .unwrap_or_default(),
    );
}

pub fn policy() -> AutomationPolicy {
    AutomationPolicy {
        enabled: ENABLED.load(Ordering::Relaxed),
        arrival_delay_minutes: ARRIVAL_DELAY_MINUTES.load(Ordering::Relaxed),
        grade_delay_minutes: GRADE_DELAY_MINUTES.load(Ordering::Relaxed),
        build_new_channels: BUILD_NEW_CHANNELS.load(Ordering::Relaxed),
        new_channel_window_days: NEW_CHANNEL_WINDOW_DAYS.load(Ordering::Relaxed),
    }
}

/// Why a project is due for a refresh. The reason sets the settling delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshReason {
    /// Frames were imported or uploaded.
    Arrival,
    /// A scheduler sync or database transfer changed the catalog.
    Sync,
    /// Grades changed, by hand or by a scan.
    Grade,
}

impl RefreshReason {
    fn delay(self) -> Duration {
        let minutes = match self {
            RefreshReason::Arrival | RefreshReason::Sync => {
                ARRIVAL_DELAY_MINUTES.load(Ordering::Relaxed)
            }
            RefreshReason::Grade => GRADE_DELAY_MINUTES.load(Ordering::Relaxed),
        };
        Duration::from_secs(u64::from(minutes) * 60)
    }

    /// Arrivals and syncs share one delay and one stream; grades have their own.
    fn settles_like(self, other: RefreshReason) -> bool {
        (self == RefreshReason::Grade) == (other == RefreshReason::Grade)
    }

    fn label(self) -> &'static str {
        match self {
            RefreshReason::Arrival => "new frames",
            RefreshReason::Sync => "a sync",
            RefreshReason::Grade => "grade changes",
        }
    }
}

/// A refresh waiting for its frames or grades to settle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScheduledRefresh {
    pub database_id: String,
    /// `None` refreshes every followed project of the database.
    pub project_id: Option<i32>,
    pub reason: RefreshReason,
    pub due_in_seconds: u64,
    /// Changes with every touch, and is never reused.
    pub touches: u64,
}

/// One project, or every followed project of one database.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RefreshKey {
    pub database_id: String,
    pub project_id: Option<i32>,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    due_at: Instant,
    first_touched: Instant,
    reason: RefreshReason,
    /// The latest touch's stamp, so what the refresh expects to do is worked
    /// out once per change rather than on every look at the queue.
    touches: u64,
}

/// The refreshes waiting to run.
#[derive(Default)]
pub struct AutomaticStackRefresh {
    pending: Mutex<HashMap<RefreshKey, Pending>>,
}

impl AutomaticStackRefresh {
    /// Every followed project of the database is due after the reason's delay.
    pub fn touch_database(&self, database_id: &str, reason: RefreshReason) {
        self.touch(
            RefreshKey {
                database_id: database_id.to_string(),
                project_id: None,
            },
            reason,
            Instant::now(),
        );
    }

    /// The named projects are due after the reason's delay.
    pub fn touch_projects(
        &self,
        database_id: &str,
        projects: impl IntoIterator<Item = i32>,
        reason: RefreshReason,
    ) {
        let now = Instant::now();
        for project_id in projects {
            self.touch(
                RefreshKey {
                    database_id: database_id.to_string(),
                    project_id: Some(project_id),
                },
                reason,
                now,
            );
        }
    }

    fn touch(&self, key: RefreshKey, reason: RefreshReason, now: Instant) {
        if !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        let delay = reason.delay();
        let mut pending = self.pending.lock().unwrap();
        let entry = pending.entry(key).or_insert(Pending {
            due_at: now + delay,
            first_touched: now,
            reason,
            touches: 0,
        });
        entry.touches = TOUCHES.fetch_add(1, Ordering::Relaxed) + 1;
        if entry.reason.settles_like(reason) {
            // A further touch lets the stream settle, but never past the
            // cap from the stream's first touch.
            entry.due_at = (now + delay).min(entry.first_touched + delay * MAX_DEFERRALS);
            entry.reason = reason;
        } else if reason != RefreshReason::Grade {
            // New frames outrank waiting grades: a fresh, shorter stream
            // starts now, and the refresh comes no later than it would have.
            entry.due_at = entry.due_at.min(now + delay);
            entry.first_touched = now;
            entry.reason = reason;
        }
        // A grade change under pending frames changes nothing: the frames
        // already come sooner.
    }

    /// Put a refresh back, to run again after `by`, keeping its history so
    /// the cap still holds.
    fn postpone(&self, key: RefreshKey, pending: Pending, by: Duration, now: Instant) {
        let mut entries = self.pending.lock().unwrap();
        let entry = entries.entry(key).or_insert(pending);
        entry.due_at = entry.due_at.max(now + by);
    }

    fn take_due(&self, now: Instant) -> Vec<(RefreshKey, Pending)> {
        let mut pending = self.pending.lock().unwrap();
        let due: Vec<RefreshKey> = pending
            .iter()
            .filter(|(_, entry)| entry.due_at <= now)
            .map(|(key, _)| key.clone())
            .collect();
        due.into_iter()
            .filter_map(|key| pending.remove(&key).map(|entry| (key, entry)))
            .collect()
    }

    /// Whole-database entries first, then projects in a stable order, so a
    /// test and a log read the same way.
    fn ordered_due(&self, now: Instant) -> Vec<(RefreshKey, Pending)> {
        let mut due = self.take_due(now);
        due.sort_by(|(left, _), (right, _)| {
            left.database_id
                .cmp(&right.database_id)
                .then_with(|| left.project_id.cmp(&right.project_id))
        });
        due
    }

    /// The refreshes waiting to run, soonest first, for the header's queue.
    pub fn scheduled(&self) -> Vec<ScheduledRefresh> {
        let now = Instant::now();
        let mut scheduled = self
            .pending
            .lock()
            .unwrap()
            .iter()
            .map(|(key, entry)| ScheduledRefresh {
                database_id: key.database_id.clone(),
                project_id: key.project_id,
                reason: entry.reason,
                due_in_seconds: entry.due_at.saturating_duration_since(now).as_secs(),
                touches: entry.touches,
            })
            .collect::<Vec<_>>();
        scheduled.sort_by(|left, right| {
            left.due_in_seconds
                .cmp(&right.due_in_seconds)
                .then_with(|| left.database_id.cmp(&right.database_id))
                .then_with(|| left.project_id.cmp(&right.project_id))
        });
        scheduled
    }

    /// The waiting refreshes with wall-clock due times, for the job journal.
    pub fn journal_entries(&self) -> Vec<super::journal::JournaledRefresh> {
        let now = Instant::now();
        let wall = chrono::Utc::now().timestamp();
        self.pending
            .lock()
            .unwrap()
            .iter()
            .map(|(key, entry)| super::journal::JournaledRefresh {
                database_id: key.database_id.clone(),
                project_id: key.project_id,
                reason: entry.reason,
                // Rounded, so a settling refresh does not rewrite the
                // journal every few seconds as the clock moves.
                due_unix: (wall + entry.due_at.saturating_duration_since(now).as_secs() as i64)
                    .div_euclid(JOURNAL_DUE_STEP)
                    * JOURNAL_DUE_STEP,
            })
            .collect()
    }

    /// Put back a refresh the journal kept across a restart, due when it was
    /// due then, or now if that has passed. Nothing when automation is off.
    pub fn restore(&self, refresh: &super::journal::JournaledRefresh) {
        if !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        let now = Instant::now();
        let wait = (refresh.due_unix - chrono::Utc::now().timestamp()).max(0) as u64;
        let due_at = now + Duration::from_secs(wait);
        let key = RefreshKey {
            database_id: refresh.database_id.clone(),
            project_id: refresh.project_id,
        };
        let mut pending = self.pending.lock().unwrap();
        let entry = pending.entry(key).or_insert(Pending {
            due_at,
            first_touched: now,
            reason: refresh.reason,
            touches: 0,
        });
        entry.due_at = entry.due_at.min(due_at);
    }

    /// Drop one waiting refresh. False when nothing waits under that key.
    pub fn skip(&self, key: &RefreshKey) -> bool {
        self.pending.lock().unwrap().remove(key).is_some()
    }

    /// Make one waiting refresh due now; the next tick starts it.
    pub fn run_now(&self, key: &RefreshKey) -> bool {
        match self.pending.lock().unwrap().get_mut(key) {
            Some(entry) => {
                entry.due_at = Instant::now();
                true
            }
            None => false,
        }
    }

    /// How many refreshes wait, for status and tests.
    pub fn pending_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }

    /// Forget everything queued, when automation is switched off.
    pub fn clear(&self) {
        self.pending.lock().unwrap().clear();
    }
}

/// Run the scheduler for the life of the process.
pub fn spawn(state: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(TICK).await;
            if !ENABLED.load(Ordering::Relaxed) {
                continue;
            }
            tick(&state).await;
        }
    })
}

async fn tick(state: &Arc<AppState>) {
    if !READY.load(Ordering::Relaxed) {
        return;
    }
    let now = Instant::now();
    let due = state.auto_stacks.ordered_due(now);
    if due.is_empty() {
        return;
    }
    // One stacking worker, and the user comes first on it: while a build
    // runs, or interactive work is in flight, everything due waits.
    if state.interactive_job_active() || !state.stack_previews.active().is_empty() {
        for (key, pending) in due {
            state.auto_stacks.postpone(key, pending, BUSY_RETRY, now);
        }
        return;
    }
    let mut remaining = due.into_iter();
    while let Some((key, pending)) = remaining.next() {
        let Some(ctx) = state.get_database(&key.database_id) else {
            continue;
        };
        // A quality scan rewrites the scores a refresh would stack by;
        // let it finish, and its end touches the database again.
        if crate::server::quality_backfill::snapshot(&ctx.quality_backfill).running {
            state.auto_stacks.postpone(key, pending, BUSY_RETRY, now);
            continue;
        }
        let projects = match key.project_id {
            Some(project_id) => vec![project_id],
            None => followed_projects(&ctx),
        };
        for project_id in projects {
            match refresh_project(state, &ctx, project_id, pending.reason).await {
                Ok(RefreshOutcome::Started) => {
                    // The worker is taken; the rest of this database and
                    // every other due key wait for the next free moment,
                    // keeping their history so the cap still holds.
                    let rest: Vec<i32> = match key.project_id {
                        Some(_) => Vec::new(),
                        None => followed_projects(&ctx)
                            .into_iter()
                            .filter(|other| *other > project_id)
                            .collect(),
                    };
                    for other in rest {
                        state.auto_stacks.postpone(
                            RefreshKey {
                                database_id: key.database_id.clone(),
                                project_id: Some(other),
                            },
                            pending,
                            BUSY_RETRY,
                            now,
                        );
                    }
                    for (key, pending) in remaining {
                        state.auto_stacks.postpone(key, pending, BUSY_RETRY, now);
                    }
                    return;
                }
                Ok(RefreshOutcome::Unchanged) => {}
                Ok(RefreshOutcome::Skipped(why)) => {
                    tracing::debug!(
                        db = %key.database_id,
                        project_id,
                        "automatic stack refresh skipped: {why}"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        db = %key.database_id,
                        project_id,
                        "automatic stack refresh failed: {error:?}"
                    );
                }
            }
        }
    }
}

enum RefreshOutcome {
    /// A build was queued on the stacking worker.
    Started,
    /// The remembered cards already show the current frames and grades.
    Unchanged,
    /// Nothing to refresh, and why.
    Skipped(String),
}

/// Projects whose cards this database shows: the cards the grid would show
/// now, under the project's current exposure grouping. Cards kept aside for
/// the other grouping are not followed.
fn followed_projects(ctx: &DatabaseContext) -> Vec<i32> {
    let mut projects = stacked_projects(ctx);
    if BUILD_NEW_CHANNELS.load(Ordering::Relaxed) {
        projects.extend(projects_with_recent_frames(ctx));
    }
    projects.sort_unstable();
    projects.dedup();
    projects
}

/// Projects whose cards this database shows now.
fn stacked_projects(ctx: &DatabaseContext) -> Vec<i32> {
    let mut projects: Vec<i32> = read_latest_indices::<LatestStackPreviews>(
        &crate::server::storage::stacks(&ctx.stack_root),
    )
    .into_iter()
    .filter(|latest| latest.database_id == ctx.id)
    .filter_map(|latest| {
        let project_id = latest.project_id;
        match current_project_latest_stacks(ctx, project_id, latest) {
            Ok(latest) if !latest.groups.is_empty() => Some(project_id),
            Ok(_) => None,
            Err(error) => {
                tracing::warn!(
                    db = %ctx.id,
                    project_id,
                    "could not read remembered stack previews: {error:?}"
                );
                None
            }
        }
    })
    .collect();
    projects.sort_unstable();
    projects.dedup();
    projects
}

/// Projects with a frame captured inside the new-channel window, or every
/// project with a frame when any age will do: the only ones a refresh may
/// start stacking from nothing.
fn projects_with_recent_frames(ctx: &DatabaseContext) -> Vec<i32> {
    let since = new_channel_since();
    let conn = ctx.db();
    let Ok(conn) = conn.lock() else {
        return Vec::new();
    };
    // Any age takes frames with no capture date too, as the plan does.
    let query = match since {
        Some(_) => "SELECT DISTINCT projectId FROM acquiredimage WHERE acquireddate >= ?1",
        None => "SELECT DISTINCT projectId FROM acquiredimage WHERE ?1 IS NOT NULL",
    };
    conn.prepare(query)
        .and_then(|mut statement| {
            statement
                .query_map([since.unwrap_or(0)], |row| row.get::<_, i32>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default()
}

fn read_latest(
    ctx: &DatabaseContext,
    project_id: i32,
) -> Result<Option<LatestStackPreviews>, AppError> {
    let bytes = match std::fs::read(latest_path(&ctx.stack_root, project_id)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(AppError::InternalError(format!(
                "reading remembered stack previews: {error}"
            )))
        }
    };
    let latest: LatestStackPreviews = serde_json::from_slice(&bytes).map_err(|error| {
        AppError::InternalError(format!("remembered stack previews are unreadable: {error}"))
    })?;
    current_project_latest_stacks(ctx, project_id, current_latest_stacks(latest)).map(Some)
}

async fn refresh_project(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    project_id: i32,
    reason: RefreshReason,
) -> Result<RefreshOutcome, AppError> {
    let ctx_for_plan = Arc::clone(ctx);
    let planned = tokio::task::spawn_blocking(move || {
        let Some(plan) = plan_refresh(&ctx_for_plan, project_id)? else {
            return Ok::<_, AppError>(None);
        };
        validate_request(&plan.request)?;
        // One build per channel, the same builds a person's Build stacks
        // makes, so a refresh after it is a cache hit for what did not
        // change and restacks only what did.
        let channels = prepare_channels(
            &ctx_for_plan,
            project_id,
            &plan.request,
            Some(&plan.channels),
        )?;
        Ok(Some(channels))
    })
    .await
    .map_err(|error| {
        AppError::InternalError(format!("Stack preparation task failed: {error}"))
    })??;
    let Some(channels) = planned else {
        return Ok(RefreshOutcome::Skipped(
            "no stacked channel, and no new one to build".into(),
        ));
    };
    let mut started = 0;
    for (request, mut prepared) in channels {
        // A channel with too few frames has nothing to stack yet.
        if prepared
            .public
            .groups
            .iter()
            .all(|group| group.state == StackGroupState::Skipped)
        {
            continue;
        }
        let job_id = prepared.public.job_id.clone();
        if let Some(existing) = state.stack_previews.get(&job_id)
            && matches!(
                existing.state,
                StackJobState::Queued | StackJobState::Running
            )
        {
            continue;
        }
        // A build of these exact frames already finished, perhaps before
        // the card moved on to another: the card points at it again and its
        // color follows, as when a person's build matches one.
        let finished = state
            .stack_previews
            .get(&job_id)
            .filter(|existing| existing.state == StackJobState::Completed)
            .or_else(|| {
                std::fs::read(manifest_path(&prepared.stack_root, &job_id))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<StackPreviewJob>(&bytes).ok())
                    .filter(|existing| existing.state == StackJobState::Completed)
            });
        if let Some(existing) = finished {
            let state = Arc::clone(state);
            let ctx = Arc::clone(ctx);
            let _ = tokio::task::spawn_blocking(move || {
                if let Err(error) = state.stack_previews.persist_latest(&ctx, &existing) {
                    tracing::warn!("could not point the card back at a finished build: {error}");
                }
                compose_colors_after(&state, &ctx, &existing);
            })
            .await;
            continue;
        }
        prepared.public.automatic = true;
        if !state.stack_previews.insert(prepared.public.clone()) {
            break;
        }
        tracing::info!(
            db = %ctx.id,
            project_id,
            job_id,
            "Refreshing {} after {}",
            prepared
                .public
                .groups
                .first()
                .map(|group| super::channel_label(&group.target_name, &group.filter_name))
                .unwrap_or_default(),
            reason.label(),
        );
        let origin = super::journal::JournaledStackJob::mono(
            &ctx.id,
            project_id,
            &request,
            &prepared.public,
            true,
        );
        enqueue_job(Arc::clone(state), prepared, origin);
        started += 1;
    }
    Ok(if started > 0 {
        RefreshOutcome::Started
    } else {
        RefreshOutcome::Unchanged
    })
}

/// A channel's identity within its database, for telling refreshes that
/// would stack the same thing apart from ones that only share a name.
fn claim_key(project_id: i32, key: &StackChannelKey) -> String {
    format!(
        "{project_id}\0{}\0{}\0{}",
        key.target_id,
        key.filter_name,
        key.exposure_group_key.as_deref().unwrap_or("")
    )
}

/// The most projects a whole-database refresh's guess plans.
const EXPECTED_MAX_PROJECTS: usize = 25;

/// How long a waiting refresh's expected channels are trusted between
/// touches: frames can change without one, such as a quality scan.
const EXPECTED_TTL: Duration = Duration::from_secs(60);

type ExpectedKey = (String, Option<i32>, u64, bool, u32);
type ExpectedCache = HashMap<ExpectedKey, (Instant, Vec<(String, String)>)>;
static EXPECTED: std::sync::LazyLock<Mutex<ExpectedCache>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// The channels a waiting refresh expects to restack or stack for the first
/// time, for the header's queue, as (key, name). Empty when it expects to
/// do nothing.
pub(super) fn expected_channels(
    ctx: &DatabaseContext,
    refresh: &ScheduledRefresh,
) -> Vec<(String, String)> {
    // A whole-database refresh is touched by every arrival, so its guess
    // is kept a minute however often that happens; one project's is cheap
    // to redo on each touch.
    let key = (
        refresh.database_id.clone(),
        refresh.project_id,
        refresh.project_id.map_or(0, |_| refresh.touches),
        BUILD_NEW_CHANNELS.load(Ordering::Relaxed),
        NEW_CHANNEL_WINDOW_DAYS.load(Ordering::Relaxed),
    );
    if let Some((at, channels)) = EXPECTED.lock().unwrap().get(&key)
        && at.elapsed() < EXPECTED_TTL
    {
        return channels.clone();
    }
    // For a whole database, the guess plans only projects already stacked,
    // and not too many: it runs while the header asks what is queued, on the
    // database's shared connection. New channels elsewhere still stack when
    // the refresh runs, and show then as their own builds.
    let projects = match refresh.project_id {
        Some(project_id) => vec![project_id],
        None => {
            let mut projects = stacked_projects(ctx);
            projects.truncate(EXPECTED_MAX_PROJECTS);
            projects
        }
    };
    let channels = projects
        .into_iter()
        .filter_map(|project_id| match plan_refresh(ctx, project_id) {
            Ok(plan) => plan,
            Err(error) => {
                tracing::debug!(db = %ctx.id, project_id, "could not plan a refresh: {error:?}");
                None
            }
        })
        .flat_map(|plan| plan.expected)
        .collect::<Vec<_>>();
    let mut cache = EXPECTED.lock().unwrap();
    cache.retain(|_, (at, _)| at.elapsed() < EXPECTED_TTL);
    cache.insert(key, (Instant::now(), channels.clone()));
    channels
}

/// What a refresh of one project asks for: the channels it stacks, the
/// request that names their frames, and which of them it expects to restack
/// or build for the first time, for the header's queue.
pub(super) struct RefreshPlan {
    pub request: StackPreviewRequest,
    pub channels: Vec<StackChannelKey>,
    /// The channels whose frames or grades moved since their stack, then the
    /// ones with no stack yet: each with a key unique within its database
    /// and the name people read.
    pub expected: Vec<(String, String)>,
}

/// Plan a project's refresh: its stacked channels over the frames it holds
/// now, and, with new channels on, every channel with no stack yet whose
/// frames include one captured inside the window. `None` when there is
/// nothing to stack.
pub(super) fn plan_refresh(
    ctx: &DatabaseContext,
    project_id: i32,
) -> Result<Option<RefreshPlan>, AppError> {
    let latest = read_latest(ctx, project_id)?.unwrap_or_else(|| LatestStackPreviews {
        schema_version: 1,
        database_id: ctx.id.clone(),
        project_id,
        updated_unix_seconds: 0,
        groups: Vec::new(),
    });
    let build_new = BUILD_NEW_CHANNELS.load(Ordering::Relaxed);
    if latest.groups.is_empty() && !build_new {
        return Ok(None);
    }
    let (images, exposure_groups) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let images = Database::new(&conn)
            .get_images_by_project_id(project_id)
            .map_err(AppError::db)?;
        let groups = crate::server::exposure_groups::cached_project_groups(ctx, &conn, project_id)?;
        (images, groups)
    };
    let key_of = |image: &AcquiredImage| StackChannelKey {
        target_id: image.target_id,
        filter_name: image.filter_name.clone(),
        exposure_group_key: exposure_groups
            .by_image
            .get(&image.id)
            .map(|group| group.key.clone()),
    };
    let mut current: BTreeMap<StackChannelKey, Vec<&AcquiredImage>> = BTreeMap::new();
    let mut names: HashMap<StackChannelKey, String> = HashMap::new();
    for (image, _, target_name) in &images {
        let key = key_of(image);
        names.entry(key.clone()).or_insert_with(|| {
            let mut name = super::channel_label(target_name, &image.filter_name);
            if let Some(group) = exposure_groups.by_image.get(&image.id) {
                name.push_str(" · ");
                name.push_str(&group.label);
            }
            name
        });
        current.entry(key).or_default().push(image);
    }
    let remembered = latest
        .groups
        .iter()
        .map(|entry| {
            (
                StackChannelKey {
                    target_id: entry.group.target_id,
                    filter_name: entry.group.filter_name.clone(),
                    exposure_group_key: entry
                        .group
                        .exposure_group
                        .as_ref()
                        .map(|group| group.key.clone()),
                },
                entry
                    .group
                    .input_images
                    .iter()
                    .map(|image| (image.image_id, image.grading_status))
                    .collect::<HashSet<_>>(),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut request = refresh_request(&latest, &[]).unwrap_or_else(|| StackPreviewRequest {
        image_ids: Vec::new(),
        accepted_only: false,
        force: false,
        north_up: false,
        calibration: crate::calibration::CalibrationMode::Auto,
        calibration_overrides: Vec::new(),
        order: Default::default(),
        scoring: Default::default(),
        method: None,
        color_defaults: None,
        channel: None,
    });
    let recent_since = new_channel_since();
    let mut channels = Vec::new();
    let mut restack = Vec::new();
    let mut fresh = Vec::new();
    for (key, frames) in &current {
        let name = names.get(key).cloned().unwrap_or_default();
        match remembered.get(key) {
            Some(built) => {
                channels.push(key.clone());
                let now = frames
                    .iter()
                    .map(|image| (image.id, image.grading_status))
                    .collect::<HashSet<_>>();
                if &now != built {
                    restack.push((claim_key(project_id, key), name));
                }
            }
            None if build_new => {
                // As the build will count them: accepted frames only when the
                // project's cards stack only those, else anything not rejected.
                let usable = frames
                    .iter()
                    .filter(|image| {
                        if request.accepted_only {
                            image.grading_status == 1
                        } else {
                            image.grading_status != 2
                        }
                    })
                    .count();
                let recent = frames.iter().any(|image| {
                    recent_since
                        .is_none_or(|since| image.acquired_date.is_some_and(|at| at >= since))
                });
                if usable >= 2 && recent {
                    channels.push(key.clone());
                    fresh.push((claim_key(project_id, key), format!("{name} (new)")));
                }
            }
            None => {}
        }
    }
    if channels.is_empty() {
        return Ok(None);
    }
    let targets = channels
        .iter()
        .map(|key| key.target_id)
        .collect::<HashSet<_>>();
    // Every frame of the targets involved, so each channel's siblings vote on
    // the pier-side mapping as they do in a person's build.
    let image_ids = images
        .iter()
        .filter(|(image, _, _)| targets.contains(&image.target_id))
        .map(|(image, _, _)| image.id)
        .collect::<Vec<_>>();
    request.image_ids = image_ids;
    restack.extend(fresh);
    Ok(Some(RefreshPlan {
        request,
        channels,
        expected: restack,
    }))
}

/// The request that rebuilds what a project's cards remember, over the
/// frames the project holds now. `None` when fewer than two frames remain
/// in the remembered channels.
pub(super) fn refresh_request(
    latest: &LatestStackPreviews,
    images: &[AcquiredImage],
) -> Option<StackPreviewRequest> {
    let newest = latest
        .groups
        .iter()
        .max_by_key(|group| (group.created_unix_seconds, group.job_id.clone()))?;
    let channels: HashSet<(i32, String)> = latest
        .groups
        .iter()
        .map(|group| (group.group.target_id, group.group.filter_name.clone()))
        .collect();
    let mut image_ids: Vec<i32> = images
        .iter()
        .filter(|image| channels.contains(&(image.target_id, image.filter_name.clone())))
        .map(|image| image.id)
        .collect();
    image_ids.sort_unstable();
    image_ids.dedup();
    // With no images given, only the settings are wanted.
    if image_ids.len() < 2 && !images.is_empty() {
        return None;
    }
    // Each card keeps its own calibration choice; the request carries the
    // exceptions from the default.
    let mut calibration_overrides: BTreeMap<(i32, String, Option<String>), CalibrationOverride> =
        BTreeMap::new();
    for group in &latest.groups {
        let mode = group.group.calibration.mode;
        if mode == crate::calibration::CalibrationMode::Auto {
            continue;
        }
        let key = group
            .group
            .exposure_group
            .as_ref()
            .map(|exposure| exposure.key.clone());
        calibration_overrides
            .entry((
                group.group.target_id,
                group.group.filter_name.clone(),
                key.clone(),
            ))
            .or_insert(CalibrationOverride {
                target_id: group.group.target_id,
                filter_name: group.group.filter_name.clone(),
                exposure_group_key: key,
                calibration: mode,
            });
    }
    let north_up = newest
        .group
        .sky_orientation
        .as_ref()
        .is_some_and(|orientation| orientation.convention == seiza_stacking::SKY_ORIENTATION_NAME);
    Some(StackPreviewRequest {
        image_ids,
        accepted_only: newest.accepted_only,
        force: false,
        north_up,
        calibration: crate::calibration::CalibrationMode::Auto,
        calibration_overrides: calibration_overrides.into_values().collect(),
        order: newest.order,
        scoring: scoring_overrides(&newest.scoring),
        // The server's current method, so a settings change reaches the next refresh.
        method: None,
        // A refresh brings back the color previews composed before, never a
        // first one nobody asked for.
        color_defaults: None,
        channel: None,
    })
}

/// The overrides that reproduce a recorded scoring snapshot.
fn scoring_overrides(scoring: &StackScoringSettings) -> ScoringOverrideQuery {
    ScoringOverrideQuery {
        penalty_satellite: Some(scoring.penalty_satellite),
        penalty_pointing: Some(scoring.penalty_pointing),
        penalty_temporal: Some(scoring.penalty_temporal),
        hfr_reject_above: scoring.hfr_reject_above,
        star_count_reject_below: scoring.star_count_reject_below,
    }
}

/// Compose a target's color previews again once a build leaves none of its
/// channels waiting, so color follows the channels in the queue: the ones
/// composed before, from the newest stack of each of their channels, and,
/// when the build carried the person's display pipeline and the target has
/// none, its first composite. A target another build is still stacking
/// waits for that build, which composes it when it ends.
pub(super) fn compose_colors_after(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    job: &StackPreviewJob,
) {
    // Every target the build touched: a channel that stopped or failed
    // still releases the color its finished siblings were waiting on.
    let targets = job
        .groups
        .iter()
        .map(|group| group.target_id)
        .collect::<std::collections::BTreeSet<_>>();
    for target_id in targets {
        if state.stack_previews.target_still_building(
            &job.database_id,
            job.project_id,
            target_id,
            &job.job_id,
        ) {
            continue;
        }
        let requests = match color::requests_after_build(
            ctx,
            job.project_id,
            target_id,
            job.color_defaults.as_ref(),
        ) {
            Ok(requests) => requests,
            Err(error) => {
                tracing::warn!(
                    db = %ctx.id,
                    project_id = job.project_id,
                    target_id,
                    "could not plan the color previews after a build: {error:?}"
                );
                continue;
            }
        };
        for request in requests {
            enqueue_color_after(state, ctx, job.project_id, &request, job.automatic);
        }
    }
}

/// Queue one color preview after a build, unless the same one is already
/// queued, running, or built.
fn enqueue_color_after(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    project_id: i32,
    request: &StackColorRequest,
    automatic: bool,
) {
    let mut prepared = match color::prepare_color_job(ctx, project_id, request) {
        Ok(prepared) => prepared,
        Err(error) => {
            tracing::warn!(
                db = %ctx.id,
                project_id,
                "could not prepare a color preview after a build: {error:?}"
            );
            return;
        }
    };
    let color_id = prepared.public.job_id.clone();
    if let Some(existing) = state.stack_previews.get_color(&color_id)
        && matches!(
            existing.state,
            StackJobState::Queued | StackJobState::Running | StackJobState::Completed
        )
    {
        return;
    }
    let manifest = color::color_manifest_path(&prepared.stack_root, &color_id);
    if let Ok(bytes) = std::fs::read(&manifest)
        && let Ok(existing) = serde_json::from_slice::<StackColorJob>(&bytes)
        && existing.state == StackJobState::Completed
        && color::color_job_artifacts_exist(&prepared.stack_root, &existing)
    {
        return;
    }
    prepared.public.automatic = automatic;
    if !state.stack_previews.insert_color(prepared.public.clone()) {
        tracing::warn!("color preview after a build skipped: too many color jobs are active");
        return;
    }
    tracing::info!(
        db = %ctx.id,
        project_id,
        job_id = color_id,
        "Composing the {} color preview after its channels were stacked",
        prepared.public.label
    );
    let origin = super::journal::JournaledStackJob::color(&ctx.id, project_id, request, automatic);
    color::enqueue_color_job(Arc::clone(state), prepared, origin);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minutes(count: u64) -> Duration {
        Duration::from_secs(count * 60)
    }

    fn key(project: Option<i32>) -> RefreshKey {
        RefreshKey {
            database_id: "db-one".into(),
            project_id: project,
        }
    }

    /// Tests share the process-wide policy, so they take turns with it.
    static POLICY_LOCK: Mutex<()> = Mutex::new(());

    fn enabled(arrival: u32, grade: u32) -> std::sync::MutexGuard<'static, ()> {
        let guard = POLICY_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        configure(AutomationPolicy {
            enabled: true,
            arrival_delay_minutes: arrival,
            grade_delay_minutes: grade,
            build_new_channels: false,
            new_channel_window_days: DEFAULT_NEW_CHANNEL_WINDOW_DAYS,
        });
        guard
    }

    #[test]
    fn a_waiting_refresh_is_listed_skipped_or_brought_forward() {
        let _guard = POLICY_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        configure(AutomationPolicy {
            enabled: true,
            ..AutomationPolicy::default()
        });
        let refresh = AutomaticStackRefresh::default();
        refresh.touch_projects("db-a", [7, 8], RefreshReason::Arrival);
        refresh.touch_projects("db-a", [9], RefreshReason::Grade);
        let scheduled = refresh.scheduled();
        assert_eq!(scheduled.len(), 3);
        // Arrivals settle sooner than grades, so they lead.
        assert_eq!(scheduled[2].project_id, Some(9));
        assert_eq!(scheduled[2].reason, RefreshReason::Grade);
        let key = |project| RefreshKey {
            database_id: "db-a".into(),
            project_id: Some(project),
        };
        assert!(refresh.skip(&key(8)));
        assert!(!refresh.skip(&key(8)));
        assert!(refresh.run_now(&key(9)));
        let scheduled = refresh.scheduled();
        assert_eq!(scheduled[0].project_id, Some(9));
        assert_eq!(scheduled[0].due_in_seconds, 0);
        assert_eq!(refresh.ordered_due(Instant::now()).len(), 1);
        configure(AutomationPolicy::default());
    }

    #[test]
    fn a_stream_of_arrivals_settles_but_not_forever() {
        let _policy = enabled(5, 15);
        let refresh = AutomaticStackRefresh::default();
        let start = Instant::now();
        refresh.touch(key(Some(1)), RefreshReason::Arrival, start);
        assert!(refresh.take_due(start + minutes(4)).is_empty(), "not yet");
        // Each new frame pushes the run out by the full delay again.
        refresh.touch(key(Some(1)), RefreshReason::Arrival, start + minutes(4));
        assert!(refresh.take_due(start + minutes(6)).is_empty());
        for touch in [8u64, 12, 16, 19] {
            refresh.touch(key(Some(1)), RefreshReason::Arrival, start + minutes(touch));
        }
        // The cap from the first touch: four delays, twenty minutes.
        assert!(refresh.take_due(start + minutes(19)).is_empty());
        let due = refresh.take_due(start + minutes(20));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].1.reason, RefreshReason::Arrival);
        assert_eq!(refresh.pending_count(), 0);
    }

    #[test]
    fn grades_wait_longer_and_an_arrival_pulls_them_in() {
        let _policy = enabled(5, 15);
        let refresh = AutomaticStackRefresh::default();
        let start = Instant::now();
        refresh.touch(key(Some(7)), RefreshReason::Grade, start);
        assert!(refresh.take_due(start + minutes(14)).is_empty());
        // A later grade change pushes the refresh out again.
        refresh.touch(key(Some(7)), RefreshReason::Grade, start + minutes(10));
        assert!(refresh.take_due(start + minutes(20)).is_empty());
        // New frames outrank waiting grades: due in the arrival delay, and
        // no sooner, even late in a long grading stream.
        for touch in [22u64, 30, 40, 50] {
            refresh.touch(key(Some(7)), RefreshReason::Grade, start + minutes(touch));
        }
        refresh.touch(key(Some(7)), RefreshReason::Arrival, start + minutes(55));
        assert!(
            refresh.take_due(start + minutes(59)).is_empty(),
            "the arrival settles first"
        );
        let due = refresh.take_due(start + minutes(60));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].1.reason, RefreshReason::Arrival);
        // And a grade change after an arrival does not push the arrival out.
        refresh.touch(key(Some(8)), RefreshReason::Arrival, start);
        refresh.touch(key(Some(8)), RefreshReason::Grade, start + minutes(1));
        assert_eq!(refresh.take_due(start + minutes(5)).len(), 1);
    }

    #[test]
    fn a_postponed_refresh_keeps_its_history_and_disabled_automation_takes_nothing() {
        let _policy = enabled(5, 15);
        let refresh = AutomaticStackRefresh::default();
        let start = Instant::now();
        refresh.touch(key(None), RefreshReason::Sync, start);
        let mut due = refresh.take_due(start + minutes(5));
        let (key_taken, pending) = due.pop().unwrap();
        assert_eq!(key_taken, key(None));
        refresh.postpone(
            key_taken,
            pending,
            Duration::from_secs(30),
            start + minutes(5),
        );
        assert!(refresh.take_due(start + minutes(5)).is_empty());
        let again = refresh.take_due(start + minutes(6));
        assert_eq!(again.len(), 1);
        assert_eq!(
            again[0].1.first_touched, start,
            "the cap still counts from the first touch"
        );

        configure(AutomationPolicy::default());
        refresh.touch(key(Some(1)), RefreshReason::Arrival, start);
        assert_eq!(refresh.pending_count(), 0, "off means nothing is queued");
    }

    fn image(id: i32, target_id: i32, filter: &str) -> AcquiredImage {
        AcquiredImage {
            id,
            project_id: 1,
            target_id,
            acquired_date: Some(1_700_000_000 + i64::from(id)),
            filter_name: filter.into(),
            grading_status: 1,
            metadata: "{}".into(),
            reject_reason: None,
            profile_id: None,
            guid: None,
        }
    }

    fn remembered(
        target_id: i32,
        filter: &str,
        created: i64,
        mode: crate::calibration::CalibrationMode,
        north_up: bool,
    ) -> serde_json::Value {
        let calibration = serde_json::to_value(crate::calibration::AppliedCalibration {
            mode,
            ..Default::default()
        })
        .unwrap();
        serde_json::json!({
            "job_id": format!("job-{filter}-{created}"),
            "artifact_revision": "rev",
            "accepted_only": created % 2 == 0,
            "created_unix_seconds": created,
            "cache_version": super::super::STACK_PREVIEW_CACHE_VERSION,
            "order": "capture",
            "weighting": if filter == "G" { "noise" } else { "equal" },
            "scoring": {"penalty_satellite": 0.5, "penalty_pointing": 1.0, "penalty_temporal": 1.0, "hfr_reject_above": 3.0, "star_count_reject_below": null},
            "group": {
                "index": 0, "target_id": target_id, "target_name": "T", "filter_name": filter,
                "state": "ready", "total_candidates": 2, "eligible_frames": 2, "quality_excluded": 0,
                "missing_files": 0, "processed_frames": 2, "accepted_frames": 2, "rejected_frames": 0,
                "reference_image_id": 1, "total_exposure_seconds": 600.0, "preview_url": null, "fits_url": null,
                "error": null, "frames": [],
                "calibration": calibration,
                "sky_orientation": north_up.then(|| serde_json::json!({
                    "convention": seiza_stacking::SKY_ORIENTATION_NAME, "version": 1, "source": "sky_anchor",
                    "output_width": 10, "output_height": 10,
                    "source_to_output": seiza_stacking::AffineTransform::IDENTITY
                }))
            }
        })
    }

    #[test]
    fn a_waiting_refresh_names_only_the_channels_it_will_stack() {
        let _guard = POLICY_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let directory = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        conn.execute_batch("INSERT INTO project(Id,name,profileId,guid) VALUES(1,'Project','profile','project-one');
            INSERT INTO target(Id,name,projectId,active,ra,dec,epochcode,rotation,roi,guid)
                VALUES(1,'T',1,1,10,20,0,0,100,'target-one');
            CREATE TABLE psf_guard_project_processing(project_key TEXT PRIMARY KEY,split_exposure_groups INTEGER NOT NULL,split_chosen INTEGER);
            INSERT INTO psf_guard_project_processing VALUES('guid:project-one',0,1);").unwrap();
        let now = chrono::Utc::now().timestamp();
        let long_ago = now - 400 * 86_400;
        // R was stacked from frames 1 and 2; G never was, and its frames are
        // recent; B never was, and its frames are old.
        for (id, filter, at) in [
            (1, "R", long_ago),
            (2, "R", long_ago),
            (3, "G", now),
            (4, "G", now),
            (5, "B", long_ago),
            (6, "B", long_ago),
        ] {
            conn.execute("INSERT INTO acquiredimage(Id,projectId,targetId,gradingStatus,metadata,acquireddate,filtername)
                VALUES(?1,1,1,1,'{}',?2,?3)", rusqlite::params![id, at, filter]).unwrap();
        }
        let mut ctx = DatabaseContext::new_for_test(conn);
        ctx.use_storage_for_test(directory.path());
        let ctx = Arc::new(ctx);
        let mut stacked = remembered(1, "R", 10, crate::calibration::CalibrationMode::Auto, true);
        stacked["group"]["input_images"] = serde_json::json!([
            {"image_id": 1, "grading_status": 1},
            {"image_id": 2, "grading_status": 1}
        ]);
        let latest = serde_json::json!({
            "schema_version": 1, "database_id": ctx.id, "project_id": 1, "updated_unix_seconds": 5,
            "groups": [stacked]
        });
        let path = latest_path(&ctx.stack_root, 1);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(&latest).unwrap()).unwrap();

        // R is current: the refresh would only find cache hits, so it
        // expects nothing and the queue does not list it.
        configure(AutomationPolicy {
            enabled: true,
            ..AutomationPolicy::default()
        });
        let plan = plan_refresh(&ctx, 1).unwrap().expect("R is followed");
        assert!(plan.expected.is_empty(), "{:?}", plan.expected);
        let names = |plan: &RefreshPlan| {
            plan.expected
                .iter()
                .map(|(_, name)| name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(plan.channels.len(), 1);

        // A new R frame, and R is due to restack.
        {
            let db = ctx.db();
            db.lock()
                .unwrap()
                .execute("INSERT INTO acquiredimage(Id,projectId,targetId,gradingStatus,metadata,acquireddate,filtername)
                    VALUES(7,1,1,1,'{}',?1,'R')", [now])
                .unwrap();
        }
        let plan = plan_refresh(&ctx, 1).unwrap().unwrap();
        assert_eq!(names(&plan), ["T · R"]);

        // With new channels on, G's recent frames are stacked too; B's old
        // ones are left alone.
        configure(AutomationPolicy {
            enabled: true,
            build_new_channels: true,
            ..AutomationPolicy::default()
        });
        let plan = plan_refresh(&ctx, 1).unwrap().unwrap();
        assert_eq!(names(&plan), ["T · R", "T · G (new)"]);
        assert_eq!(plan.channels.len(), 2);
        // Every frame of the target goes in, so each channel's siblings vote
        // on the pier-side mapping.
        assert_eq!(plan.request.image_ids.len(), 7);

        // With no cutoff, B's old frames are stacked too.
        configure(AutomationPolicy {
            enabled: true,
            build_new_channels: true,
            new_channel_window_days: 0,
            ..AutomationPolicy::default()
        });
        let plan = plan_refresh(&ctx, 1).unwrap().unwrap();
        assert_eq!(names(&plan), ["T · R", "T · B (new)", "T · G (new)"]);
        configure(AutomationPolicy::default());
    }

    #[test]
    fn a_refresh_asks_for_what_the_cards_remember_over_the_frames_the_project_holds_now() {
        let latest: LatestStackPreviews = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "database_id": "db-one", "project_id": 1, "updated_unix_seconds": 5,
            "groups": [
                remembered(1, "R", 10, crate::calibration::CalibrationMode::Auto, false),
                remembered(1, "G", 12, crate::calibration::CalibrationMode::Off, true)
            ]
        }))
        .unwrap();
        let images = vec![
            image(1, 1, "R"),
            image(2, 1, "R"),
            image(3, 1, "G"),
            image(4, 1, "G"),
            image(5, 1, "G"),
            // A channel with no card is not part of the refresh.
            image(6, 1, "B"),
            // Nor is another target.
            image(7, 2, "R"),
        ];
        let request = refresh_request(&latest, &images).expect("a request");
        assert_eq!(request.image_ids, vec![1, 2, 3, 4, 5]);
        // The newest card's policies carry the request.
        assert!(request.accepted_only, "the G card at 12 was accepted-only");
        assert!(request.north_up);
        assert_eq!(
            request.method, None,
            "a refresh takes the server's current method"
        );
        assert_eq!(request.scoring.penalty_satellite, Some(0.5));
        assert_eq!(request.scoring.hfr_reject_above, Some(3.0));
        assert_eq!(request.scoring.star_count_reject_below, None);
        assert!(!request.force);
        // Only the card that chose something other than auto is an override.
        assert_eq!(request.calibration_overrides.len(), 1);
        assert_eq!(request.calibration_overrides[0].filter_name, "G");
        assert_eq!(
            request.calibration_overrides[0].calibration,
            crate::calibration::CalibrationMode::Off
        );
        assert!(
            refresh_request(&latest, &images[..1]).is_none(),
            "one frame cannot be stacked"
        );
    }
}
