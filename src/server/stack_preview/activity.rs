//! The stack work queued, running or waiting on every database, as the
//! Stacks page lists it, and the routes that move, stop or start it.

use super::*;

/// Cross-database view of stack builds still in flight. The manager is
/// process-global, so this stays outside the per-database routes and lets the
/// header report stacking from any view.
pub async fn get_stack_activity(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<StackActivity>> {
    // Names of scheduled refreshes come from the catalogs: a short read
    // each, off the runtime.
    let activity =
        tokio::task::spawn_blocking(move || stack_activity(&state, RefreshPlanning::Plan))
            .await
            .unwrap_or_else(|_| StackActivity {
                schema_version: 1,
                active: Vec::new(),
                scheduled: Vec::new(),
                waiting_reapplies: Vec::new(),
            });
    Json(ApiResponse::success(activity))
}

/// Whether listing stack work may plan a waiting refresh to learn which
/// channels it would restack.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefreshPlanning {
    /// Plan on the database's shared connection when the cache has no
    /// answer: the Stacks page's route.
    Plan,
    /// Read only what the refresh scheduler cached: the header's jobs
    /// route, which must not wait for a shared connection.
    CachedOnly,
}

/// The stack work queued, running or waiting on every database. Reads a
/// short query per scheduled refresh to name its project, so call it off
/// the async runtime.
pub(crate) fn stack_activity(state: &Arc<AppState>, planning: RefreshPlanning) -> StackActivity {
    let scheduled = state.auto_stacks.scheduled();
    let scheduled = if scheduled.is_empty() {
        Vec::new()
    } else {
        name_scheduled(state, scheduled, planning)
    };
    let waiting_reapplies = waiting_reapplies(state, &scheduled);
    // Read after the refreshes and re-applies, so one that started
    // meanwhile is in the line by now; it is listed once, as its build or
    // run.
    let active = state.stack_previews.active();
    StackActivity {
        schema_version: 1,
        scheduled: without_started(scheduled, &active),
        waiting_reapplies: reapplies_not_started(waiting_reapplies, &active),
        active,
    }
}

/// What automatic stack work waits for, in words that fit "Waiting for …":
/// for a due refresh, the automatic builds already in the line, a person's
/// work, a quiet moment after it, or a quality analysis of its database;
/// for a re-apply, the build or run holding the worker or the line ahead of
/// it, a person's work, or a quiet moment after it. `None` when it is free
/// to start at the scheduler's next look.
pub(super) fn automatic_work_waits_for(
    state: &AppState,
    database_id: &str,
    reapply: bool,
) -> Option<String> {
    let manager = &state.stack_previews;
    let held = if reapply {
        !manager.active().is_empty()
    } else {
        automatic::automatic_builds_in_line(manager)
    };
    let holder = held.then(|| {
        manager
            .worker_holder("")
            .or_else(|| manager.quick_lane_holder())
            .unwrap_or_else(|| "the work in the stack line".to_string())
    });
    holder
        .or_else(|| state.interactive_job_name())
        .or_else(|| {
            (!automatic::quiet_for_automatic_work(state))
                .then(|| "a quiet moment after your work".to_string())
        })
        .or_else(|| {
            let ctx = state.get_database(database_id)?;
            crate::server::quality_backfill::snapshot(&ctx.quality_backfill)
                .running
                .then(|| format!("the quality analysis of {}", ctx.name))
        })
}

/// A re-apply whose run has just joined the line is listed as that run,
/// not twice.
pub(super) fn reapplies_not_started(
    waiting: Vec<WaitingReapplyView>,
    active: &[StackActivityEntry],
) -> Vec<WaitingReapplyView> {
    waiting
        .into_iter()
        .filter(|reapply| {
            !active.iter().any(|entry| {
                entry.automatic != reapply.personal
                    && entry.kind == StackActivityKind::Processing
                    && entry.database_id == reapply.database_id
                    && entry.project_id == reapply.project_id
                    && entry.label == reapply.label
            })
        })
        .collect()
}

/// A waiting refresh whose build has just joined the line is listed as that
/// build, not twice.
pub(super) fn without_started(
    scheduled: Vec<ScheduledRefreshView>,
    active: &[StackActivityEntry],
) -> Vec<ScheduledRefreshView> {
    scheduled
        .into_iter()
        .filter(|refresh| {
            !refresh.waiting
                || !active.iter().any(|entry| {
                    entry.automatic
                        && entry.kind != StackActivityKind::Processing
                        && entry.database_id == refresh.database_id
                        && refresh
                            .project_id
                            .is_none_or(|project_id| project_id == entry.project_id)
                })
        })
        .collect()
}

/// Re-applies of processing out of the line, waiting for their turn.
pub(super) fn waiting_reapplies(
    state: &AppState,
    scheduled: &[ScheduledRefreshView],
) -> Vec<WaitingReapplyView> {
    let mut waiting = state.stack_previews.waiting_reapplies();
    waiting.sort_by_key(|(since, _)| *since);
    waiting
        .into_iter()
        .map(|(_, mut view)| {
            if view.personal {
                // A person's waits only for their re-apply ahead of it.
                view.waiting_for = state
                    .stack_previews
                    .personal_reapply_in_line()
                    .map(|label| format!("the processing of {label}"));
                return view;
            }
            view.waiting_for =
                automatic_work_waits_for(state, &view.database_id, true).or_else(|| {
                    // A refresh of its project may bring a newer stack first.
                    scheduled
                        .iter()
                        .any(|refresh| {
                            refresh.database_id == view.database_id
                                && refresh
                                    .project_id
                                    .is_none_or(|project_id| project_id == view.project_id)
                        })
                        .then(|| "its project's stacks to refresh".to_string())
                });
            view
        })
        .collect()
}

pub(super) fn name_scheduled(
    state: &AppState,
    scheduled: Vec<automatic::ScheduledRefresh>,
    planning: RefreshPlanning,
) -> Vec<ScheduledRefreshView> {
    // Soonest first: a later refresh whose channels an earlier one of the
    // same database already restacks would find nothing left, so it is not
    // listed either.
    let mut claimed: HashSet<(String, String)> = HashSet::new();
    scheduled
        .into_iter()
        .filter_map(|refresh| {
            let ctx = state.get_database(&refresh.database_id)?;
            let expected = match planning {
                RefreshPlanning::Plan => Some(automatic::expected_channels(&ctx, &refresh)),
                RefreshPlanning::CachedOnly => automatic::cached_expected_channels(&refresh),
            };
            // Not planned yet: a refresh due and waiting is listed, since
            // what it restacks cannot be told yet.
            let unknown = expected.is_none() && refresh.waiting;
            let expected = expected.unwrap_or_default();
            let fresh = expected
                .iter()
                .filter(|(key, _)| claimed.insert((refresh.database_id.clone(), key.clone())))
                .count();
            let channels = expected
                .into_iter()
                .map(|(_, name)| name)
                .collect::<Vec<_>>();
            (fresh > 0 || unknown).then_some((refresh, channels))
        })
        .map(|(refresh, channels)| {
            let ctx = state.get_database(&refresh.database_id);
            let project_name = match (&ctx, refresh.project_id) {
                // Not waiting for the shared connection: a busy one leaves
                // the name out, and the row shows the project's number.
                (Some(ctx), Some(project_id)) => ctx.db().try_lock().ok().and_then(|conn| {
                    conn.query_row(
                        "SELECT name FROM project WHERE Id = ?1",
                        [project_id],
                        |row| row.get::<_, String>(0),
                    )
                    .ok()
                }),
                _ => None,
            };
            let waiting_for = refresh
                .waiting
                .then(|| automatic_work_waits_for(state, &refresh.database_id, false))
                .flatten();
            ScheduledRefreshView {
                database_name: ctx
                    .map(|ctx| ctx.name.clone())
                    .unwrap_or_else(|| refresh.database_id.clone()),
                database_id: refresh.database_id,
                project_id: refresh.project_id,
                project_name,
                reason: refresh.reason,
                due_in_seconds: refresh.due_in_seconds,
                channels,
                waiting: refresh.waiting,
                waiting_for,
            }
        })
        .collect()
}

#[derive(Debug, Deserialize)]
pub struct ScheduledRefreshRequest {
    pub database_id: String,
    #[serde(default)]
    pub project_id: Option<i32>,
}

impl ScheduledRefreshRequest {
    fn key(self) -> automatic::RefreshKey {
        automatic::RefreshKey {
            database_id: self.database_id,
            project_id: self.project_id,
        }
    }
}

/// `POST /api/stack-activity/scheduled/skip` — drop a waiting automatic
/// refresh. The next arrival or grade change schedules it again.
pub async fn skip_scheduled_refresh(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ScheduledRefreshRequest>,
) -> Result<Json<ApiResponse<StackActivity>>, AppError> {
    if !state.auto_stacks.skip(&request.key()) {
        return Err(AppError::NotFound);
    }
    Ok(get_stack_activity(State(state)).await)
}

/// `POST /api/stack-activity/scheduled/run-now` — start a waiting automatic
/// refresh at the scheduler's next look, within seconds.
pub async fn run_scheduled_refresh_now(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ScheduledRefreshRequest>,
) -> Result<Json<ApiResponse<StackActivity>>, AppError> {
    if !state.auto_stacks.run_now(&request.key()) {
        return Err(AppError::NotFound);
    }
    Ok(get_stack_activity(State(state)).await)
}

#[derive(Debug, Deserialize)]
pub struct MoveStackJobRequest {
    /// Place in the line of waiting builds, 0 next.
    pub position: usize,
}

/// `POST /api/stack-activity/{job_id}/move` — change when a waiting build runs.
pub async fn move_stack_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
    Json(request): Json<MoveStackJobRequest>,
) -> Result<Json<ApiResponse<StackActivity>>, AppError> {
    validate_job_id(&job_id)?;
    if !state.stack_previews.move_in_line(&job_id, request.position) {
        return Err(AppError::BadRequest(
            "This build is not waiting; only a waiting build can move".into(),
        ));
    }
    Ok(get_stack_activity(State(state)).await)
}

/// `POST /api/stack-activity/{job_id}/cancel` — stop a mono or color build,
/// or a processing run, from any view, waiting or running.
pub async fn cancel_stack_activity_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Json<ApiResponse<StackActivity>>, AppError> {
    validate_job_id(&job_id)?;
    // A running color composition stops at its next channel or step, and a
    // running RC-Astro tool in it is killed.
    // A running stretch or deconvolution has no point to stop at; it
    // finishes in moments. An RC-Astro run stops its tool at once.
    if state.stack_previews.processing_unstoppable(&job_id) {
        return Err(AppError::BadRequest(
            "A running stretch or deconvolution cannot be stopped; it finishes shortly".into(),
        ));
    }
    if !state.stack_previews.request_cancel(&job_id) {
        return Err(AppError::BadRequest(
            "This build has already finished".into(),
        ));
    }
    Ok(get_stack_activity(State(state)).await)
}
