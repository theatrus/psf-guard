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
    // Which way the stack is published is settled before it is stacked: a
    // north-up stack needs its reference's plate solve, and a stack that
    // cannot get one fails before it spends its frames.
    let orientation = if group.mosaic {
        // A mosaic plans its own canvas, from no single reference.
        state.stack_previews.update(job_id, |job| {
            job.groups[group.index].reference_image_id = None;
        });
        integrate::Orientation::MosaicCanvas
    } else if north_up {
        state.stack_previews.update(job_id, |job| {
            job.groups[group.index].phase = "orienting".into();
        });
        let headers = crate::image_io::read_header(&group.frames[0].path)
            .map_err(|error| error.to_string())?;
        // On the build's leased pool, as everything this build computes.
        let (wcs, source) =
            pool.install(|| resolve_stack_wcs(state, &ctx, &group.frames[0], &headers))?;
        state.stack_previews.update(job_id, |job| {
            job.groups[group.index].phase = "stacking".into();
        });
        integrate::Orientation::NorthUp { wcs, source }
    } else {
        integrate::Orientation::Reference { anchor }
    };
    let identity = integrate::CheckpointIdentity {
        target_id: group_target_id,
        filter_name: group_filter_name.clone(),
        exposure_group_key: group_exposure_key.clone(),
    };
    let outputs = integrate::ChannelOutputs {
        fits: fits_path(stack_root, job_id, group.index),
        clipped: clipped_path(stack_root, job_id, group.index),
        checkpoint_context: resume::context_path(
            stack_root,
            database_id,
            group_target_id,
            &group_filter_name,
            group_exposure_key.as_deref(),
        ),
        checkpoint_manifest: resume::manifest_path(
            stack_root,
            database_id,
            group_target_id,
            &group_filter_name,
            group_exposure_key.as_deref(),
        ),
        scratch: crate::server::storage::stack_scratch(stack_root),
        mosaic_scratch: stack_dir(stack_root, job_id),
    };
    // What the card lists before the stack begins, such as the frames the
    // build left out: a worker, and this server after it, start from it.
    let listed_before = state
        .stack_previews
        .get(job_id)
        .and_then(|job| {
            job.groups
                .get(group.index)
                .map(|status| status.frames.clone())
        })
        .unwrap_or_default();
    let stack_here = |pool: &crate::concurrency::ComputePool,
                      checkpoint: Option<resume::ResumeState>| {
        let mut events = CardEvents {
            state,
            job_id,
            index: group.index,
            live: Some(live::Publisher::new(
                state,
                stack_root,
                job_id,
                group.index,
                pool,
            )),
            stack_root,
            database_id,
            identity: identity.clone(),
            listed_before: Some(listed_before.clone()),
        };
        integrate::integrate_channel(
            integrate::ChannelWork {
                job_id,
                group: &group,
                plan: &plan,
                checkpoint,
                accepted_only,
                scoring,
                method,
                gpu,
                order,
                calibration_fingerprint: &calibration_fingerprint,
                identity: identity.clone(),
                orientation: orientation.clone(),
                outputs: outputs.clone(),
                pool,
                threads,
                available_memory,
                worker_policy,
                cancel,
            },
            &mut events,
        )
    };
    // A worker on another machine stacks the channel when Settings ›
    // Workers sends stack builds there. This server's cores are free for
    // other work while it does, and are leased again for the previews.
    let (integrated, lease, pool) = if remote_channel::wanted(state) {
        drop(pool);
        drop(lease);
        let mut events = CardEvents {
            state,
            job_id,
            index: group.index,
            live: None,
            stack_root,
            database_id,
            identity: identity.clone(),
            listed_before: Some(listed_before.clone()),
        };
        let remote = remote_channel::run_on_worker(
            state,
            remote_channel::RemoteChannel {
                job_id,
                automatic: state.stack_previews.is_automatic(job_id),
                group: &group,
                plan: &plan,
                checkpoint: checkpoint.clone(),
                accepted_only,
                scoring,
                method,
                gpu,
                order,
                calibration_fingerprint: &calibration_fingerprint,
                identity: identity.clone(),
                orientation: &orientation,
                outputs: &outputs,
                cancel,
            },
            &mut events,
        );
        let lease = state.lease_workers(priority, budget.workers);
        let pool = crate::concurrency::ComputePool::take("stack-preview", lease.workers)
            .map_err(|error| error.to_string())?;
        let integrated = match remote {
            Ok(outcome) => outcome,
            // Under workers first, this server stacks a channel no worker
            // could take, or one a worker failed, such as when it ran out
            // of memory.
            Err(
                remote_channel::NotStacked::NoWorker(message)
                | remote_channel::NotStacked::Failed(message),
            ) if state
                .workers
                .board
                .placement(crate::remote_work::protocol::TaskKind::StackChannel)
                .allows(true) =>
            {
                tracing::warn!(job_id, "{message}; this server stacks the channel");
                add_note_to(
                    state,
                    job_id,
                    group.index,
                    &format!("{message}; stacked on this server"),
                );
                stack_here(&pool, checkpoint)?
            }
            Err(
                remote_channel::NotStacked::NoWorker(message)
                | remote_channel::NotStacked::Failed(message),
            ) => return Err(message),
        };
        (integrated, lease, pool)
    } else {
        (stack_here(&pool, checkpoint)?, lease, pool)
    };
    let _held = lease;
    let built = match integrated {
        integrate::ChannelOutcome::Built(built) => built,
        integrate::ChannelOutcome::Cancelled => return Ok(GroupOutcome::Cancelled),
    };
    pool.install(|| {
        render_group_previews(
            state,
            &ctx,
            stack_root,
            job_id,
            group.index,
            &built.image,
            built.clipped.as_ref(),
        )
    })?;
    let snr_url = state.stack_previews.get(job_id).and_then(|current| {
        publish_snr_artifact(
            &snr_path(stack_root, job_id, group.index),
            &built.progressive,
            format!(
                "/api/db/{}/stack-previews/{}/{}/snr?v={}",
                database_id, job_id, group.index, current.artifact_revision
            ),
        )
    });
    state.stack_previews.update(job_id, |job| {
        let status = &mut job.groups[group.index];
        status.snr = Some(built.progressive);
        status.snr_url = snr_url;
    });
    Ok(GroupOutcome::Built)
}

/// Add a sentence to a channel card's note.
fn add_note_to(state: &AppState, job_id: &str, index: usize, sentence: &str) {
    state.stack_previews.update(job_id, |job| {
        add_note(&mut job.groups[index].resume_note, sentence);
    });
}

/// A channel's integration, as it shows on this server's build card.
pub(super) struct CardEvents<'a> {
    pub(super) state: &'a Arc<AppState>,
    pub(super) job_id: &'a str,
    pub(super) index: usize,
    /// The running image, for a channel this server stacks.
    pub(super) live: Option<live::Publisher<'a>>,
    pub(super) stack_root: &'a FsPath,
    pub(super) database_id: &'a str,
    pub(super) identity: integrate::CheckpointIdentity,
    /// The frames the card listed before the channel started, such as those
    /// the build left out. A worker that takes the channel over after
    /// another starts from these again.
    pub(super) listed_before: Option<Vec<StackFrameDecision>>,
}

impl CardEvents<'_> {
    fn update(&self, change: impl FnOnce(&mut StackGroupStatus)) {
        let index = self.index;
        self.state
            .stack_previews
            .update(self.job_id, |job| change(&mut job.groups[index]));
    }
}

impl integrate::ChannelEvents for CardEvents<'_> {
    fn calibration_note(&mut self, note: String) {
        self.update(|status| {
            let calibration = &mut status.calibration;
            calibration.warning = Some(match calibration.warning.take() {
                Some(previous) if previous.contains(&note) => previous,
                Some(previous) => format!("{previous}. {note}"),
                None => note,
            });
        });
    }

    fn resume_note(&mut self, note: &str) {
        self.update(|status| add_note(&mut status.resume_note, note));
    }

    fn checkpoint_unusable(&mut self, why: &str) {
        resume::discard(
            self.stack_root,
            self.database_id,
            self.identity.target_id,
            &self.identity.filter_name,
            self.identity.exposure_group_key.as_deref(),
        );
        self.update(|status| add_note(&mut status.resume_note, why));
    }

    fn frames_restored(&mut self, restored: integrate::RestoredFrames) {
        if self.listed_before.is_none() {
            self.listed_before = Some(
                self.state
                    .stack_previews
                    .get(self.job_id)
                    .and_then(|job| job.groups.get(self.index).map(|group| group.frames.clone()))
                    .unwrap_or_default(),
            );
        }
        let listed_before = self.listed_before.clone().unwrap_or_default();
        self.update(|status| {
            status.processed_frames = restored.frames.len();
            status.accepted_frames = restored.accepted;
            status.rejected_frames = restored.rejected;
            status.output_channels = restored.output_channels;
            status.total_exposure_seconds = restored.total_exposure_seconds;
            if restored.reused > 0 {
                // A checkpoint's ledger is the whole list.
                status.reused_frames = restored.reused;
                status.frames = restored.frames;
            } else {
                // A fresh stack's reference joins the frames the build
                // listed before, such as those it left out.
                status.frames = listed_before;
                status.frames.extend(restored.frames);
            }
            if restored.snr.is_some() {
                status.snr = restored.snr;
            }
        });
    }

    fn frame(&mut self, decision: StackFrameDecision, exposure_seconds: f64) {
        self.update(|status| {
            status.processed_frames += 1;
            if matches!(decision.disposition.as_str(), "accepted") {
                status.accepted_frames += 1;
                status.total_exposure_seconds += exposure_seconds;
            } else {
                status.rejected_frames += 1;
            }
            status.frames.push(decision);
        });
    }

    fn snr(&mut self, progressive: snr::ProgressiveSnr) {
        self.update(|status| status.snr = Some(progressive));
    }

    fn phase(&mut self, phase: String, final_pass: Option<FinalPassProgress>) {
        self.update(|status| {
            status.phase = phase;
            if final_pass.is_some() {
                status.final_pass = final_pass;
            }
        });
    }

    fn integrated_fractions(&mut self, fractions: Vec<f32>) {
        self.update(|status| {
            let mut fractions = fractions.into_iter();
            for frame in &mut status.frames {
                if matches!(frame.disposition.as_str(), "reference" | "accepted")
                    && let Some(fraction) = fractions.next()
                {
                    frame.integrated_fraction = Some(fraction);
                }
            }
        });
    }

    fn sky_orientation(&mut self, orientation: StackSkyOrientation) {
        self.update(|status| status.sky_orientation = Some(orientation));
    }

    fn live(&mut self, stacker: &seiza_stacking::LiveStacker, accepted: u32, force: bool) {
        if let Some(live) = &mut self.live {
            live.publish(stacker, accepted, force);
        }
    }

    fn batch_frames(&self) -> usize {
        self.live
            .as_ref()
            .map_or(usize::MAX, |live| live.batch_frames())
    }

    fn observe_batch(&mut self, elapsed: std::time::Duration, frames: usize) {
        if let Some(live) = &mut self.live {
            live.observe_batch(elapsed, frames);
        }
    }
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
