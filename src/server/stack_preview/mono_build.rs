//! A mono stack build: choosing each channel's reference, stacking its
//! groups on the worker, rendering their previews, and publishing what the
//! build made.

use super::*;

/// Whether every channel reached a state the job cannot improve on. A channel
/// that stopped short is not settled: it is work the stop took away.
pub(super) fn every_channel_settled(job: &StackPreviewJob) -> bool {
    job.groups.iter().all(|group| {
        matches!(
            group.state,
            StackGroupState::Ready | StackGroupState::Skipped | StackGroupState::Error
        )
    })
}

/// Mark whatever has not finished as cancelled. Channels that already produced
/// an artifact keep their Ready state and their preview.
pub(super) fn cancel_unfinished_groups(job: &mut StackPreviewJob) {
    for group in &mut job.groups {
        if matches!(
            group.state,
            StackGroupState::Queued | StackGroupState::Running
        ) {
            group.state = StackGroupState::Cancelled;
            group.phase = "cancelled".into();
            group.live_preview = None;
        }
    }
}

/// Which way up each group's reference frame faces, keyed by group index.
///
/// Every reference frame is read once, before any stacking, so the pier-to-sky
/// mapping learned from a solved channel is available to the channels that were
/// never solved. A frame that cannot be read contributes nothing rather than
/// failing the job; that group falls back to its own exposure.
pub(super) fn reference_anchors(
    state: &Arc<AppState>,
    database_id: &str,
    groups: &[PreparedGroup],
    peers: &[PreparedFrame],
) -> HashMap<usize, Option<(bool, &'static str)>> {
    let Some(ctx) = state.get_database(database_id) else {
        return HashMap::new();
    };
    let orient = |reference: &PreparedFrame| {
        let headers = crate::image_io::read_header(&reference.path).unwrap_or_default();
        ReferenceOrientation {
            north_up: cached_or_embedded_wcs(&ctx, reference, &headers)
                .and_then(|(wcs, _)| faces_north_up(wcs.cd)),
            west_of_pier: pier_side_from_headers(&headers).and_then(is_west_of_pier),
        }
    };
    let mut orientations = Vec::new();
    for group in groups {
        let Some(reference) = group.frames.first() else {
            continue;
        };
        if group.frames.len() < 2 {
            continue;
        }
        orientations.push((group.index, orient(reference)));
    }
    // Sibling channels stacked in their own jobs vote on the mapping too,
    // as they would have in one job.
    let calibration = calibrate_pier_side(
        &orientations
            .iter()
            .map(|(_, orientation)| *orientation)
            .chain(peers.iter().map(orient))
            .collect::<Vec<_>>(),
    );
    orientations
        .into_iter()
        .map(|(index, orientation)| (index, anchored_north_up(orientation, calibration)))
        .collect()
}

/// A frame's field rotation, from its cached plate solve or, failing that,
/// the WCS in its header. Only the reference choice reads it; registration
/// still measures every frame from its pixels.
pub(super) fn solved_orientation(
    ctx: &DatabaseContext,
    frame: &PreparedFrame,
) -> Option<reference::Orientation> {
    let (wcs, _) = cached_or_embedded_wcs(ctx, frame, &[]).or_else(|| {
        let headers = crate::image_io::read_header(&frame.path).ok()?;
        cached_or_embedded_wcs(ctx, frame, &headers)
    })?;
    crate::sequence_analysis::field_rotation_from_cd(wcs.cd)
}

/// Put Seiza's choice of reference at the front of every group that will
/// stack, taken from the largest orientation group when the frames' plate
/// solves show more than one. Scores come from the cache where they can; a
/// group where no frame scores keeps its best-graded reference and says so.
#[allow(clippy::too_many_arguments)]
pub(super) fn choose_references(
    state: &Arc<AppState>,
    database_id: &str,
    job_id: &str,
    stack_root: &FsPath,
    groups: &mut [PreparedGroup],
    order: snr::StackFrameOrder,
    rotation_margin_degrees: f64,
    worker_policy: &crate::concurrency::WorkerPolicy,
    cancel: &Stop,
) {
    let ctx = state.get_database(database_id);
    for group in groups
        .iter_mut()
        .filter(|group| group.frames.len() >= 2 && !group.mosaic)
    {
        if cancel.requested() {
            return;
        }
        let priority = job_priority(state.stack_previews.is_automatic(job_id));
        let budget = crate::concurrency::plan_workers(
            None,
            worker_policy,
            priority,
            crate::concurrency::probe_frame_pixels(&group.frames[0].path),
        );
        let lease = state.lease_workers(priority, budget.workers);
        let pool = match crate::concurrency::ComputePool::take("stack-reference", lease.workers) {
            Ok(pool) => pool,
            Err(error) => {
                tracing::warn!("Reference scoring pool: {error}");
                continue;
            }
        };
        let index = group.index;
        let Some(scores) =
            reference::scores(&group.frames, stack_root, &pool, cancel, |done, count| {
                state.stack_previews.update(job_id, |job| {
                    job.groups[index].phase = format!("Choosing a reference: frame {done}/{count}");
                });
            })
        else {
            return;
        };
        let oriented = ctx.as_deref().and_then(|ctx| {
            let orientations = group
                .frames
                .iter()
                .map(|frame| solved_orientation(ctx, frame))
                .collect::<Vec<_>>();
            let solved = orientations.iter().flatten().count();
            reference::orientation_candidates(&orientations, rotation_margin_degrees)
                .map(|(candidates, kept)| (candidates, kept, solved))
        });
        let chosen = match &oriented {
            Some((candidates, _, _)) => reference::choose_among(&scores, candidates),
            None => reference::choose(&scores),
        };
        match chosen {
            Some(chosen) => {
                let image_id = group.frames[chosen].image_id;
                let narrowed = oriented
                    .filter(|(candidates, _, _)| candidates[chosen])
                    .map(|(_, kept, solved)| {
                        format!(
                            "The reference comes from the largest orientation group, which keeps {kept} of {solved} plate-solved frames within the rotation limit"
                        )
                    });
                reference::reorder(&mut group.frames, chosen, order);
                state.stack_previews.update(job_id, |job| {
                    job.groups[index].reference_image_id = Some(image_id);
                    if let Some(narrowed) = &narrowed {
                        add_note(&mut job.groups[index].resume_note, narrowed);
                    }
                });
            }
            None => state.stack_previews.update(job_id, |job| {
                add_note(
                    &mut job.groups[index].resume_note,
                    "No frame could be scored as a reference; the best-graded frame is the reference",
                );
            }),
        }
    }
}

/// Add a sentence to a card's note without losing what is already there.
pub(super) fn add_note(note: &mut Option<String>, sentence: &str) {
    *note = Some(match note.take() {
        Some(previous) if previous.contains(sentence) => previous,
        Some(previous) => format!("{previous}. {sentence}"),
        None => sentence.to_string(),
    });
}

/// The core share a build may use: a person's build takes the interactive
/// share, and an automatic refresh the background share, like the other
/// work nobody is waiting on.
pub(super) fn job_priority(automatic: bool) -> crate::concurrency::Priority {
    if automatic {
        crate::concurrency::Priority::Background
    } else {
        crate::concurrency::Priority::Interactive
    }
}

/// Stack each channel of a prepared build, recording how each one went.
pub(super) fn execute_build(prepared: PreparedJob, worker: &Worker<'_>) -> BuildOutput {
    let state = worker.server();
    let cancel = worker.stop_flag();
    let job_id = prepared.public.job_id.clone();
    let database_id = prepared.public.database_id.clone();
    let accepted_only = prepared.public.accepted_only;
    let scoring = prepared.public.scoring;
    // Every job this process prepares records its method.
    let method = prepared.public.method.unwrap_or_default();
    // Read as the build starts: a change made while the build is queued
    // applies to it, and a change made while it runs applies to the next
    // build.
    let gpu = super::gpu::current();
    let PreparedJob {
        public: _,
        mut groups,
        orientation_peers,
        stack_root,
        calibration_root,
        north_up,
        order,
    } = prepared;
    state.stack_previews.update(&job_id, |job| {
        job.state = StackJobState::Running;
    });
    let worker_policy = state.worker_policy();
    let group_job = GroupJob {
        database_id: &database_id,
        job_id: &job_id,
        stack_root: &stack_root,
        calibration_root: &calibration_root,
        north_up,
        accepted_only,
        scoring,
        method,
        gpu,
        order,
        worker_policy: &worker_policy,
        cancel,
    };
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // The reference decides the anchors below, so it is chosen first. A
        // stop while scoring falls through to the loop, which sees it.
        if method.reference == StackReference::Auto {
            choose_references(
                state,
                &database_id,
                &job_id,
                &stack_root,
                &mut groups,
                order,
                method.reference_rotation_margin_degrees,
                &worker_policy,
                cancel,
            );
        }
        // Read every reference frame's orientation before the first stack is
        // built. A channel that was never solved has to borrow the pier-to-sky
        // mapping from one that was, and it cannot do that from a decision taken
        // after its own. Headers only, so this costs no pixel reads.
        let anchors = reference_anchors(state, &database_id, &groups, &orientation_peers);
        for group in groups {
            if group.frames.len() < 2 {
                continue;
            }
            if cancel.requested() {
                return;
            }
            state.stack_previews.update(&job_id, |job| {
                job.groups[group.index].state = StackGroupState::Running;
                job.groups[group.index].phase = "calibration".into();
            });
            let anchor = anchors.get(&group.index).copied().flatten();
            let result = run_group(state, &group_job, group.clone(), anchor);
            state.stack_previews.update(&job_id, |job| {
                // However the channel ended, it is no longer building masters.
                job.groups[group.index].calibration_progress = None;
                job.groups[group.index].live_preview = None;
            });
            state.stack_previews.update(&job_id, |job| match result {
                Ok(GroupOutcome::Built) => {
                    job.groups[group.index].state = StackGroupState::Ready;
                    job.groups[group.index].phase = "ready".into();
                }
                Ok(GroupOutcome::Cancelled) => {
                    job.groups[group.index].state = StackGroupState::Cancelled;
                    job.groups[group.index].phase = "cancelled".into();
                }
                Err(error) => {
                    job.groups[group.index].state = StackGroupState::Error;
                    job.groups[group.index].phase = "error".into();
                    job.groups[group.index].error = Some(error);
                }
            });
        }
    }));
    let cancelled = cancel.requested();
    state.stack_previews.update(&job_id, |job| match run {
        // A stop that lands after the last channel finished took nothing away,
        // so the job completed. Calling it cancelled would also stop the next
        // build from reusing work that is sitting there finished.
        Ok(()) if cancelled && !every_channel_settled(job) => {
            cancel_unfinished_groups(job);
            job.state = StackJobState::Cancelled;
        }
        Ok(()) => job.state = StackJobState::Completed,
        Err(_) => {
            job.state = StackJobState::Failed;
            job.error = Some("Stack worker panicked".into());
        }
    });
    BuildOutput {
        job_id,
        database_id,
        stack_root,
    }
}

/// Take a finished build in: record it, point the cards at what it built,
/// and start what follows it.
pub(super) fn publish_build(state: &Arc<AppState>, output: BuildOutput) {
    let BuildOutput {
        job_id,
        database_id,
        stack_root,
    } = output;
    if let Some(job) = state.stack_previews.get(&job_id)
        && let Err(error) = persist_manifest(&stack_root, &job)
    {
        tracing::warn!("Failed to persist stack preview manifest: {error}");
    }
    // A cancelled job still remembers the channels it finished before the
    // stop: those artifacts are complete and are what the user asked for.
    // An automatic build that was stopped for a user's build keeps its
    // artifacts for its own next run but leaves the cards alone: a color
    // composition the user queued against the current cards must still
    // find them current when its turn comes.
    if let Some(job) = state.stack_previews.get(&job_id)
        && (job.state == StackJobState::Completed
            || (job.state == StackJobState::Cancelled && !job.automatic))
        && let Some(ctx) = state.get_database(&database_id)
        // A stopped build's finished channels are not the person's to
        // process now: Stop all must not start RC-Astro.
        && let Err(error) = persist_latest_and_reapply(
            state,
            &ctx,
            &job,
            job.state == StackJobState::Completed && !job.automatic,
        )
    {
        tracing::warn!("Failed to persist latest stack preview index: {error}");
    }
    // Color follows the channels: once no build is left for a target, its
    // color previews are composed again from the newest channel stacks.
    // A failed last channel still lets the channels before it reach their
    // color previews. A stopped one does not: Stop all must not start color
    // work, and the next build or a cache hit composes what is due.
    if let Some(job) = state.stack_previews.get(&job_id)
        && matches!(job.state, StackJobState::Completed | StackJobState::Failed)
        && let Some(ctx) = state.get_database(&database_id)
    {
        automatic::compose_colors_after(state, &ctx, &job);
    }
    state.stack_previews.prune_cache(&stack_root);
}

/// Whether a channel produced an artifact or stopped on request. A failure is
/// the error arm instead: a cancel is not a fault to report.
pub(super) enum GroupOutcome {
    Built,
    Cancelled,
}

/// Everything a group build needs from the job that owns it: where it writes,
/// how the caller asked for it, and how it is told to stop.
pub(super) struct GroupJob<'a> {
    database_id: &'a str,
    job_id: &'a str,
    stack_root: &'a FsPath,
    calibration_root: &'a FsPath,
    north_up: bool,
    accepted_only: bool,
    scoring: StackScoringSettings,
    method: StackMethod,
    /// Which passes run on the GPU, for every channel of the build.
    gpu: super::StackGpu,
    order: snr::StackFrameOrder,
    worker_policy: &'a crate::concurrency::WorkerPolicy,
    cancel: &'a Stop,
}

pub(super) fn run_group(
    state: &Arc<AppState>,
    job: &GroupJob<'_>,
    group: PreparedGroup,
    anchor: Option<(bool, &'static str)>,
) -> Result<GroupOutcome, String> {
    use seiza_stacking::{FrameDisposition, LiveStacker};

    let &GroupJob {
        database_id,
        job_id,
        stack_root,
        calibration_root,
        north_up,
        accepted_only,
        scoring,
        method,
        gpu,
        order,
        worker_policy,
        cancel,
    } = job;
    // Read for each channel: a build adopted part way takes the person's
    // share from its next channel on.
    let priority = job_priority(state.stack_previews.is_automatic(job_id));
    let weighting = method.weighting;
    let available_memory = crate::concurrency::available_memory_bytes();
    let reference_path = &group
        .frames
        .first()
        .ok_or_else(|| "Stack group has no input frames".to_string())?
        .path;
    let budget = crate::concurrency::plan_workers(
        None,
        worker_policy,
        priority,
        crate::concurrency::probe_frame_pixels(reference_path),
    );
    // Held until the group is built: its workers come out of the budget the
    // other jobs of this priority share.
    let lease = state.lease_workers(priority, budget.workers);
    let threads = execution::ThreadBudget::from_total(lease.workers);
    let pool = crate::concurrency::ComputePool::take("stack-preview", lease.workers)
        .map_err(|error| error.to_string())?;
    tracing::info!(
        job_id,
        group_index = group.index,
        total_worker_budget = budget.workers,
        phase_workers = lease.workers,
        compute_workers = threads.compute_workers,
        preparation_workers = threads.preparation_workers,
        serial = threads.serial,
        rationale = %budget.rationale,
        shared = %lease.summary(),
        "Stack worker budget configured"
    );
    log_excluded_frames(state, job_id, group.index);
    let ctx = state
        .get_database(database_id)
        .ok_or_else(|| format!("Database {database_id} is no longer configured"))?;
    // Through the shared opener, so this connection gets the same busy
    // timeout as every other and the same in-place upgrade of PSF Guard's
    // tables. Opening it raw is how a stack came to read a catalog whose
    // calibration tables predated a column the query names.
    let calibration_conn =
        crate::server::database_context::open_scheduler_connection(&ctx.database_path)
            .map_err(|error| format!("Opening calibration catalog: {error}"))?;
    let directory_tree = ctx
        .get_directory_tree()
        .map_err(|error| format!("Indexing calibration folders: {error}"))?;
    let light_paths = group
        .frames
        .iter()
        .map(|frame| frame.path.clone())
        .collect::<Vec<_>>();
    // Building a night of masters is minutes of work, so the stop flag reaches
    // between them too. The plan partitions a multi-night group into
    // sessions, each with its own masters; a single-night group gets one
    // session and stacks exactly as before.
    let calibration_mode = group.calibration;
    let flat_star_masking = group.flat_star_masking;
    // Each frame a master build reads moves the card, and a stop reaches
    // into the build instead of waiting for it.
    let watch = {
        let state = Arc::clone(state);
        let job_id = job_id.to_string();
        let index = group.index;
        crate::calibration::MasterBuildWatch {
            report: Some(Arc::new(move |progress| {
                state.stack_previews.update(&job_id, |job| {
                    job.groups[index].calibration_progress = Some(progress);
                });
            })),
            stop: Some(cancel.clone()),
            ..Default::default()
        }
    };
    let calibration_started = std::time::Instant::now();
    let plan = pool.install(move || {
        crate::calibration::resolve_or_build_master_plan_with_options(
            &calibration_conn,
            calibration_root,
            &light_paths,
            Some(&directory_tree),
            Some(cancel.as_flag()),
            crate::calibration::CalibrationPlanOptions {
                mode: calibration_mode,
                pinned: &[],
                flat_star_masking,
                watch,
            },
        )
    });
    tracing::info!(
        job_id,
        group_index = group.index,
        elapsed_seconds = calibration_started.elapsed().as_secs_f64(),
        "Stack calibration plan finished"
    );
    if cancel.requested() {
        return Ok(GroupOutcome::Cancelled);
    }
    // `{:#}` keeps the whole anyhow chain: "building master flat: <why>".
    // `to_string()` printed only the outermost context, which reported a
    // failed master build with no cause at all.
    //
    // Auto mode's contract reaches here too: a plan that cannot be built —
    // an unreadable cached master, a selection query refused — is a reason
    // to stack raw and say why, never to abandon the group. The cancel
    // check above already returned, so this cannot swallow a stop. Forced
    // calibration keeps the hard error.
    let plan = match plan {
        Ok(plan) => plan,
        Err(error) if group.calibration != crate::calibration::CalibrationMode::On => {
            tracing::warn!("calibration plan failed; stacking raw: {error:#}");
            let mut plan =
                crate::calibration::CalibrationPlan::without_calibration(group.frames.len());
            plan.applied.warning = Some(format!(
                "Stacked without calibration: the calibration plan could not be built — \
                 {error:#}"
            ));
            plan
        }
        Err(error) => {
            tracing::warn!("Stack group calibration failed: {error:#}");
            return Err(format!("{error:#}"));
        }
    };
    let mut applied_calibration = plan.applied.clone();
    // A session without a dark master keeps its lights' hot pixels, and a
    // handful of frames cannot reject them statistically, so the spatial
    // impulse filter runs over that session's frames. The live stacker
    // fixes its filter at construction, so the online pass filters only
    // when no session has a dark; the final integration, which produces
    // the stack, decides per session. Say so on the card; display only,
    // after the identity is computed.
    let session_cosmetics = SessionCosmetics::for_plan(&plan, group.calibration);
    let cosmetic = session_cosmetics.whole_group();
    if let Some(note) = session_cosmetics.card_note() {
        applied_calibration.warning = Some(match applied_calibration.warning.take() {
            Some(previous) => format!("{previous}. {note}"),
            None => note,
        });
    }
    // The resume checkpoint key carries the applied-master signature too:
    // the selection fingerprint is computed before any build, so after a
    // transient build failure a later run with the same selection could
    // otherwise resume an accumulator whose earlier frames integrated
    // under different masters — one stack mixing two calibrations.
    let calibration_fingerprint = format!(
        "{}|{}",
        applied_calibration.fingerprint, applied_calibration.masters_signature
    );
    state.stack_previews.update(job_id, |job| {
        let status = &mut job.groups[group.index];
        status.calibration = applied_calibration;
        status.calibration_progress = None;
        status.phase = "stacking".into();
    });
    let (group_target_id, group_filter_name, group_exposure_key) = state
        .stack_previews
        .get(job_id)
        .map(|job| {
            let status = &job.groups[group.index];
            (
                status.target_id,
                status.filter_name.clone(),
                exposure_group_key(status).map(str::to_owned),
            )
        })
        .ok_or_else(|| "Stack job disappeared while running".to_string())?;
    let requested = group
        .frames
        .iter()
        .map(|frame| {
            (
                frame.image_id,
                frame.source_fingerprint.as_str(),
                frame.exposure_seconds,
            )
        })
        .collect::<Vec<_>>();
    let decision = resume::load(
        stack_root,
        database_id,
        group_target_id,
        &group_filter_name,
        group_exposure_key.as_deref(),
        accepted_only,
        scoring,
        method,
        SEIZA_STACKING_OUTPUT_VERSION,
        &calibration_fingerprint,
        order,
        &requested,
    );
    if let Some(reason) = decision.fresh_reason() {
        tracing::info!(
            target_id = group_target_id,
            filter = %group_filter_name,
            "Full restack: {reason}"
        );
        resume::discard_unusable(
            &decision,
            order,
            stack_root,
            database_id,
            group_target_id,
            &group_filter_name,
            group_exposure_key.as_deref(),
        );
        state.stack_previews.update(job_id, |job| {
            add_note(
                &mut job.groups[group.index].resume_note,
                &format!("Full restack: {reason}"),
            );
        });
    }
    let checkpoint = (!group.mosaic).then(|| decision.state()).flatten();
    let reference_frame = crate::image_io::open_linear_frame(&group.frames[0].path)
        .map_err(|error| error.to_string())?;
    let reference_is_bayer = reference_frame.bayer.is_some();
    let output_channels = if reference_is_bayer {
        3_u64
    } else {
        reference_frame.image.channels as u64
    };
    let pixels = reference_frame.image.pixel_count();
    let mut pipeline_budget = execution::plan_pipeline(
        available_memory,
        worker_policy,
        pixels,
        pixels.saturating_mul(output_channels as usize),
        &plan,
        &threads,
    )?;
    let mut threads = pipeline_budget.threads;
    tracing::info!(
        job_id,
        group_index = group.index,
        compute_workers = threads.compute_workers,
        preparation_workers = threads.preparation_workers,
        serial = threads.serial,
        memory = %pipeline_budget.summary(),
        "Stack pipeline memory planned"
    );

    let initialized = pool.install(|| {
        // A checkpoint that fails to reopen — a Seiza version change, a
        // truncated file — is discarded and the group builds from scratch.
        // A reopened stack runs its live passes under this build's GPU
        // setting, not the one it was saved under.
        let restored = checkpoint.and_then(|checkpoint| {
            match LiveStacker::open_context_with_gpu(&checkpoint.context_path, gpu.options()) {
                Ok(stacker)
                    if resume::context_matches_manifest(
                        stacker.view().accepted_frames,
                        stacker.view().rejected_frames,
                        &checkpoint.manifest,
                    ) =>
                {
                    Some((stacker, checkpoint.manifest))
                }
                Ok(_) => {
                    tracing::warn!(
                        "Stack checkpoint context and manifest did not match; rebuilding from scratch"
                    );
                    resume::discard(stack_root, database_id, group_target_id, &group_filter_name, group_exposure_key.as_deref());
                    state.stack_previews.update(job_id, |job| {
                        job.groups[group.index].resume_note =
                            Some("Full restack: the checkpoint files did not match".into());
                    });
                    None
                }
                Err(error) => {
                    tracing::warn!(
                        "Stack checkpoint could not be reopened ({error}); rebuilding from scratch"
                    );
                    resume::discard(stack_root, database_id, group_target_id, &group_filter_name, group_exposure_key.as_deref());
                    state.stack_previews.update(job_id, |job| {
                        job.groups[group.index].resume_note =
                            Some("Full restack: the checkpoint could not be reopened".into());
                    });
                    None
                }
            }
        });
        let mut orientation_vote = OrientationVote::default();
        let (stacker, ledger, points) = if group.mosaic {
            state.stack_previews.update(job_id, |job| {
                let status = &mut job.groups[group.index];
                status.phase = "planning mosaic".into();
                status.reference_image_id = None;
                status.resume_note = Some("Mosaic canvases are replanned when their inputs change".into());
            });
            let limit = mosaic::maximum_samples(available_memory, worker_policy, &plan)?;
            let stacker = mosaic::plan(&group, &plan,
                method.stack_options(cosmetic, gpu), &stack_dir(stack_root, job_id), limit, cancel,
                |count, width, height| {
                    state.stack_previews.update(job_id, |job| {
                        job.groups[group.index].phase = format!("planning mosaic: {count} references, {width} × {height}");
                    });
                })?;
            state.stack_previews.update(job_id, |job| {
                let status = &mut job.groups[group.index];
                status.output_channels = output_channels as usize;
                for note in stacker.expansion_planning_notes() {
                    add_note(&mut status.resume_note, note);
                }
            });
            (stacker, Vec::new(), Vec::new())
        } else { match restored {
            Some((stacker, manifest)) => {
                // Replay the checkpointed ledger: the per-frame record, the
                // counters, and each accepted frame's orientation vote. The
                // reference is the ledger's first entry and votes upright.
                let points = manifest.snr_points;
                let ledger = manifest.frames;
                for frame in &ledger {
                    if let Some(rotation) = frame.rotation_radians {
                        orientation_vote.add(rotation, frame.exposure_seconds);
                    }
                }
                tracing::info!(
                    "Stack preview {} group {}: resuming from a checkpoint of {} frame(s)",
                    job_id,
                    group.index,
                    ledger.len()
                );
                state.stack_previews.update(job_id, |job| {
                    let status = &mut job.groups[group.index];
                    status.processed_frames = ledger.len();
                    status.accepted_frames = ledger
                        .iter()
                        .filter(|frame| {
                            matches!(
                                frame.decision.disposition.as_str(),
                                "reference" | "accepted"
                            )
                        })
                        .count();
                    status.rejected_frames = ledger.len() - status.accepted_frames;
                    status.reused_frames = ledger.len();
                    status.output_channels = output_channels as usize;
                    status.total_exposure_seconds = ledger
                        .iter()
                        .filter(|frame| frame.rotation_radians.is_some())
                        .map(|frame| frame.exposure_seconds)
                        .sum();
                    status.frames = ledger.iter().map(|frame| frame.decision.clone()).collect();
                    let exposures = accepted_exposures(&ledger);
                    status.snr = (!points.is_empty()).then(|| {
                        snr::ProgressiveSnr::new(order, points.clone(), &exposures)
                    });
                });
                (stacker, ledger, points)
            }
            None => {
                // From the record, like every other frame's. Reading this one
                // from its opened header instead would weigh it in seconds
                // against frames weighing 1.0 apiece whenever the catalog does
                // not record an exposure, and the reference alone would then
                // outvote the whole night.
                let reference_exposure = group.frames[0].exposure_seconds;
                // The reference frame defines zero rotation; it votes upright.
                orientation_vote.add(0.0, reference_exposure);
                // With no dark master anywhere in the plan, nothing
                // subtracts the lights' hot pixels, and a handful of frames
                // cannot reject them statistically. The spatial impulse
                // filter is the remaining defense; when any session has a
                // dark, the dark does the job with real measurements.
                let mut options = method.stack_options(cosmetic, gpu);
                // The reference is judged against its session's masters
                // under that session's gate, its rig's tolerances.
                options.match_tolerances = plan.sessions[plan.assignments[0]].match_tolerances;
                // The reference calibrates with its own session's masters;
                // later sessions swap theirs in per batch.
                //
                // Creation validates the reference against those masters, so
                // this is a place a calibration refusal can escape — it did
                // once, killing two filters at frame zero over cached masters
                // that predate optics metadata. Auto mode's contract instead:
                // degrade to a raw stack, and say so and why. Forced
                // calibration keeps the hard error.
                let reference_masters = plan.sessions[plan.assignments[0]].masters.clone();
                let (stacker, calibration_bypassed) = match LiveStacker::new(reference_frame, reference_masters, options.clone()) {
                    Ok(stacker) => (stacker, false),
                    Err(error)
                        if group.calibration != crate::calibration::CalibrationMode::On =>
                    {
                        tracing::warn!(
                            "reference refused its session masters; stacking raw: {error}"
                        );
                        let note = format!(
                            "Stacked without calibration: the reference frame's masters were \
                             refused — {error}"
                        );
                        state.stack_previews.update(job_id, |job| {
                            let calibration = &mut job.groups[group.index].calibration;
                            calibration.warning = Some(match calibration.warning.take() {
                                Some(previous) if previous.contains(&note) => previous,
                                Some(previous) => format!("{previous}. {note}"),
                                None => note,
                            });
                        });
                        let reference_frame =
                            crate::image_io::open_linear_frame(&group.frames[0].path)
                                .map_err(|error| error.to_string())?;
                        (LiveStacker::new(
                            reference_frame,
                            seiza_stacking::CalibrationMasters::default(),
                            options,
                        )
                        .map_err(|error| error.to_string())?, true)
                    }
                    Err(error) => return Err(error.to_string()),
                };
                let reference_mapping = stacker.reference_mapping();
                let reference_decision = StackFrameDecision {
                    image_id: group.frames[0].image_id,
                    disposition: "reference".into(),
                    reason: None,
                    quality_score: group.frames[0].quality_score,
                    matched_stars: None,
                    registration_rms_pixels: None,
                    registration_drift_pixels: None,
                    registered_mapping: Some(reference_mapping),
                    normalization_mean_gain: Some(1.0),
                    normalization_mean_offset: Some(0.0),
                    source_fingerprint: Some(group.frames[0].source_fingerprint.clone()),
                    overlap_fraction: Some(1.0),
                    integrated_fraction: Some(1.0),
                    // The reference defines the noise every other weight is
                    // read against, so it weighs 1 in every channel.
                    noise_sigma: (!stacker.reference_noise().is_empty())
                        .then(|| stacker.reference_noise().to_vec()),
                    integration_weight: (!weighting.is_equal()).then(|| {
                        let channels = match stacker.reference_noise().len() {
                            0 => output_channels as usize,
                            measured => measured,
                        };
                        vec![1.0; channels]
                    }),
                };
                state.stack_previews.update(job_id, |job| {
                    let status = &mut job.groups[group.index];
                    status.processed_frames = 1;
                    status.accepted_frames = 1;
                    status.output_channels = output_channels as usize;
                    status.total_exposure_seconds = reference_exposure;
                    status.frames.push(reference_decision.clone());
                });
                let ledger = vec![resume::ResumeFrame {
                    decision: reference_decision,
                    exposure_seconds: reference_exposure,
                    calibration_bypassed,
                    retryable_failure: false,
                    rotation_radians: Some(0.0),
                }];
                (stacker, ledger, Vec::new())
            }
        }};
        Ok::<_, String>((stacker, ledger, points, orientation_vote))
    });
    if cancel.requested() {
        return Ok(GroupOutcome::Cancelled);
    }
    let (mut stacker, mut ledger, mut points, mut orientation_vote) = initialized?;
    if group.mosaic {
        let view = stacker.view();
        pipeline_budget = execution::plan_pipeline(
            available_memory,
            worker_policy,
            view.width * view.height,
            view.mean.len(),
            &plan,
            &threads,
        )?;
        threads = pipeline_budget.threads;
    }
    // The reference frame alone is the first depth on the curve and the
    // baseline every later depth is read against: one frame's noise.
    if points.is_empty()
        && let Some(sample) = pool.install(|| seiza_stacking::measure_depth(stacker.view()))
    {
        points.push(snr::point(sample, integrated_exposure(&ledger)));
    }
    let save_checkpoint =
        |stacker: &LiveStacker, ledger: &[resume::ResumeFrame], points: &[snr::SnrPoint]| {
            pool.install(|| {
                // A quality-ordered build is a full restack by nature. Writing its
                // accumulator here would leave a checkpoint no later build can
                // extend, in place of the capture-order one that can be.
                if group.mosaic || !order.resumable() {
                    return;
                }
                if ledger.iter().any(|frame| frame.retryable_failure) {
                    resume::discard(
                        stack_root,
                        database_id,
                        group_target_id,
                        &group_filter_name,
                        group_exposure_key.as_deref(),
                    );
                    return;
                }
                let context_path = resume::context_path(
                    stack_root,
                    database_id,
                    group_target_id,
                    &group_filter_name,
                    group_exposure_key.as_deref(),
                );
                let manifest = resume::ResumeManifest {
                    schema_version: resume::RESUME_SCHEMA_VERSION,
                    stacking_version: SEIZA_STACKING_OUTPUT_VERSION.into(),
                    target_id: group_target_id,
                    filter_name: group_filter_name.clone(),
                    exposure_group_key: group_exposure_key.clone(),
                    accepted_only,
                    scoring,
                    method,
                    calibration_fingerprint: calibration_fingerprint.clone(),
                    order,
                    snr_points: points.to_vec(),
                    frames: ledger.to_vec(),
                };
                let saved = context_path
                    .parent()
                    .ok_or_else(|| "checkpoint path has no parent".to_string())
                    .and_then(|parent| {
                        std::fs::create_dir_all(parent).map_err(|error| error.to_string())
                    })
                    .and_then(|()| {
                        stacker
                            .save_context(&context_path)
                            .map_err(|error| error.to_string())
                    })
                    .and_then(|()| {
                        resume::store_manifest(
                            &resume::manifest_path(
                                stack_root,
                                database_id,
                                group_target_id,
                                &group_filter_name,
                                group_exposure_key.as_deref(),
                            ),
                            &manifest,
                        )
                    });
                if let Err(error) = saved {
                    // A checkpoint is an optimization; a build never fails over
                    // it. Discard the pair so nothing resumes from half a save.
                    tracing::warn!("Failed to save stack checkpoint: {error}");
                    resume::discard(
                        stack_root,
                        database_id,
                        group_target_id,
                        &group_filter_name,
                        group_exposure_key.as_deref(),
                    );
                }
            })
        };

    let mut live_preview = live::Publisher::new(state, stack_root, job_id, group.index, &pool);
    if !cancel.requested() && stacker.view().accepted_frames > 0 {
        live_preview.publish(&stacker, stacker.view().accepted_frames, false);
    }

    // Pending frames keep their index into `group.frames`, which is what
    // the calibration plan's session assignments are aligned with.
    let pending: Vec<(usize, &PreparedFrame)> =
        group.frames.iter().enumerate().skip(ledger.len()).collect();
    // Reads, calibration, registration and normalization overlap across
    // frames while integration stays in this order, so the accumulator
    // sees exactly the sequence a frame-at-a-time loop would. A frame
    // declaring itself normalized is put on the same 16-bit scale the
    // rest of the catalog uses as it is read.
    let pipeline = pipeline_budget.options;
    let mut cancelled = false;
    // Depths already behind us were measured by the build that wrote the
    // checkpoint, so only the ones ahead split this run's batches.
    let start_depth = ledger.len();
    let mut checkpoints: std::collections::VecDeque<usize> =
        seiza_stacking::checkpoint_depths(start_depth + pending.len())
            .into_iter()
            .filter(|depth| *depth > start_depth)
            .collect();
    // Consecutive frames of one calibration session push as one
    // pipelined batch, with the session's masters swapped in first. The
    // frames after the reference are chronological, so a night is one
    // batch and a single-session group is exactly one call. On a
    // resumed stack this also replaces whatever masters the checkpoint
    // stored with the ones this batch needs.
    // Frames the stacker turned away for calibration, folded into the
    // group warning below: each carries its reason in the frame list,
    // but nobody reads a hundred rows to learn that six frames shared
    // one cause.
    let mut calibration_rejections: Vec<String> = Vec::new();
    // Whose masters the stacker holds, and whether that session stacks raw.
    // A fresh stack holds the reference's; a resumed one holds whatever its
    // checkpoint stored, so its first batch swaps. A batch also ends at each
    // depth the curve is measured at, and swapping in the masters already
    // held would only validate them again and add a set to Seiza's ledger.
    let mut active_masters: Option<(usize, bool)> =
        (start_depth == 1).then(|| (plan.assignments[0], ledger[0].calibration_bypassed));
    let mut batch_start = 0usize;
    while batch_start < pending.len() && !cancelled {
        let session = plan.assignments[pending[batch_start].0];
        let mut batch_end = pending[batch_start..]
            .iter()
            .position(|(index, _)| plan.assignments[*index] != session)
            .map(|offset| batch_start + offset)
            .unwrap_or(pending.len());
        // A batch also ends at the next depth the curve is measured at.
        // The accumulator can only be read between batches, and the
        // doubling ladder keeps this to about one extra boundary per
        // doubling — nine of them across five hundred frames.
        if let Some(&next) = checkpoints.front() {
            let limit = next.saturating_sub(start_depth);
            if limit > batch_start {
                batch_end = batch_end.min(limit);
            }
        }
        batch_end = batch_end.min(batch_start.saturating_add(live_preview.batch_frames()));
        let batch = &pending[batch_start..batch_end];
        let paths: Vec<PathBuf> = batch.iter().map(|(_, frame)| frame.path.clone()).collect();
        // Auto mode's contract: a calibration problem is something to
        // warn about and work around, never a reason to abandon a stack
        // half-integrated. A swap the stacker refuses falls back to
        // stacking this session's frames raw, and the group warning says
        // so and why. Forced calibration keeps the hard error: the user
        // explicitly asked for these masters.
        let mut calibration_bypassed = false;
        if let Some((active, bypassed)) = active_masters
            && active == session
        {
            calibration_bypassed = bypassed;
        } else if let Err(error) =
            pool.install(|| use_session(&mut stacker, &plan.sessions[session]))
        {
            if group.calibration == crate::calibration::CalibrationMode::On {
                return Err(error.to_string());
            }
            tracing::warn!(
                "session {session} masters refused; stacking its frames uncalibrated: {error}"
            );
            pool.install(|| stacker.set_calibration(seiza_stacking::CalibrationMasters::default()))
                .map_err(|error| error.to_string())?;
            calibration_bypassed = true;
            let note = format!(
                "Session {} stacked uncalibrated: its masters were refused — {error}",
                session + 1
            );
            state.stack_previews.update(job_id, |job| {
                let calibration = &mut job.groups[group.index].calibration;
                calibration.warning = Some(match calibration.warning.take() {
                    Some(previous) if previous.contains(&note) => previous,
                    Some(previous) => format!("{previous}. {note}"),
                    None => note,
                });
            });
        }
        active_masters = Some((session, calibration_bypassed));
        let mut consumed = 0usize;
        // Seiza prepares frames as tasks in the build's pool and commits
        // outcomes in source order.
        let batch_started = std::time::Instant::now();
        let report = execution::run_pipeline(
            &mut stacker,
            &paths,
            &pipeline,
            &pool,
            &threads,
            |_, outcome| {
                let (_, frame) = batch[consumed];
                consumed += 1;
                let exposure = frame.exposure_seconds;
                let (decision, retryable_failure) = match outcome {
                    Ok(FrameDisposition::Accepted(diagnostics)) => {
                        orientation_vote
                            .add(diagnostics.mapping.transform().rotation_radians, exposure);
                        (
                            StackFrameDecision {
                                image_id: frame.image_id,
                                disposition: "accepted".into(),
                                reason: None,
                                quality_score: frame.quality_score,
                                matched_stars: Some(diagnostics.matched_stars),
                                registration_rms_pixels: Some(diagnostics.registration_rms_pixels),
                                registration_drift_pixels: Some(
                                    diagnostics.registration_drift_pixels,
                                ),
                                registered_mapping: Some(*diagnostics.mapping),
                                normalization_mean_gain: Some(diagnostics.normalization_mean_gain),
                                normalization_mean_offset: Some(
                                    diagnostics.normalization_mean_offset,
                                ),
                                source_fingerprint: Some(frame.source_fingerprint.clone()),
                                overlap_fraction: Some(diagnostics.overlap_fraction),
                                integrated_fraction: Some(diagnostics.integrated_fraction),
                                noise_sigma: (!diagnostics.noise.is_empty())
                                    .then(|| diagnostics.noise.clone()),
                                integration_weight: (!diagnostics.weight.is_empty())
                                    .then(|| diagnostics.weight.clone()),
                            },
                            false,
                        )
                    }
                    // A frame the stack turned away and one that could not be
                    // read are both "not integrated" to a caller reading the
                    // group's decisions; only the reason differs.
                    Ok(FrameDisposition::Rejected(reason)) => {
                        if let seiza_stacking::FrameRejectionReason::Calibration(message) = &reason
                        {
                            calibration_rejections.push(message.clone());
                        }
                        tracing::info!(
                            job_id,
                            group_index = group.index,
                            image_id = frame.image_id,
                            file = %frame.path.display(),
                            "Stack rejected a frame: {reason}"
                        );
                        (rejected_decision(frame, reason.to_string()), false)
                    }
                    Err(error) => {
                        tracing::warn!(
                            job_id,
                            group_index = group.index,
                            image_id = frame.image_id,
                            file = %frame.path.display(),
                            "Stack could not read a frame: {error}"
                        );
                        (rejected_decision(frame, error.to_string()), true)
                    }
                };
                ledger.push(resume::ResumeFrame {
                    decision: decision.clone(),
                    exposure_seconds: exposure,
                    calibration_bypassed,
                    retryable_failure,
                    rotation_radians: if decision.disposition == "accepted" {
                        decision
                            .registered_mapping
                            .as_ref()
                            .map(|mapping| mapping.transform().rotation_radians)
                    } else {
                        None
                    },
                });
                state.stack_previews.update(job_id, |job| {
                    let status = &mut job.groups[group.index];
                    status.processed_frames += 1;
                    if matches!(decision.disposition.as_str(), "accepted") {
                        status.accepted_frames += 1;
                        status.total_exposure_seconds += exposure;
                    } else {
                        status.rejected_frames += 1;
                    }
                    status.frames.push(decision);
                });

                // Integrating one frame is the unit of work, so this is where
                // a stop takes effect. Frames already prepared are discarded.
                if cancel.requested() {
                    cancelled = true;
                    seiza_stacking::Continue::No
                } else {
                    seiza_stacking::Continue::Yes
                }
            },
        )
        .map_err(|error| error.to_string())?;
        tracing::info!(
            job_id,
            group_index = group.index,
            session,
            batch_frames = paths.len(),
            elapsed_seconds = batch_started.elapsed().as_secs_f64(),
            integrated = report.frames.integrated,
            rejected = report.frames.rejected,
            failed = report.frames.failed,
            preparation_workers = report.workers,
            execution = ?report.execution,
            "Stack preparation batch finished"
        );
        live_preview.observe_batch(batch_started.elapsed(), consumed);
        batch_start = batch_end;

        // Read the accumulator whenever this batch carried it past a
        // depth on the ladder, and once more wherever a stop landed, so a
        // stopped build still publishes the curve it paid for.
        let depth = start_depth + batch_start;
        let crossed = checkpoints.front().is_some_and(|next| *next <= depth);
        while checkpoints.front().is_some_and(|next| *next <= depth) {
            checkpoints.pop_front();
        }
        // Frames the stack turned away move the depth without moving the
        // accepted count, and a step that integrated nothing is not a
        // depth on the curve.
        let accepted_frames = stacker.view().accepted_frames;
        let advanced = points
            .last()
            .is_none_or(|last| last.frames < accepted_frames);
        if (crossed || cancelled)
            && advanced
            && let Some(sample) = pool.install(|| seiza_stacking::measure_depth(stacker.view()))
        {
            points.push(snr::point(sample, integrated_exposure(&ledger)));
            let exposures = accepted_exposures(&ledger);
            let progressive = snr::ProgressiveSnr::new(order, points.clone(), &exposures);
            state.stack_previews.update(job_id, |job| {
                job.groups[group.index].snr = Some(progressive);
            });
        }
        if !cancelled && !cancel.requested() {
            live_preview.publish(&stacker, accepted_frames, false);
        }
    }

    if cancelled {
        // The frames that did land are checkpointed, so building again
        // continues from them.
        save_checkpoint(&stacker, &ledger, &points);
        return Ok(GroupOutcome::Cancelled);
    }
    if !cancel.requested() {
        // Keep the deepest live estimate visible throughout final rejection.
        live_preview.publish(&stacker, stacker.view().accepted_frames, true);
    }
    // The accumulator is complete: checkpoint it before the snapshot
    // consumes the stacker, so an additive rebuild can pick it up here.
    save_checkpoint(&stacker, &ledger, &points);
    // The per-frame reasons live in the frame list; the summary is what a
    // person actually reads. One line per distinct cause, with a count.
    if !calibration_rejections.is_empty() {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for reason in &calibration_rejections {
            match counts.iter_mut().find(|(existing, _)| existing == reason) {
                Some((_, count)) => *count += 1,
                None => counts.push((reason.clone(), 1)),
            }
        }
        let note = format!(
            "{} frame(s) could not be calibrated and were left out — {}",
            calibration_rejections.len(),
            counts
                .iter()
                .map(|(reason, count)| format!("{count}× {reason}"))
                .collect::<Vec<_>>()
                .join("; ")
        );
        state.stack_previews.update(job_id, |job| {
            let calibration = &mut job.groups[group.index].calibration;
            calibration.warning = Some(match calibration.warning.take() {
                Some(previous) if previous.contains(&note) => previous,
                Some(previous) => format!("{previous}. {note}"),
                None => note,
            });
        });
    }
    // Last exit before the job writes anything. Orienting and rendering
    // follow, and a stop after this point would have to clean up published
    // artifacts.
    if cancel.requested() {
        return Ok(GroupOutcome::Cancelled);
    }
    pool.install(|| {
        if group.mosaic {
            let expected = group.frames.iter().map(|f| f.target_id).collect::<HashSet<_>>();
            let joined = group.frames.iter().zip(&ledger).filter(|(_, record)| record.admitted())
                .map(|(frame, _)| frame.target_id).collect::<HashSet<_>>();
            let missing = expected.difference(&joined).copied().collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(format!("Mosaic is incomplete: no frames aligned from panel target(s) {missing:?}. Inspect the rejected frames."));
            }
        }
        let reference_headers = stacker.reference_headers().to_vec();
        let accepted_frames = stacker.view().accepted_frames;
        let rejected_frames = stacker.view().rejected_frames;
        // Where the raw frames clipped, which only the live stacker knows;
        // the final pass integrates on the same grid. Colour calibration
        // renders those pixels neutral, so a clipped star core is not
        // tinted by the channels' gains.
        let clipped = stacker.clipped_fraction();
        // Seiza reads each frame once and then runs its three passes band
        // by band: two stages. Without room for its scratch files it reads
        // every frame on each of three passes instead.
        let passes = std::cell::Cell::new(2);
        let mut report_pass = |pass: seiza_stacking::BatchStackPass, index: usize, count: usize| {
            let pass = match pass {
                seiza_stacking::BatchStackPass::Estimate => 1,
                seiza_stacking::BatchStackPass::Refine => {
                    passes.set(3);
                    2
                }
                seiza_stacking::BatchStackPass::Integrate => passes.get(),
            };
            let passes = passes.get();
            state.stack_previews.update(job_id, |job| {
                let group = &mut job.groups[group.index];
                group.phase = format!(
                    "Rejecting transients: pass {pass}/{passes}, frame {}/{count}",
                    index + 1
                );
                group.final_pass = Some(FinalPassProgress {
                    pass,
                    passes,
                    frame: index + 1,
                    frames: count,
                });
            });
        };
        // The final pass replays PSF Guard's ledger, keeping each frame's
        // background matched to the reference the live pass chose. Seiza
        // 0.19's own reintegration refits those backgrounds against the mean
        // of the best frames instead; on a 14-frame Sh2 230 night that mean
        // carried a corner gradient into the whole stack, a gradient the
        // live stack did not have. Seiza's command line does the same.
        let reintegrate = method.final_pass == StackFinalPass::Reintegrate && accepted_frames >= 3;
        let reintegrate = if reintegrate && method.bayer_drizzle && reference_is_bayer {
            // Only Seiza can fill the photosites no frame reached, and it
            // does so in the live stack.
            state.stack_previews.update(job_id, |job| {
                add_note(
                    &mut job.groups[group.index].resume_note,
                    "Final pass skipped: PSF Guard cannot yet replay a Bayer drizzle, so this is \
                     the live stack",
                );
            });
            false
        } else {
            reintegrate
        };
        let integrated = if reintegrate {
            // The final pass keeps each admitted frame in a scratch file,
            // four bytes a sample, when the stack folder's disk has room for
            // all of them and a margin; otherwise it reads every frame on
            // each pass instead.
            let scratch = crate::server::storage::stack_scratch(stack_root);
            let view = stacker.view();
            let needed = (view.width * view.height * view.channels) as u64
                * 4
                * u64::from(accepted_frames);
            let room = crate::server::cache_budget::available_bytes(stack_root);
            let scratch = room
                .is_none_or(|room| needed.saturating_add(needed / 10) <= room)
                .then_some(scratch);
            if scratch.is_none() {
                passes.set(3);
                tracing::warn!(
                    job_id,
                    group_index = group.index,
                    needed_mib = needed / (1024 * 1024),
                    "Final pass reading every frame on each pass: the stack folder's disk is short of room for its scratch files"
                );
            }
            // The live accumulator is no longer needed. Release it before
            // revisiting early transients.
            drop(stacker);
            tracing::info!(
                job_id,
                group_index = group.index,
                accepted_frames,
                "Starting final transient rejection"
            );
            let result = final_integration::integrate(
                &group,
                &ledger,
                method,
                gpu,
                &plan,
                &session_cosmetics,
                scratch.as_deref(),
                cancel,
                &mut report_pass,
            );
            let result = match result {
                Ok(result) => result,
                Err(seiza_stacking::Error::Cancelled) => return Ok(GroupOutcome::Cancelled),
                Err(error) => return Err(format!("Final transient rejection failed: {error}")),
            };
            let mut diagnostics = result.frames.iter();
            state.stack_previews.update(job_id, |job| {
                for frame in &mut job.groups[group.index].frames {
                    if matches!(frame.disposition.as_str(), "reference" | "accepted")
                        && let Some(diagnostic) = diagnostics.next()
                    {
                        frame.integrated_fraction = Some(
                            diagnostic.integrated_samples as f32
                                / result.snapshot.image.sample_count() as f32,
                        );
                    }
                }
            });
            let rejected_samples: u64 = result
                .snapshot
                .rejected_samples
                .iter()
                .map(|&count| u64::from(count))
                .sum();
            tracing::info!(
                job_id,
                group_index = group.index,
                rejected_samples,
                "Final transient rejection completed"
            );
            result.snapshot.image
        } else {
            stacker
                .into_snapshot()
                .map_err(|error| error.to_string())?
                .image
        };
        if cancel.requested() {
            return Ok(GroupOutcome::Cancelled);
        }
        if north_up {
            state.stack_previews.update(job_id, |job| {
                job.groups[group.index].phase = "orienting".into();
            });
        }
        // Registration already absorbs a meridian flip, so a stack is published
        // in the reference frame's own rotation unless the caller asks for the
        // shared north-up grid that a mosaic needs. The one correction it still
        // makes is a half turn when the reference sits on the thinner side of a
        // flip, so the result faces the way most of the night was shot.
        let (image, clipped, sky_orientation, mut output_cards) = if group.mosaic {
            let orientation = StackSkyOrientation::source_frame(integrated.width, integrated.height, "mosaic canvas");
            (integrated, clipped, orientation, Vec::new())
        } else if north_up {
            let (source_wcs, orientation_source) =
                resolve_stack_wcs(state, &ctx, &group.frames[0], &reference_headers)?;
            let orientation = seiza_stacking::SkyOrientationPlan::new(
                integrated.width,
                integrated.height,
                &source_wcs,
            )
            .map_err(|error| error.to_string())?;
            let oriented = orientation
                .apply_with_gpu(&integrated, &gpu.options())
                .map_err(|error| error.to_string())?;
            let clipped = clipped
                .map(|clipped| orientation.apply_with_gpu(&clipped, &gpu.options()))
                .transpose()
                .map_err(|error| error.to_string())?;
            let record = StackSkyOrientation {
                convention: seiza_stacking::SKY_ORIENTATION_NAME.into(),
                version: seiza_stacking::SKY_ORIENTATION_VERSION,
                source: orientation_source,
                output_width: orientation.output_width(),
                output_height: orientation.output_height(),
                source_to_output: orientation.source_to_output(),
            };
            (oriented, clipped, record, orientation.fits_header_cards())
        } else {
            let (turn, decided_by) = half_turn_decision(anchor, &orientation_vote);
            tracing::info!(
                "Stack preview {} group {}: publishing {}, decided by {decided_by}",
                job_id,
                group.index,
                if turn {
                    "half a turn out of the reference frame"
                } else {
                    "in the reference frame's rotation"
                }
            );
            let (width, height) = (integrated.width, integrated.height);
            if turn {
                (
                    half_turn(integrated),
                    clipped.map(half_turn),
                    StackSkyOrientation::source_frame_half_turn(width, height, decided_by),
                    Vec::new(),
                )
            } else {
                (
                    integrated,
                    clipped,
                    StackSkyOrientation::source_frame(width, height, decided_by),
                    Vec::new(),
                )
            }
        };
        state.stack_previews.update(job_id, |job| {
            let status = &mut job.groups[group.index];
            status.phase = "rendering".into();
            status.sky_orientation = Some(sky_orientation);
        });
        let fits_destination = fits_path(stack_root, job_id, group.index);
        let fits_parent = fits_destination
            .parent()
            .ok_or_else(|| "Stack FITS path has no parent".to_string())?;
        std::fs::create_dir_all(fits_parent).map_err(|error| error.to_string())?;
        let fits_temporary =
            fits_destination.with_extension(format!("{}.tmp.fits", std::process::id()));
        output_cards.push(
            seiza_fits::WriteHeaderCard::new(
                "STACKCNT",
                seiza_fits::HeaderValue::Integer(i64::from(accepted_frames)),
            )
            .with_comment("accepted input frames"),
        );
        output_cards.push(
            seiza_fits::WriteHeaderCard::new(
                "STACKREJ",
                seiza_fits::HeaderValue::Integer(i64::from(rejected_frames)),
            )
            .with_comment("rejected input frames"),
        );
        let exposures = accepted_exposures(&ledger);
        let progressive = snr::ProgressiveSnr::new(order, points, &exposures);
        output_cards.push(
            seiza_fits::WriteHeaderCard::new(
                "SNRORDER",
                seiza_fits::HeaderValue::String(order.as_str().into()),
            )
            .with_comment("frame order the SNR curve was measured in"),
        );
        if let Some(deepest) = progressive.points.last() {
            output_cards.push(
                seiza_fits::WriteHeaderCard::new(
                    "SNRNOISE",
                    seiza_fits::HeaderValue::Float(deepest.noise),
                )
                .with_comment("measured pixel-to-pixel noise"),
            );
            output_cards.push(
                seiza_fits::WriteHeaderCard::new(
                    "SNRVALUE",
                    seiza_fits::HeaderValue::Float(deepest.snr),
                )
                .with_comment("measured signal-to-noise ratio"),
            );
        }
        if let Some(analysis) = &progressive.analysis {
            output_cards.push(
                seiza_fits::WriteHeaderCard::new(
                    "SNRSLOPE",
                    seiza_fits::HeaderValue::Float(analysis.noise_exponent),
                )
                .with_comment("fitted noise exponent; -0.5 is perfect averaging"),
            );
            output_cards.push(
                seiza_fits::WriteHeaderCard::new(
                    "SNRVERDT",
                    seiza_fits::HeaderValue::String(analysis.verdict.as_str().into()),
                )
                .with_comment("reading of the progressive SNR curve"),
            );
        }
        seiza_stacking::write_linear_image_fits_f32(
            &fits_temporary,
            &image,
            &reference_headers,
            &output_cards,
        )
        .map_err(|error| error.to_string())?;
        // The map goes first, so a stack is never published beside an
        // earlier build's map.
        write_clipped(
            &clipped_path(stack_root, job_id, group.index),
            clipped.as_ref(),
        )?;
        std::fs::rename(&fits_temporary, &fits_destination).map_err(|error| error.to_string())?;
        render_group_previews(
            state,
            &ctx,
            stack_root,
            job_id,
            group.index,
            &image,
            clipped.as_ref(),
        )?;
        let snr_url = state.stack_previews.get(job_id).and_then(|current| {
            publish_snr_artifact(
                &snr_path(stack_root, job_id, group.index),
                &progressive,
                format!(
                    "/api/db/{}/stack-previews/{}/{}/snr?v={}",
                    database_id, job_id, group.index, current.artifact_revision
                ),
            )
        });
        state.stack_previews.update(job_id, |job| {
            let status = &mut job.groups[group.index];
            status.snr = Some(progressive);
            status.snr_url = snr_url;
        });
        Ok(GroupOutcome::Built)
    })
}

/// Render a channel's card previews from its integrated image, and record
/// the stretch release that rendered them. A one-shot-color stack's preview
/// is color calibrated against Gaia first, as its processing is by default;
/// the FITS stays as integrated. A build renders the image it just
/// integrated, and a preview refresh the FITS that build wrote. `clipped`,
/// the stack's clipped map, keeps a calibrated preview's clipped star cores
/// neutral.
pub(super) fn render_group_previews(
    state: &AppState,
    ctx: &DatabaseContext,
    stack_root: &FsPath,
    job_id: &str,
    group_index: usize,
    image: &seiza_stacking::LinearImage,
    clipped: Option<&seiza_stacking::LinearImage>,
) -> Result<(), String> {
    let calibrated = if image.channels == 3 {
        let status = state
            .stack_previews
            .get(job_id)
            .and_then(|job| job.groups.get(group_index).cloned());
        let source = osc_calibration_source(
            state,
            ctx,
            status.as_ref().and_then(|status| status.reference_image_id),
            status.and_then(|status| status.sky_orientation),
        );
        let plan = source.plan(color_calibration::StackColorCalibration::default());
        let open_catalog = || {
            state
                .astrometry
                .gaia_photometry_catalog()
                .map(|(catalog, _)| catalog)
        };
        let (calibrated, outcome) =
            color_calibration::calibrate_stack(&plan, image, clipped, &open_catalog);
        let basis = source.basis();
        state.stack_previews.update(job_id, |job| {
            job.groups[group_index].color_calibration = Some(outcome);
            job.groups[group_index].color_calibration_basis = Some(basis);
        });
        calibrated
    } else {
        None
    };
    stretch::render_image_previews_atomic(
        calibrated.as_ref().unwrap_or(image),
        &stretch::default_linear_config(),
        stretch::StackStretchSourceTransfer::Linear,
        &preview_path(stack_root, job_id, group_index),
        &original_preview_path(stack_root, job_id, group_index),
    )
    .map_err(|error| error.to_string())?;
    state.stack_previews.update(job_id, |job| {
        job.groups[group_index].preview_stretch_version =
            Some(stretch::SEIZA_STRETCH_OUTPUT_VERSION.into());
    });
    Ok(())
}

/// The reference frame's WCS from sources already at hand: a cached
/// pixel-derived solve first, then a valid embedded FITS WCS. Never solves, so
/// a caller that only wants to know which way up the frame is does not pay for
/// a plate solve.
pub(super) fn cached_or_embedded_wcs(
    ctx: &DatabaseContext,
    reference: &PreparedFrame,
    headers: &[(String, seiza_fits::HeaderValue)],
) -> Option<(seiza::Wcs, String)> {
    if let Some(solution) = ctx
        .astrometry_evidence
        .evidence_for_source(
            &ctx.cache_dir_path,
            reference.image_id,
            reference.expected_target,
        )
        .and_then(|analysis| analysis.solution)
    {
        return Some((
            crate::astrometry::wcs_from_response(&solution.wcs),
            "cached_pixel_solve".into(),
        ));
    }
    let embedded =
        crate::astrometry_headers::FitsAstrometryHeaders::from_headers(headers).embedded_wcs?;
    let value = embedded.value;
    Some((
        seiza::Wcs {
            crval: (value.crval[0], value.crval[1]),
            crpix: (value.crpix[0], value.crpix[1]),
            cd: value.cd,
            sip: None,
        },
        "embedded_wcs".into(),
    ))
}

pub(super) fn resolve_stack_wcs(
    state: &AppState,
    ctx: &DatabaseContext,
    reference: &PreparedFrame,
    headers: &[(String, seiza_fits::HeaderValue)],
) -> Result<(seiza::Wcs, String), String> {
    if let Some(resolved) = cached_or_embedded_wcs(ctx, reference, headers) {
        return Ok(resolved);
    }
    let analysis = state
        .astrometry
        .solve_image(
            reference.image_id,
            &reference.path,
            reference.expected_target,
        )
        .map_err(|error| format!("Plate solving the stack reference failed: {error}"))?;
    let solution = analysis.solution.ok_or_else(|| {
        let detail = analysis
            .error
            .unwrap_or_else(|| "the solver found no match".into());
        format!(
            "Stack preview needs a plate solution for north-up display, but {detail}. Install the Seiza solver catalogs or solve a source image first."
        )
    })?;
    Ok((
        crate::astrometry::wcs_from_response(&solution.wcs),
        "stack_reference_solve".into(),
    ))
}
