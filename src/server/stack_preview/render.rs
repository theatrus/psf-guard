//! Rendering a finished build's card previews again from its FITS, when an
//! older stretch release or an out-of-date color calibration rendered them.

use super::*;

/// Whether a finished channel's card preview should be rendered again from
/// its FITS: an older stretch release rendered it, or a one-shot-color
/// stack's color calibration is out of date. The stack itself is current. A
/// render that already failed at this stretch release is not tried again.
pub(super) fn preview_needs_render(
    state: &AppState,
    ctx: &DatabaseContext,
    job_id: &str,
    group: &StackGroupStatus,
) -> bool {
    if group.state != StackGroupState::Ready
        || group.preview_render_failed.as_deref() == Some(stretch::SEIZA_STRETCH_OUTPUT_VERSION)
        || !fits_path(&ctx.stack_root, job_id, group.index).is_file()
    {
        return false;
    }
    let rendered_by = group
        .preview_stretch_version
        .as_deref()
        .unwrap_or(JOB_ID_STRETCH_VERSION);
    rendered_by != stretch::SEIZA_STRETCH_OUTPUT_VERSION
        || preview_calibration_outdated(group, || {
            osc_calibration_source(
                state,
                ctx,
                group.reference_image_id,
                group.sky_orientation.clone(),
                &fits_path(&ctx.stack_root, job_id, group.index),
            )
        })
        .is_some()
}

/// Queue a render of a finished build's out-of-date card previews from the
/// FITS it wrote, when any are out of date, and return the job as it then
/// stands. The stack is not built again. The job runs in the build line
/// while it renders and is completed again afterwards, stopped or not.
/// `automatic` is false for a person's Build, which makes the job theirs;
/// `color_defaults` replaces the job's stored defaults when given. Reads
/// files, so call it off the async runtime where that matters.
pub(super) fn queue_preview_render(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    finished: &StackPreviewJob,
    automatic: bool,
    color_defaults: Option<color::StackColorProcessing>,
) -> Option<StackPreviewJob> {
    let stale = finished
        .groups
        .iter()
        .filter(|group| preview_needs_render(state, ctx, &finished.job_id, group))
        .map(|group| group.index)
        .collect::<Vec<_>>();
    if stale.is_empty()
        || !state
            .stack_previews
            .begin_preview_render(finished, &stale, automatic, color_defaults)
    {
        return None;
    }
    let job_id = finished.job_id.clone();
    let cancel = state.stack_previews.track_job(&job_id);
    let work = state.shutdown.hold(format!(
        "stack preview render for project {} (db={})",
        finished.project_id, finished.database_id
    ));
    // However it ends, the build settles as the finished build it was.
    let (settle_ctx, settle_job) = (Arc::clone(ctx), job_id.clone());
    let (unrun_ctx, unrun_job) = (Arc::clone(ctx), job_id.clone());
    let failed_job = job_id.clone();
    let spec = RenderSpec {
        database_id: ctx.id.clone(),
        job_id: job_id.clone(),
        stale,
    };
    let submission = Submission::<RenderWork>::new(
        "a stack preview render",
        &spec,
        move |state, rendered| {
            if let Err(error) = rendered {
                tracing::warn!(job_id = failed_job, "Stack preview render failed: {error}");
            }
            finish_preview_render(state, &settle_ctx, &settle_job);
        },
        move |state| finish_preview_render(state, &unrun_ctx, &unrun_job),
    )
    .map(|submission| {
        let (job_id, cancel) = (job_id.clone(), cancel.clone());
        let database_id = ctx.id.clone();
        submission.finally(move |state, _| {
            state.stack_previews.forget_job_of(&job_id, &cancel);
            drop(work);
            // Stopped for a person's work: render what is still out of date
            // again, after it.
            if state.stack_previews.line.take_preempted(&job_id)
                && !state.shutdown.is_stopping()
                && let Some(finished) = state.stack_previews.get(&job_id)
                && let Some(ctx) = state.get_database(&database_id)
            {
                let automatic = state.stack_previews.is_automatic(&job_id);
                queue_preview_render(state, &ctx, &finished, automatic, None);
                if automatic {
                    state.stack_previews.line.move_ahead_of_automatic(&job_id);
                }
            }
        })
    });
    match submission {
        Ok(submission) => submit(
            state,
            &job_id,
            Lane::Main,
            None,
            cancel,
            Vec::new(),
            automatic,
            Box::new(submission),
        ),
        Err(error) => {
            tracing::warn!(job_id, "Stack preview render could not be queued: {error}");
            finish_preview_render(state, ctx, &job_id);
            state.stack_previews.forget_job_of(&job_id, &cancel);
        }
    }
    state.stack_previews.get(&job_id)
}

/// A finished build's card previews rendered again from its FITS.
pub(super) struct RenderWork;

#[derive(Serialize, Deserialize)]
pub(super) struct RenderSpec {
    database_id: String,
    job_id: String,
    /// The channels whose previews are out of date.
    stale: Vec<usize>,
}

impl LineWork for RenderWork {
    type Spec = RenderSpec;
    type Output = ();
    /// It reads the build's FITS from this server's stack cache and records
    /// each channel's progress on the build.
    const NEEDS: Needs = Needs::ThisServer;

    fn execute(spec: RenderSpec, worker: &Worker<'_>) {
        let state = worker.server();
        let Some(ctx) = state.get_database(&spec.database_id) else {
            return;
        };
        let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            render_stale_previews(state, &ctx, &spec.job_id, &spec.stale, worker.stop_flag())
        }));
        if rendered.is_err() {
            tracing::warn!(job_id = spec.job_id, "Stack preview render panicked");
        }
    }
}

/// Render each listed channel's card previews again from its FITS. A
/// channel that cannot be rendered keeps the preview it had, says why on
/// its card, and is not tried again at this stretch release.
pub(super) fn render_stale_previews(
    state: &Arc<AppState>,
    ctx: &DatabaseContext,
    job_id: &str,
    stale: &[usize],
    cancel: &Stop,
) {
    state.stack_previews.update(job_id, |job| {
        job.state = StackJobState::Running;
    });
    let priority = job_priority(state.stack_previews.is_automatic(job_id));
    let worker_policy = state.worker_policy();
    for &index in stale {
        if cancel.requested() {
            return;
        }
        state.stack_previews.update(job_id, |job| {
            job.groups[index].phase = PHASE_RENDERING.into();
        });
        let fits = fits_path(&ctx.stack_root, job_id, index);
        let budget = crate::concurrency::plan_workers(
            None,
            &worker_policy,
            priority,
            crate::concurrency::probe_frame_pixels(&fits),
        );
        let lease = state.lease_workers(priority, budget.workers);
        let rendered = crate::concurrency::ComputePool::take("stack-preview-render", lease.workers)
            .map_err(|error| error.to_string())
            .and_then(|pool| {
                pool.install(|| {
                    let frame = crate::image_io::open_linear_frame(&fits)
                        .map_err(|error| error.to_string())?;
                    let clipped = (frame.image.channels == 3)
                        .then(|| read_clipped(&clipped_path(&ctx.stack_root, job_id, index)))
                        .flatten();
                    render_group_previews(
                        state,
                        ctx,
                        &ctx.stack_root,
                        job_id,
                        index,
                        &frame.image,
                        clipped.as_ref(),
                    )
                })
            });
        let revision = new_artifact_revision();
        state.stack_previews.update(job_id, |job| {
            let group = &mut job.groups[index];
            match &rendered {
                Ok(()) => {
                    group.preview_revision = Some(revision);
                    group.preview_render_failed = None;
                }
                Err(error) => {
                    tracing::warn!(job_id, group_index = index, "Stack preview render failed: {error}");
                    group.preview_render_failed = Some(stretch::SEIZA_STRETCH_OUTPUT_VERSION.into());
                    add_note(
                        &mut group.resume_note,
                        &format!("The preview could not be rendered again from the stack's FITS: {error}"),
                    );
                }
            }
            group.phase = "ready".into();
        });
    }
}

/// Complete a preview render however it ended, and point the cards at the
/// new previews. Every channel is ready: the stack was finished before the
/// render and still is, so the next Build reuses it. Color is not composed
/// again: it never waited for the render, and its inputs did not change.
pub(super) fn finish_preview_render(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    job_id: &str,
) {
    let settle = |job: &mut StackPreviewJob| {
        for group in &mut job.groups {
            if group.state == StackGroupState::Ready && group_in_flight(group) {
                group.phase = "ready".into();
            }
        }
        job.state = StackJobState::Completed;
    };
    let Some(mut job) = state.stack_previews.get(job_id) else {
        return;
    };
    settle(&mut job);
    // On disk before it reads as finished: a client that sees it finished
    // reads the latest index next, and must find the new preview there.
    if let Err(error) = persist_manifest(&ctx.stack_root, &job) {
        tracing::warn!("Failed to persist stack preview manifest: {error}");
    }
    // The artifact revision is unchanged, so no card moves and no processing
    // is applied again; the cards only take the new preview revision.
    if let Err(error) = persist_latest_and_reapply(state, ctx, &job, !job.automatic) {
        tracing::warn!("Failed to persist latest stack preview index: {error}");
    }
    state.stack_previews.update(job_id, settle);
}
