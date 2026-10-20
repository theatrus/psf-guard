//! The stack line's plumbing: putting builds, renders, compositions and
//! processing runs in the line, how its dispatchers run them, and a build's
//! request, prepared again at its turn, with what happens when a person's
//! work stops it.

use super::*;

/// Queue a previewed build: at the back of the line, or in the place
/// `slot` held while the request it came from was previewed. The line
/// carries `request`, the build's own channel request, and the build is
/// prepared again from it at its turn; `prepared` is the preview, which
/// gives the build the keys it holds while it waits.
pub(super) fn enqueue_job(
    state: Arc<AppState>,
    prepared: PreparedJob,
    request: &StackPreviewRequest,
    origin: journal::JournaledStackJob,
    slot: Option<&str>,
    preview: build_set::PreviewChannels,
) {
    // While it waits and while it runs, compositions of its targets wait.
    let mut targets = prepared
        .public
        .groups
        .iter()
        .map(|group| group.target_id)
        .collect::<Vec<_>>();
    targets.sort_unstable();
    targets.dedup();
    let database_id = prepared.public.database_id.clone();
    let holds = targets
        .into_iter()
        .map(|target_id| channel_key(&database_id, prepared.public.project_id, target_id))
        .collect();
    let spec = BuildRequest {
        job_id: prepared.public.job_id.clone(),
        database_id,
        project_id: prepared.public.project_id,
        request: request.clone(),
        preview,
    };
    submit_build(state, spec, holds, origin, prepared.public.automatic, slot);
}

/// Put a build in the line from its request. An automatic build that a
/// person's work stops goes back in the line as its request, and resumes
/// from its checkpoint at its turn.
pub(super) fn submit_build(
    state: Arc<AppState>,
    spec: BuildRequest,
    holds: Vec<String>,
    origin: journal::JournaledStackJob,
    automatic: bool,
    slot: Option<&str>,
) {
    let job_id = spec.job_id.clone();
    let cancel = state.stack_previews.track_job(&job_id);
    state
        .stack_previews
        .remember_origin(&job_id, origin.clone());
    // A stop waits for the build to checkpoint, or to leave the line.
    let work = state.shutdown.hold(format!(
        "stack build for project {} (db={})",
        spec.project_id, spec.database_id
    ));
    let (stopped_job, failed_job) = (job_id.clone(), job_id.clone());
    let again = (spec.clone(), holds.clone(), origin);
    let submission = Submission::<BuildWork>::new(
        "a stack build",
        &spec,
        move |state, built| match built {
            Ok(Some(output)) => publish_build(state, output),
            // Prepared at its turn into another build, or failed there.
            Ok(None) => {}
            Err(error) => state.stack_previews.update(&failed_job, |job| {
                job.state = StackJobState::Failed;
                job.error = Some(format!("Stack worker failed: {error}"));
            }),
        },
        move |state| {
            // Stopped while it waited: the work never starts.
            state.stack_previews.update(&stopped_job, |job| {
                cancel_unfinished_groups(job);
                job.state = StackJobState::Cancelled;
            });
        },
    )
    .map(|submission| {
        let (job_id, cancel) = (job_id.clone(), cancel.clone());
        // A stop that lands as its turn comes still keeps it from starting.
        let start_stop = cancel.clone();
        submission
            .starting_with(move |_| !start_stop.requested())
            .finally(move |state, ending| {
                if let crate::server::job_line::Ending::Panicked(error) = ending {
                    state.stack_previews.update(&job_id, |job| {
                        job.state = StackJobState::Failed;
                        job.error = Some(format!("Stack worker panicked: {error}"));
                    });
                }
                state.stack_previews.forget_job_of(&job_id, &cancel);
                drop(work);
                // Stopped for a person's work: back in the line as its
                // request, unless the server is stopping, when the journal
                // keeps it for the next start.
                if state.stack_previews.line.take_preempted(&job_id)
                    && !state.shutdown.is_stopping()
                    && requeue_stopped_build(state, &job_id)
                {
                    let (spec, holds, mut origin) = again;
                    // A person may have adopted it while it stopped.
                    let automatic = state.stack_previews.is_automatic(&job_id);
                    origin.set_automatic(automatic);
                    submit_build(Arc::clone(state), spec, holds, origin, automatic, None);
                    if automatic {
                        state.stack_previews.line.move_ahead_of_automatic(&job_id);
                    }
                }
            })
    });
    match submission {
        Ok(submission) => submit(
            &state,
            &job_id,
            Lane::Main,
            slot,
            cancel,
            holds,
            automatic,
            Box::new(submission),
        ),
        Err(error) => {
            state.stack_previews.update(&job_id, |job| {
                job.state = StackJobState::Failed;
                job.error = Some(error);
            });
            state.stack_previews.forget_job_of(&job_id, &cancel);
        }
    }
}

/// Make a build that a person's work stopped queued again, with its
/// stopped channels waiting once more. False when it did not stop, such as
/// one that finished before the stop reached it: it is not queued again.
pub(super) fn requeue_stopped_build(state: &AppState, job_id: &str) -> bool {
    let mut requeued = false;
    state.stack_previews.update(job_id, |job| {
        if job.state != StackJobState::Cancelled {
            return;
        }
        job.state = StackJobState::Queued;
        for group in &mut job.groups {
            if group.state == StackGroupState::Cancelled {
                group.state = StackGroupState::Queued;
                group.phase = "queued".into();
            }
        }
        requeued = true;
    });
    requeued
}

/// What a build holds in the line, waiting or running, for each target it
/// stacks: a color composition of that target waits until nothing does.
pub(super) fn channel_key(database_id: &str, project_id: i32, target_id: i32) -> String {
    format!("channels:{database_id}:{project_id}:{target_id}")
}

/// What a build still being prepared holds: its targets are not known
/// yet, so it holds every target of its project.
pub(super) fn project_channels_key(database_id: &str, project_id: i32) -> String {
    format!("channels:{database_id}:{project_id}:*")
}

/// Put work in the line under `job_id`, stopped by `cancel`: on the worker
/// lane in the place `before` held, or at the back. The lane's dispatcher
/// runs it in its turn.
#[allow(clippy::too_many_arguments)]
pub(super) fn submit(
    state: &Arc<AppState>,
    job_id: &str,
    lane: Lane,
    before: Option<&str>,
    cancel: Stop,
    holds: Vec<String>,
    automatic: bool,
    task: Box<dyn Task>,
) {
    submit_entry(
        state,
        Entry::new(job_id, lane, cancel, task)
            .holding(holds)
            .tier(tier_of(automatic)),
        before,
    );
}

/// The line's tier for work the automatic refresh started, or a person's.
pub(super) fn tier_of(automatic: bool) -> Tier {
    if automatic {
        Tier::Automatic
    } else {
        Tier::Person
    }
}

/// Put `entry` in the line, as [`submit`] does.
pub(super) fn submit_entry(state: &Arc<AppState>, entry: Entry, before: Option<&str>) {
    ensure_dispatching(state);
    let job_id = entry.id().to_string();
    if let Err(entry) = state.stack_previews.line.submit(entry, before) {
        tracing::warn!(
            job_id,
            "The same work is already in line; this copy is dropped"
        );
        drop(entry);
    }
}

/// How the stack line runs its entries. A person's work runs at a higher
/// priority than pre-generation and quality scans, and pauses background
/// work while it runs.
pub(super) static STACK_LINE: DispatchRules = DispatchRules {
    name: "stack",
    line: |state| &state.stack_previews.line,
    lanes: &[Lane::Main, Lane::Quick],
    turn: |state, id| {
        let automatic = state.stack_previews.is_automatic(id)
            || state.stack_previews.is_automatic_processing(id);
        Turn {
            priority: job_priority(automatic),
            pauses_background_work: !automatic,
        }
    },
    // A person's work anywhere on the server comes first. A person's next
    // build that joins after automatic work started stops it, and the
    // automatic build resumes from its checkpoint.
    automatic_may_start: |state| !state.interactive_job_active(),
};

/// Start each lane's dispatcher if it is not running.
pub(super) fn ensure_dispatching(state: &Arc<AppState>) {
    state
        .stack_previews
        .line
        .ensure_dispatching(state, &STACK_LINE);
}

/// A stack build on the line.
pub(super) struct BuildWork;

impl LineWork for BuildWork {
    type Spec = BuildRequest;
    /// `None` when the build was prepared at its turn into another build,
    /// or could not be prepared.
    type Output = Option<BuildOutput>;
    /// It reads this server's catalog to prepare the build at its turn,
    /// opens the calibration catalog to choose and build masters, and
    /// reads its WCS and score caches.
    const NEEDS: Needs = Needs::ThisServer;

    fn execute(spec: BuildRequest, worker: &Worker<'_>) -> Option<BuildOutput> {
        let prepared = prepare_at_turn(&spec, worker.server(), worker.stop_flag())?;
        Some(execute_build(prepared, worker))
    }
}

/// A build in the line: its channel request, prepared at its turn.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct BuildRequest {
    /// The id the build was listed under when it was previewed.
    job_id: String,
    database_id: String,
    project_id: i32,
    request: StackPreviewRequest,
    /// Which channels the preview was prepared with.
    #[serde(default)]
    preview: build_set::PreviewChannels,
}

/// Prepare a build again from its request, as the catalog is now. Returns
/// the prepared build when it is the one that was previewed. When frames,
/// grades or calibration changed while it waited, the request becomes
/// another build: that build is queued at the front of the line, or a
/// matching finished build is reused, and the previewed record points to
/// it. A build that cannot be prepared fails with the reason.
pub(super) fn prepare_at_turn(
    spec: &BuildRequest,
    state: &Arc<AppState>,
    stop: &Stop,
) -> Option<PreparedJob> {
    let fail = |error: String| {
        state.stack_previews.update(&spec.job_id, |job| {
            cancel_unfinished_groups(job);
            job.state = StackJobState::Failed;
            job.error = Some(error);
        });
    };
    let Some(ctx) = state.get_database(&spec.database_id) else {
        fail("The database was removed while the build waited".into());
        return None;
    };
    // It holds the lane from here: listed as running, not as queued.
    state.stack_previews.update(&spec.job_id, |job| {
        job.state = StackJobState::Running;
    });
    let prepared = match build_set::prepare_like_preview(
        &ctx,
        spec.project_id,
        &spec.request,
        &spec.preview,
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            fail(error.message());
            return None;
        }
    };
    if prepared.public.job_id == spec.job_id {
        return Some(prepared);
    }
    // Stopped while it was read, by a person or for a person's work, or the
    // server is stopping: no other build takes its place. A preempted build
    // goes back in the line as its request.
    if stop.requested() || state.shutdown.is_stopping() {
        state.stack_previews.update(&spec.job_id, |job| {
            cancel_unfinished_groups(job);
            job.state = StackJobState::Cancelled;
        });
        return None;
    }
    let automatic = state.stack_previews.is_automatic(&spec.job_id);
    let placement = build_set::Placement {
        automatic,
        slot: None,
        preview: spec.preview.clone(),
    };
    match build_set::start_prepared(
        state,
        &ctx,
        spec.project_id,
        &spec.request,
        prepared,
        placement,
    ) {
        Ok(started) => {
            let became = started.job.job_id.clone();
            tracing::info!(
                job_id = spec.job_id,
                became,
                "A queued build's frames changed while it waited; it runs as a new build"
            );
            // A person's build keeps the turn the previewed build had; an
            // automatic one goes ahead of the other automatic work only.
            if automatic {
                state.stack_previews.line.move_ahead_of_automatic(&became);
            } else {
                state.stack_previews.move_in_line(&became, 0);
            }
            state.stack_previews.update(&spec.job_id, |job| {
                job.state = StackJobState::Replaced;
                job.replaced_by = vec![became.clone()];
            });
            if let Some(cached) = started.cached {
                automatic::compose_colors_after(state, &ctx, &cached);
            }
        }
        Err(error) => fail(error.message()),
    }
    None
}

/// Where a finished build's outputs are, for publishing.
pub(super) struct BuildOutput {
    pub(super) job_id: String,
    pub(super) database_id: String,
    pub(super) stack_root: PathBuf,
}
