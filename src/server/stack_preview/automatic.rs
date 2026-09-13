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
//! choices, over the project's current frames. The job identity covers the
//! frames and their grades, so a refresh that would rebuild nothing new is a
//! cache hit and starts no work. After a mono refresh finishes, the color
//! previews composed from its channels are recomposed the same way.

use super::color::{self, StackColorJob, StackColorRequest, StackColorSourceRef};
use super::{
    current_latest_stacks, current_project_latest_stacks, enqueue_job, latest_path, manifest_path,
    prepare_job, read_latest_indices, validate_request, CalibrationOverride, LatestStackPreviews,
    StackGroupState, StackJobState, StackPreviewJob, StackPreviewRequest, StackScoringSettings,
    MAX_REMEMBERED_JOBS,
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
/// How long a due refresh waits when the stacker or the user is busy.
const BUSY_RETRY: Duration = Duration::from_secs(30);

static ENABLED: AtomicBool = AtomicBool::new(false);
static ARRIVAL_DELAY_MINUTES: AtomicU32 = AtomicU32::new(DEFAULT_ARRIVAL_DELAY_MINUTES);
static GRADE_DELAY_MINUTES: AtomicU32 = AtomicU32::new(DEFAULT_GRADE_DELAY_MINUTES);

/// What the operator chose, process-wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationPolicy {
    pub enabled: bool,
    pub arrival_delay_minutes: u32,
    pub grade_delay_minutes: u32,
}

impl Default for AutomationPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            arrival_delay_minutes: DEFAULT_ARRIVAL_DELAY_MINUTES,
            grade_delay_minutes: DEFAULT_GRADE_DELAY_MINUTES,
        }
    }
}

/// Apply a policy for every refresh this process schedules from now on.
pub fn configure(policy: AutomationPolicy) {
    ENABLED.store(policy.enabled, Ordering::Relaxed);
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
        });
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
    let mut projects: Vec<i32> =
        read_latest_indices::<LatestStackPreviews>(&ctx.cache_dir_path.join("stack-previews"))
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

fn read_latest(
    ctx: &DatabaseContext,
    project_id: i32,
) -> Result<Option<LatestStackPreviews>, AppError> {
    let bytes = match std::fs::read(latest_path(&ctx.cache_dir_path, project_id)) {
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
    let Some(latest) = read_latest(ctx, project_id)? else {
        return Ok(RefreshOutcome::Skipped(
            "no remembered stack previews".into(),
        ));
    };
    if latest.groups.is_empty() {
        return Ok(RefreshOutcome::Skipped(
            "no remembered stack previews".into(),
        ));
    }
    let images: Vec<AcquiredImage> = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        Database::new(&conn)
            .get_images_by_project_id(project_id)
            .map_err(AppError::db)?
            .into_iter()
            .map(|(image, _, _)| image)
            .collect()
    };
    let Some(request) = refresh_request(&latest, &images) else {
        return Ok(RefreshOutcome::Skipped(
            "the remembered channels have fewer than two frames".into(),
        ));
    };
    validate_request(&request)?;
    let ctx_for_prepare = Arc::clone(ctx);
    let request_for_prepare = request.clone();
    let mut prepared = tokio::task::spawn_blocking(move || {
        prepare_job(&ctx_for_prepare, project_id, &request_for_prepare)
    })
    .await
    .map_err(|error| {
        AppError::InternalError(format!("Stack preparation task failed: {error}"))
    })??;
    let job_id = prepared.public.job_id.clone();
    // The identity covers frames, grades, scores, and calibration: a
    // request that matches the cards already built starts nothing.
    if let Some(existing) = state.stack_previews.get(&job_id)
        && matches!(
            existing.state,
            StackJobState::Queued | StackJobState::Running | StackJobState::Completed
        )
    {
        return Ok(RefreshOutcome::Unchanged);
    }
    if let Ok(bytes) = std::fs::read(manifest_path(&prepared.cache_root, &job_id))
        && let Ok(existing) = serde_json::from_slice::<StackPreviewJob>(&bytes)
        && existing.state == StackJobState::Completed
    {
        return Ok(RefreshOutcome::Unchanged);
    }
    prepared.public.automatic = true;
    if !state.stack_previews.insert(prepared.public.clone()) {
        return Ok(RefreshOutcome::Skipped(format!(
            "{MAX_REMEMBERED_JOBS} stack preview jobs are already active"
        )));
    }
    tracing::info!(
        db = %ctx.id,
        project_id,
        job_id,
        "Refreshing stack previews after {} ({} frames across {} channels)",
        reason.label(),
        request.image_ids.len(),
        latest.groups.len()
    );
    enqueue_job(Arc::clone(state), prepared);
    Ok(RefreshOutcome::Started)
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
    if image_ids.len() < 2 {
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

/// Recompose the color previews that drew on channels a finished automatic
/// refresh just rebuilt, with the same kind, palette, crop, and processing.
pub(super) fn recompose_colors(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    job: &StackPreviewJob,
) {
    let remembered = match color::load_latest_colors(ctx, job.project_id) {
        Ok(latest) => latest.jobs,
        Err(error) => {
            tracing::warn!(
                db = %ctx.id,
                project_id = job.project_id,
                "could not read remembered color previews: {error:?}"
            );
            return;
        }
    };
    for previous in remembered {
        let Some(request) = color_refresh_request(&previous, job) else {
            continue;
        };
        let mut prepared = match color::prepare_color_job(ctx, job.project_id, &request) {
            Ok(prepared) => prepared,
            Err(error) => {
                tracing::warn!(
                    db = %ctx.id,
                    project_id = job.project_id,
                    "could not prepare a color refresh: {error:?}"
                );
                continue;
            }
        };
        let color_id = prepared.public.job_id.clone();
        if let Some(existing) = state.stack_previews.get_color(&color_id)
            && matches!(
                existing.state,
                StackJobState::Queued | StackJobState::Running | StackJobState::Completed
            )
        {
            continue;
        }
        let manifest = color::color_manifest_path(&prepared.cache_root, &color_id);
        if let Ok(bytes) = std::fs::read(&manifest)
            && let Ok(existing) = serde_json::from_slice::<StackColorJob>(&bytes)
            && existing.state == StackJobState::Completed
            && color::color_job_artifacts_exist(&prepared.cache_root, &existing)
        {
            continue;
        }
        prepared.public.automatic = true;
        if !state.stack_previews.insert_color(prepared.public.clone()) {
            tracing::warn!("color refresh skipped: too many color jobs are active");
            continue;
        }
        tracing::info!(
            db = %ctx.id,
            project_id = job.project_id,
            job_id = color_id,
            "Recomposing the {} color preview after an automatic refresh",
            previous.label
        );
        color::enqueue_color_job(Arc::clone(state), prepared);
    }
}

/// The remembered color preview's request, pointed at the channels the mono
/// job just rebuilt. `None` when the job rebuilt none of its channels.
fn color_refresh_request(
    previous: &StackColorJob,
    job: &StackPreviewJob,
) -> Option<StackColorRequest> {
    let mut input_sources = BTreeMap::new();
    let mut changed = false;
    for source in &previous.sources {
        let rebuilt = job.groups.iter().find(|group| {
            group.state == StackGroupState::Ready
                && group.target_id == previous.target_id
                && group.filter_name == source.filter_name
                && group.exposure_group.as_ref().map(|exposure| &exposure.key)
                    == source.exposure_group.as_ref().map(|exposure| &exposure.key)
        });
        let reference = match rebuilt {
            Some(group) => {
                changed |= group.index != source.group_index
                    || job.job_id != source.job_id
                    || job.artifact_revision != source.artifact_revision;
                StackColorSourceRef {
                    job_id: job.job_id.clone(),
                    group_index: group.index,
                    artifact_revision: job.artifact_revision.clone(),
                }
            }
            None => StackColorSourceRef {
                job_id: source.job_id.clone(),
                group_index: source.group_index,
                artifact_revision: source.artifact_revision.clone(),
            },
        };
        input_sources.insert(source.role, reference);
    }
    if !changed {
        return None;
    }
    Some(StackColorRequest {
        target_id: previous.target_id,
        kind: previous.kind,
        palette: previous.palette,
        force: false,
        crop: previous.crop,
        processing: previous.processing.clone(),
        input_sources,
    })
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
        });
        guard
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
