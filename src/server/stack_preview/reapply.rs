//! Re-apply a channel's processing when its stack refreshes.
//!
//! Processing (a stretch, deconvolution or RC-Astro chain) belongs to one
//! revision of a stack. When a build publishes a new revision of a channel
//! whose previous revision showed processing, whether an automatic refresh or
//! a person's rebuild, the same request is queued against the new revision as
//! automatic work, with no browser needed. It waits until no refresh of its
//! project is still settling and nothing else wants the worker, steps aside
//! for work a person starts, and only ever runs on the channel's newest
//! revision. Meanwhile the card shows the previous result, marked as such.
//! Each revision carries once: a re-apply that fails is not tried again.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::stretch::{self, CarryState, StackViewProcessingRequest, StretchSource};
use super::{
    latest_path, LatestStackPreviewGroup, LatestStackPreviews, StackGroupStatus, StackJobState,
    StackPreviewManager,
};
use crate::server::database_context::DatabaseContext;
use crate::server::state::AppState;

static ENABLED: AtomicBool = AtomicBool::new(true);

/// Whether refreshed stacks get their processing again. On by default.
pub fn configure(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// How long a re-apply waits after the work that freed the worker ends,
/// so that work's own bookkeeping is done before it looks.
const SETTLE: Duration = Duration::from_secs(3);

/// A re-apply waiting for its turn, or queued as a run.
#[derive(Debug, Clone)]
pub(super) struct Reapply {
    pub database_id: String,
    pub project_id: i32,
    /// The channel's target, and the channel as people read it, for the
    /// header while the re-apply waits.
    pub target_id: Option<i32>,
    pub label: String,
    pub job_id: String,
    pub group_index: usize,
    pub revision: String,
    pub source_key: String,
    pub request: StackViewProcessingRequest,
    /// The run, once queued.
    pub run_id: Option<String>,
    pub since: Instant,
}

/// How a processing run ended, for the re-apply it may be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RunOutcome {
    Published,
    Failed(String),
    /// Taken out of the line, stopped, or superseded.
    Stopped,
    /// A server stop cut it off; the journal brings it back.
    Interrupted,
}

/// One channel of one project, whatever revision its stack is at.
fn channel_key(database_id: &str, project_id: i32, group: &StackGroupStatus) -> String {
    channel_of(
        database_id,
        project_id,
        group.target_id,
        &group.filter_name,
        group
            .exposure_group
            .as_ref()
            .map(|exposure| exposure.label.as_str()),
    )
}

fn scope_channel(scope: &super::processing_queue::ProcessingScope) -> String {
    channel_of(
        &scope.database_id,
        scope.project_id,
        scope.target_id,
        &scope.channel,
        scope.exposure.as_deref(),
    )
}

fn channel_of(
    database_id: &str,
    project_id: i32,
    target_id: i32,
    filter: &str,
    exposure: Option<&str>,
) -> String {
    format!(
        "{database_id}\0{project_id}\0{target_id}\0{filter}\0{}",
        exposure.unwrap_or_default()
    )
}

fn source_key(job_id: &str, group_index: usize) -> String {
    format!("mono:{job_id}:{group_index}")
}

/// `M44 · R`, or `M44 · R · 300 s` with an exposure band.
fn group_label(group: &StackGroupStatus) -> String {
    let mut label = super::channel_label(&group.target_name, &group.filter_name);
    if let Some(exposure) = &group.exposure_group {
        label.push_str(" · ");
        label.push_str(&exposure.label);
    }
    label
}

impl StackPreviewManager {
    /// Re-applies waiting for their turn rather than queued as runs, with
    /// when each began to wait. What each waits for is the caller's to say.
    pub(super) fn waiting_reapplies(&self) -> Vec<(Instant, super::WaitingReapplyView)> {
        self.reapplies
            .lock()
            .unwrap()
            .values()
            .filter(|reapply| reapply.run_id.is_none())
            .map(|reapply| {
                (
                    reapply.since,
                    super::WaitingReapplyView {
                        database_id: reapply.database_id.clone(),
                        project_id: reapply.project_id,
                        target_id: reapply.target_id,
                        label: reapply.label.clone(),
                        waiting_for: None,
                    },
                )
            })
            .collect()
    }

    fn reapply_for_run(&self, run_id: &str) -> Option<(String, Reapply)> {
        self.reapplies
            .lock()
            .unwrap()
            .iter()
            .find(|(_, reapply)| reapply.run_id.as_deref() == Some(run_id))
            .map(|(channel, reapply)| (channel.clone(), reapply.clone()))
    }

    fn reapply_for_source(&self, key: &str, revision: &str) -> Option<(String, Reapply)> {
        self.reapplies
            .lock()
            .unwrap()
            .iter()
            .find(|(_, reapply)| reapply.source_key == key && reapply.revision == revision)
            .map(|(channel, reapply)| (channel.clone(), reapply.clone()))
    }

    fn forget_reapply(&self, channel: &str) {
        self.reapplies.lock().unwrap().remove(channel);
    }

    /// Re-applies the server is waiting to start or running.
    #[cfg(test)]
    pub(super) fn reapply_count(&self) -> usize {
        self.reapplies.lock().unwrap().len()
    }
}

/// Whether a stack is still the newest its channel has: what the card shows.
pub(super) fn is_latest(ctx: &DatabaseContext, project_id: i32, source: &StretchSource) -> bool {
    std::fs::read(latest_path(&ctx.stack_root, project_id))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<LatestStackPreviews>(&bytes).ok())
        .is_some_and(|latest| {
            latest.groups.iter().any(|entry| {
                entry.job_id == source.job_id
                    && entry.group.index == source.group_index
                    && entry.artifact_revision == source.revision
            })
        })
}

/// A build published new revisions of these channels, each over the one the
/// card showed before. Carry each previous revision's processing over and
/// queue it again; drop a re-apply still waiting on an older revision.
pub(super) fn after_build(
    state: &Arc<AppState>,
    ctx: &DatabaseContext,
    project_id: i32,
    replaced: Vec<(LatestStackPreviewGroup, LatestStackPreviewGroup)>,
) {
    let manager = &state.stack_previews;
    let mut added = false;
    for (previous, current) in replaced {
        let channel = channel_key(&ctx.id, project_id, &current.group);
        let current_key = source_key(&current.job_id, current.group.index);
        let previous_key = source_key(&previous.job_id, previous.group.index);
        // Read what the previous card shows before its own re-apply, if it
        // was still waiting, is dropped: that carried result moves on.
        let carried = if enabled() {
            stretch::carry_processing(
                &ctx.stack_root,
                &previous_key,
                &previous.artifact_revision,
                &current_key,
                &current.artifact_revision,
            )
        } else {
            Ok(None)
        };
        drop_stale(
            state,
            ctx,
            &channel,
            &current_key,
            &current.artifact_revision,
        );
        match carried {
            Ok(Some((request, _))) => {
                tracing::info!(
                    db = %ctx.id,
                    project_id,
                    "{} refreshed; its processing will run again on the new stack",
                    super::channel_label(&current.group.target_name, &current.group.filter_name)
                );
                manager.reapplies.lock().unwrap().insert(
                    channel,
                    Reapply {
                        database_id: ctx.id.clone(),
                        project_id,
                        target_id: Some(current.group.target_id),
                        label: group_label(&current.group),
                        job_id: current.job_id.clone(),
                        group_index: current.group.index,
                        revision: current.artifact_revision.clone(),
                        source_key: current_key,
                        request,
                        run_id: None,
                        since: Instant::now(),
                    },
                );
                added = true;
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(
                db = %ctx.id,
                project_id,
                "could not carry processing to a refreshed stack: {error}"
            ),
        }
    }
    if added {
        kick(state);
    }
}

/// Only the newest revision of a channel is re-applied. A re-apply for an
/// older one is dropped, and its run stopped: an RC-Astro tool is killed.
/// Its cached steps were made from other pixels, so nothing of it carries.
fn drop_stale(
    state: &Arc<AppState>,
    ctx: &DatabaseContext,
    channel: &str,
    current_key: &str,
    current_revision: &str,
) {
    let manager = &state.stack_previews;
    let stale = manager
        .reapplies
        .lock()
        .unwrap()
        .get(channel)
        .filter(|reapply| reapply.source_key != current_key || reapply.revision != current_revision)
        .cloned();
    let Some(stale) = stale else {
        return;
    };
    manager.forget_reapply(channel);
    if let Some(run_id) = &stale.run_id {
        manager.request_cancel(run_id);
    }
    if let Err(error) = stretch::end_carry(
        &ctx.stack_root,
        &stale.source_key,
        &stale.revision,
        CarryState::Stopped,
        None,
    ) {
        tracing::warn!("could not mark a superseded re-apply: {error}");
    }
    tracing::info!(
        db = %ctx.id,
        project_id = stale.project_id,
        "A newer stack replaced the one a re-apply was waiting for; it is dropped"
    );
}

/// Look for a re-apply to start once whatever just ended has let go of the
/// worker.
pub(super) fn kick(state: &Arc<AppState>) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let state = Arc::clone(state);
    handle.spawn(async move {
        tokio::time::sleep(SETTLE).await;
        start_due(state).await;
    });
}

/// Start the oldest waiting re-apply, if the worker has nothing else to do
/// and no refresh of its project is still settling. One at a time: the run
/// it queues keeps the next one waiting.
pub(super) async fn start_due(state: Arc<AppState>) {
    static STARTING: AtomicBool = AtomicBool::new(false);
    if STARTING.swap(true, Ordering::AcqRel) {
        return;
    }
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            STARTING.store(false, Ordering::Release);
        }
    }
    let _release = Release;
    if !enabled() || !super::journal::restored() || state.shutdown.is_stopping() {
        return;
    }
    // Behind anything a person started and every build.
    if state.interactive_job_active() || !state.stack_previews.active().is_empty() {
        return;
    }
    let scheduled = state.auto_stacks.scheduled();
    let mut waiting = state
        .stack_previews
        .reapplies
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, reapply)| reapply.run_id.is_none())
        .map(|(channel, reapply)| (channel.clone(), reapply.clone()))
        .collect::<Vec<_>>();
    waiting.sort_by_key(|(_, reapply)| reapply.since);
    for (channel, reapply) in waiting {
        // A refresh still settling may bring a newer stack within minutes:
        // starting RC-Astro now would only be thrown away.
        if scheduled.iter().any(|refresh| {
            refresh.database_id == reapply.database_id
                && refresh
                    .project_id
                    .is_none_or(|project_id| project_id == reapply.project_id)
        }) {
            continue;
        }
        let manager = &state.stack_previews;
        let Some(ctx) = state.get_database(&reapply.database_id) else {
            manager.forget_reapply(&channel);
            continue;
        };
        let current =
            super::stack_processing_source(&state, &ctx, &reapply.job_id, reapply.group_index)
                .ok()
                .filter(|source| {
                    source.revision == reapply.revision
                        && is_latest(&ctx, reapply.project_id, source)
                });
        if current.is_none() {
            manager.forget_reapply(&channel);
            let _ = stretch::end_carry(
                &ctx.stack_root,
                &reapply.source_key,
                &reapply.revision,
                CarryState::Stopped,
                None,
            );
            continue;
        }
        // Probing the tools takes seconds. Whatever happened to this
        // re-apply meanwhile (a person's Apply or Revert, a newer stack) is
        // checked again before it is queued.
        let still_wanted = {
            let state = Arc::clone(&state);
            let channel = channel.clone();
            let revision = reapply.revision.clone();
            move || {
                state
                    .stack_previews
                    .reapplies
                    .lock()
                    .unwrap()
                    .get(&channel)
                    .is_some_and(|entry| entry.revision == revision && entry.run_id.is_none())
            }
        };
        match stretch::start_reapply(
            &state,
            &ctx,
            &reapply.job_id,
            reapply.group_index,
            &reapply.revision,
            reapply.request.clone(),
            &still_wanted,
        )
        .await
        {
            Ok(stretch::ReapplyStart::Withdrawn) => {
                // Its own record already went with whatever replaced it.
                let mut reapplies = manager.reapplies.lock().unwrap();
                if reapplies.get(&channel).is_some_and(|entry| {
                    entry.revision == reapply.revision && entry.run_id.is_none()
                }) {
                    reapplies.remove(&channel);
                }
            }
            Ok(stretch::ReapplyStart::Queued(run_id)) => {
                if let Some(entry) = manager.reapplies.lock().unwrap().get_mut(&channel)
                    && entry.revision == reapply.revision
                {
                    entry.run_id = Some(run_id);
                }
                return;
            }
            Ok(stretch::ReapplyStart::Done) => {
                manager.forget_reapply(&channel);
                let _ = stretch::carry_published(
                    &ctx.stack_root,
                    &reapply.source_key,
                    &reapply.revision,
                );
            }
            Err(error) => {
                manager.forget_reapply(&channel);
                tracing::warn!(
                    db = %ctx.id,
                    project_id = reapply.project_id,
                    "Could not re-apply processing to a refreshed stack: {error}"
                );
                let _ = stretch::end_carry(
                    &ctx.stack_root,
                    &reapply.source_key,
                    &reapply.revision,
                    CarryState::Failed,
                    Some(error),
                );
            }
        }
    }
}

/// A processing run ended. If it was a re-apply, settle what its card shows:
/// the new result, the failure beside the previous result, or nothing
/// carried. One that stepped aside for a person's work waits again.
pub(super) fn run_finished(state: &Arc<AppState>, run_id: &str, outcome: RunOutcome) {
    let manager = &state.stack_previews;
    let yielded = manager.take_yielded(run_id);
    let Some((channel, reapply)) = manager.reapply_for_run(run_id) else {
        // Any run ending frees the worker for a re-apply waiting on it.
        if !manager.reapplies.lock().unwrap().is_empty() {
            kick(state);
        }
        return;
    };
    if outcome == RunOutcome::Interrupted {
        return;
    }
    if yielded && outcome != RunOutcome::Published && enabled() {
        if let Some(entry) = manager.reapplies.lock().unwrap().get_mut(&channel) {
            entry.run_id = None;
        }
        if let Some(ctx) = state.get_database(&reapply.database_id) {
            let _ =
                stretch::clear_carry_run(&ctx.stack_root, &reapply.source_key, &reapply.revision);
        }
        return;
    }
    // A re-apply that stepped aside after re-applying was turned off has no
    // turn coming: it settles as stopped below, like one turned off while
    // waiting, so its card stops saying it is reprocessing.
    manager.forget_reapply(&channel);
    let Some(ctx) = state.get_database(&reapply.database_id) else {
        return;
    };
    let settled = match &outcome {
        RunOutcome::Published => {
            stretch::carry_published(&ctx.stack_root, &reapply.source_key, &reapply.revision)
        }
        RunOutcome::Failed(error) => {
            tracing::warn!(
                db = %ctx.id,
                project_id = reapply.project_id,
                "Re-applying processing to a refreshed stack failed: {error}; the previous \
                 result stays"
            );
            stretch::end_carry(
                &ctx.stack_root,
                &reapply.source_key,
                &reapply.revision,
                CarryState::Failed,
                Some(error.clone()),
            )
        }
        _ => stretch::end_carry(
            &ctx.stack_root,
            &reapply.source_key,
            &reapply.revision,
            CarryState::Stopped,
            None,
        ),
    };
    if let Err(error) = settled {
        tracing::warn!("could not settle a re-apply's selection: {error}");
    }
    kick(state);
}

/// A person applied or reverted processing on this stack: their choice
/// replaces the re-apply. The same request takes the queued run over as
/// theirs; anything else stops it.
pub(super) fn replaced_by_person(
    state: &Arc<AppState>,
    source: &StretchSource,
    their_run: Option<&str>,
) {
    let manager = &state.stack_previews;
    let Some((channel, reapply)) = manager.reapply_for_source(&source.key, &source.revision) else {
        return;
    };
    manager.forget_reapply(&channel);
    if let Some(run_id) = &reapply.run_id {
        if their_run == Some(run_id.as_str()) {
            manager.adopt(run_id);
        } else {
            manager.request_cancel(run_id);
        }
    }
    tracing::info!(
        db = %reapply.database_id,
        project_id = reapply.project_id,
        "A person's processing replaced the re-apply queued for a refreshed stack"
    );
}

/// A journaled re-apply that will not run again after a restart: cut off by
/// too many restarts in a row, or no longer restorable. Its carry is
/// settled as failed with the reason, so its card stops waiting and
/// [`recover`] never brings it back with its attempts reset. Anything other
/// than a processing run is left alone.
pub(super) fn give_up(
    state: &Arc<AppState>,
    job: &super::journal::JournaledStackJob,
    reason: &str,
) {
    let super::journal::JournaledStackJob::Processing {
        database_id,
        job_id,
        group_index,
        revision,
        ..
    } = job
    else {
        return;
    };
    let Some(ctx) = state.get_database(database_id) else {
        return;
    };
    let key = source_key(job_id, *group_index);
    tracing::warn!(
        db = %database_id,
        "A re-apply of processing to a refreshed stack is given up: {reason}"
    );
    if let Err(error) = stretch::end_carry(
        &ctx.stack_root,
        &key,
        revision,
        CarryState::Failed,
        Some(format!("Re-applying was given up: {reason}")),
    ) {
        tracing::warn!("could not mark a re-apply as given up: {error}");
    }
}

/// The setting was turned off: re-applies not yet running are dropped and
/// their cards stop showing the previous result. A running one finishes.
pub(crate) fn turned_off(state: &Arc<AppState>) {
    let manager = &state.stack_previews;
    let all = manager
        .reapplies
        .lock()
        .unwrap()
        .iter()
        .map(|(channel, reapply)| (channel.clone(), reapply.clone()))
        .collect::<Vec<_>>();
    for (channel, reapply) in all {
        let running = reapply
            .run_id
            .as_deref()
            .and_then(|run_id| manager.processing_state(run_id))
            == Some(StackJobState::Running);
        if running {
            continue;
        }
        manager.forget_reapply(&channel);
        if let Some(run_id) = &reapply.run_id {
            manager.request_cancel(run_id);
        }
        if let Some(ctx) = state.get_database(&reapply.database_id) {
            let _ = stretch::end_carry(
                &ctx.stack_root,
                &reapply.source_key,
                &reapply.revision,
                CarryState::Stopped,
                None,
            );
        }
    }
}

/// A re-apply the job journal brought back as a run.
pub(super) fn restored(
    state: &Arc<AppState>,
    database_id: &str,
    project_id: i32,
    source: &StretchSource,
    request: StackViewProcessingRequest,
    run_id: &str,
) {
    state.stack_previews.reapplies.lock().unwrap().insert(
        scope_channel(&source.scope),
        Reapply {
            database_id: database_id.into(),
            project_id,
            target_id: Some(source.scope.target_id),
            label: source.scope.label(),
            job_id: source.job_id.clone(),
            group_index: source.group_index,
            revision: source.revision.clone(),
            source_key: source.key.clone(),
            request,
            run_id: Some(run_id.into()),
            since: Instant::now(),
        },
    );
}

/// After a restart, put back the re-applies that were still waiting for
/// their turn: the selection files remember them. Ones the journal already
/// restored as runs are known.
pub(super) fn recover(state: &Arc<AppState>) {
    let known = state
        .stack_previews
        .reapplies
        .lock()
        .unwrap()
        .values()
        .map(|reapply| (reapply.source_key.clone(), reapply.revision.clone()))
        .collect::<HashSet<_>>();
    let mut recovered = 0;
    for ctx in state.all_databases() {
        for waiting in stretch::waiting_carries(&ctx.stack_root) {
            if known.contains(&(waiting.key.clone(), waiting.revision.clone())) {
                continue;
            }
            let Some((job_id, group_index)) = waiting
                .key
                .strip_prefix("mono:")
                .and_then(|key| key.rsplit_once(':'))
                .and_then(|(job_id, index)| Some((job_id.to_string(), index.parse().ok()?)))
            else {
                continue;
            };
            let source = super::stack_processing_source(state, &ctx, &job_id, group_index)
                .ok()
                .filter(|source| source.revision == waiting.revision)
                .filter(|source| is_latest(&ctx, source.scope.project_id, source));
            let Some(source) = source else {
                let _ = stretch::end_carry(
                    &ctx.stack_root,
                    &waiting.key,
                    &waiting.revision,
                    CarryState::Stopped,
                    None,
                );
                continue;
            };
            state.stack_previews.reapplies.lock().unwrap().insert(
                scope_channel(&source.scope),
                Reapply {
                    database_id: ctx.id.clone(),
                    project_id: source.scope.project_id,
                    target_id: Some(source.scope.target_id),
                    label: source.scope.label(),
                    job_id,
                    group_index,
                    revision: waiting.revision,
                    source_key: waiting.key,
                    request: waiting.request,
                    run_id: None,
                    since: Instant::now(),
                },
            );
            recovered += 1;
        }
    }
    if recovered > 0 {
        tracing::info!("{recovered} re-apply(s) of processing to refreshed stacks wait again");
        kick(state);
    }
}
