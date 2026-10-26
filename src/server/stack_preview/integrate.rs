//! A channel's integration: what a group build does between its
//! calibration plan and its published artifacts. It opens or resumes the
//! live stack, pushes the frames session by session, checkpoints the
//! accumulator, measures the SNR curve, runs the final pass, orients the
//! result and writes the stack FITS.
//!
//! It never reads the catalog or the build record. Whoever runs it hears
//! what happens through [`ChannelEvents`]: this server applies each event to
//! the build's card, and a worker on another machine sends them to the
//! server. See `docs/design/remote-workers.md`.

use std::time::Duration;

use seiza_stacking::{FrameDisposition, LiveStacker};

use super::*;

/// What an integration reports while it runs.
pub(super) trait ChannelEvents {
    /// A sentence for the card's calibration warning.
    fn calibration_note(&mut self, note: String);
    /// A sentence for the card's note about how the stack was built.
    fn resume_note(&mut self, note: &str);
    /// The checkpoint could not be reopened. It is to be removed, and the
    /// build starts over.
    fn checkpoint_unusable(&mut self, why: &str);
    /// The frames the stack starts from: a checkpoint's, or the reference
    /// alone.
    fn frames_restored(&mut self, restored: RestoredFrames);
    /// One more frame's decision.
    fn frame(&mut self, decision: StackFrameDecision, exposure_seconds: f64);
    fn snr(&mut self, progressive: snr::ProgressiveSnr);
    fn phase(&mut self, phase: String, final_pass: Option<FinalPassProgress>);
    /// How much of each admitted frame the final pass integrated, in ledger
    /// order.
    fn integrated_fractions(&mut self, fractions: Vec<f32>);
    fn sky_orientation(&mut self, orientation: StackSkyOrientation);
    /// A running image of the stack, when one is due.
    fn live(&mut self, stacker: &LiveStacker, accepted: u32, force: bool);
    /// How many frames the next batch may push, so the running image keeps
    /// its cadence.
    fn batch_frames(&self) -> usize;
    fn observe_batch(&mut self, elapsed: Duration, frames: usize);
    /// A running image, as a PNG, that a worker stacking the channel
    /// rendered and sent.
    fn worker_image(&mut self, _png: &[u8], _accepted: u32) {}
}

/// The frames a stack starts from, for its card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct RestoredFrames {
    pub frames: Vec<StackFrameDecision>,
    pub accepted: usize,
    pub rejected: usize,
    /// Frames a checkpoint brought, which this build does not push again.
    pub reused: usize,
    pub output_channels: usize,
    pub total_exposure_seconds: f64,
    pub snr: Option<snr::ProgressiveSnr>,
}

/// Which way the stack is published.
#[derive(Debug, Clone)]
pub(super) enum Orientation {
    /// On the shared north-up grid, from the reference frame's solve.
    NorthUp { wcs: seiza::Wcs, source: String },
    /// In the reference frame's rotation, or half a turn from it when the
    /// anchor or the exposure vote says so.
    Reference {
        anchor: Option<(bool, &'static str)>,
    },
    /// On the canvas a mosaic's planning chose.
    MosaicCanvas,
}

/// Which checkpoint a group's accumulator is saved as.
#[derive(Debug, Clone)]
pub(super) struct CheckpointIdentity {
    pub target_id: i32,
    pub filter_name: String,
    pub exposure_group_key: Option<String>,
}

/// Where an integration writes.
#[derive(Debug, Clone)]
pub(super) struct ChannelOutputs {
    pub fits: PathBuf,
    pub clipped: PathBuf,
    pub checkpoint_context: PathBuf,
    pub checkpoint_manifest: PathBuf,
    /// Where the final pass keeps a copy of each admitted frame, when its
    /// disk has room.
    pub scratch: PathBuf,
    /// Where a mosaic's planning keeps its temporary reference frames.
    pub mosaic_scratch: PathBuf,
}

/// Everything an integration needs.
pub(super) struct ChannelWork<'a> {
    /// For the log.
    pub job_id: &'a str,
    pub group: &'a PreparedGroup,
    pub plan: &'a crate::calibration::CalibrationPlan,
    pub checkpoint: Option<resume::ResumeState>,
    pub accepted_only: bool,
    pub scoring: StackScoringSettings,
    pub method: StackMethod,
    pub gpu: StackGpu,
    pub order: snr::StackFrameOrder,
    pub calibration_fingerprint: &'a str,
    pub identity: CheckpointIdentity,
    pub orientation: Orientation,
    pub outputs: ChannelOutputs,
    pub pool: &'a rayon::ThreadPool,
    pub threads: execution::ThreadBudget,
    pub available_memory: Option<u64>,
    pub worker_policy: &'a crate::concurrency::WorkerPolicy,
    pub cancel: &'a Stop,
}

/// What a finished integration made. The stack FITS and the clipped map are
/// written; the images stay in memory for the previews.
pub(super) struct BuiltChannel {
    pub image: seiza_stacking::LinearImage,
    pub clipped: Option<seiza_stacking::LinearImage>,
    pub progressive: snr::ProgressiveSnr,
}

pub(super) enum ChannelOutcome {
    Built(Box<BuiltChannel>),
    Cancelled,
}

/// Remove a checkpoint's files.
fn remove_checkpoint(outputs: &ChannelOutputs) {
    let _ = std::fs::remove_file(&outputs.checkpoint_context);
    let _ = std::fs::remove_file(&outputs.checkpoint_manifest);
}

/// Integrate a channel. Blocking; runs Seiza on `work.pool`.
pub(super) fn integrate_channel(
    work: ChannelWork<'_>,
    events: &mut (dyn ChannelEvents + Send),
) -> Result<ChannelOutcome, String> {
    let ChannelWork {
        job_id,
        group,
        plan,
        checkpoint,
        accepted_only,
        scoring,
        method,
        gpu,
        order,
        calibration_fingerprint,
        identity,
        orientation,
        outputs,
        pool,
        threads,
        available_memory,
        worker_policy,
        cancel,
    } = work;
    let weighting = method.weighting;
    let session_cosmetics = SessionCosmetics::for_plan(plan, group.calibration);
    let cosmetic = session_cosmetics.whole_group();
    let reference_frame = crate::image_io::open_linear_frame(&group.frames[0].path)
        .map_err(|error| error.to_string())?;
    let reference_is_bayer = reference_frame.bayer.is_some();
    let output_channels = if reference_is_bayer {
        3_u64
    } else {
        reference_frame.image.channels as u64
    };
    let pixels = reference_frame.image.pixel_count();
    let frame_size = (reference_frame.image.width, reference_frame.image.height);
    let mut pipeline_budget = execution::plan_pipeline(
        available_memory,
        worker_policy,
        pixels,
        pixels.saturating_mul(output_channels as usize),
        plan,
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
                    events.checkpoint_unusable("Full restack: the checkpoint files did not match");
                    None
                }
                Err(error) => {
                    tracing::warn!(
                        "Stack checkpoint could not be reopened ({error}); rebuilding from scratch"
                    );
                    events.checkpoint_unusable(
                        "Full restack: the checkpoint could not be reopened",
                    );
                    None
                }
            }
        });
        let mut orientation_vote = OrientationVote::default();
        let mut largest_frame = frame_size;
        let (stacker, ledger, points) = if group.mosaic {
            events.phase("planning mosaic".into(), None);
            events.resume_note("Mosaic canvases are replanned when their inputs change");
            let limit = mosaic::maximum_samples(available_memory, worker_policy, plan)?;
            let (stacker, panels) = mosaic::plan(
                group,
                plan,
                method.stack_options(cosmetic, gpu),
                &outputs.mosaic_scratch,
                limit,
                cancel,
                |count, width, height| {
                    events.phase(
                        format!("planning mosaic: {count} references, {width} × {height}"),
                        None,
                    );
                },
            )?;
            largest_frame = (
                largest_frame.0.max(panels.0),
                largest_frame.1.max(panels.1),
            );
            for note in stacker.expansion_planning_notes() {
                events.resume_note(note);
            }
            events.frames_restored(RestoredFrames {
                frames: Vec::new(),
                accepted: 0,
                rejected: 0,
                reused: 0,
                output_channels: output_channels as usize,
                total_exposure_seconds: 0.0,
                snr: None,
            });
            (stacker, Vec::new(), Vec::new())
        } else {
            match restored {
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
                let accepted = ledger
                    .iter()
                    .filter(|frame| {
                        matches!(
                            frame.decision.disposition.as_str(),
                            "reference" | "accepted"
                        )
                    })
                    .count();
                let exposures = accepted_exposures(&ledger);
                events.frames_restored(RestoredFrames {
                    frames: ledger.iter().map(|frame| frame.decision.clone()).collect(),
                    accepted,
                    rejected: ledger.len() - accepted,
                    reused: ledger.len(),
                    output_channels: output_channels as usize,
                    total_exposure_seconds: ledger
                        .iter()
                        .filter(|frame| frame.rotation_radians.is_some())
                        .map(|frame| frame.exposure_seconds)
                        .sum(),
                    snr: (!points.is_empty())
                        .then(|| snr::ProgressiveSnr::new(order, points.clone(), &exposures)),
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
                let (stacker, calibration_bypassed) =
                    match LiveStacker::new(reference_frame, reference_masters, options.clone()) {
                        Ok(stacker) => (stacker, false),
                        Err(error)
                            if group.calibration != crate::calibration::CalibrationMode::On =>
                        {
                            tracing::warn!(
                                "reference refused its session masters; stacking raw: {error}"
                            );
                            events.calibration_note(format!(
                                "Stacked without calibration: the reference frame's masters \
                                 were refused — {error}"
                            ));
                            let reference_frame =
                                crate::image_io::open_linear_frame(&group.frames[0].path)
                                    .map_err(|error| error.to_string())?;
                            (
                                LiveStacker::new(
                                    reference_frame,
                                    seiza_stacking::CalibrationMasters::default(),
                                    options,
                                )
                                .map_err(|error| error.to_string())?,
                                true,
                            )
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
                events.frames_restored(RestoredFrames {
                    frames: vec![reference_decision.clone()],
                    accepted: 1,
                    rejected: 0,
                    reused: 0,
                    output_channels: output_channels as usize,
                    total_exposure_seconds: reference_exposure,
                    snr: None,
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
        }
        };
        Ok::<_, String>((stacker, ledger, points, orientation_vote, largest_frame))
    });
    if cancel.requested() {
        return Ok(ChannelOutcome::Cancelled);
    }
    let (mut stacker, mut ledger, mut points, mut orientation_vote, largest_frame) = initialized?;
    // A mosaic's canvas sets the stack's own memory; each frame still
    // arrives at its own size and lands on part of the canvas. The largest
    // of the first frame and the panels' frames stands for every frame.
    if group.mosaic {
        let view = stacker.view();
        pipeline_budget = execution::plan_mosaic_pipeline(
            available_memory,
            worker_policy,
            (view.width, view.height),
            view.channels,
            largest_frame,
            plan,
            &threads,
        )?;
        threads = pipeline_budget.threads;
        tracing::info!(
            job_id,
            group_index = group.index,
            canvas_width = view.width,
            canvas_height = view.height,
            frame_width = largest_frame.0,
            frame_height = largest_frame.1,
            compute_workers = threads.compute_workers,
            preparation_workers = threads.preparation_workers,
            serial = threads.serial,
            memory = %pipeline_budget.summary(),
            "Stack pipeline memory replanned for the mosaic canvas"
        );
    }
    // The reference frame alone is the first depth on the curve and the
    // baseline every later depth is read against: one frame's noise.
    if points.is_empty()
        && let Some(point) = pool.install(|| {
            snr::measure_point(stacker.view(), integrated_exposure(&ledger), group.mosaic)
        })
    {
        points.push(point);
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
                    remove_checkpoint(&outputs);
                    return;
                }
                let manifest = resume::ResumeManifest {
                    schema_version: resume::RESUME_SCHEMA_VERSION,
                    stacking_version: SEIZA_STACKING_OUTPUT_VERSION.into(),
                    target_id: identity.target_id,
                    filter_name: identity.filter_name.clone(),
                    exposure_group_key: identity.exposure_group_key.clone(),
                    accepted_only,
                    scoring,
                    method,
                    calibration_fingerprint: calibration_fingerprint.to_string(),
                    order,
                    snr_points: points.to_vec(),
                    frames: ledger.to_vec(),
                };
                let saved = outputs
                    .checkpoint_context
                    .parent()
                    .ok_or_else(|| "checkpoint path has no parent".to_string())
                    .and_then(|parent| {
                        std::fs::create_dir_all(parent).map_err(|error| error.to_string())
                    })
                    .and_then(|()| {
                        stacker
                            .save_context(&outputs.checkpoint_context)
                            .map_err(|error| error.to_string())
                    })
                    .and_then(|()| resume::store_manifest(&outputs.checkpoint_manifest, &manifest));
                if let Err(error) = saved {
                    // A checkpoint is an optimization; a build never fails over
                    // it. Discard the pair so nothing resumes from half a save.
                    tracing::warn!("Failed to save stack checkpoint: {error}");
                    remove_checkpoint(&outputs);
                }
            })
        };

    if !cancel.requested() && stacker.view().accepted_frames > 0 {
        events.live(&stacker, stacker.view().accepted_frames, false);
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
        // Consecutive frames of one calibration session push as one
        // pipelined batch, with the session's masters swapped in first. The
        // frames after the reference are chronological, so a night is one
        // batch and a single-session group is exactly one call.
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
        batch_end = batch_end.min(batch_start.saturating_add(events.batch_frames()));
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
            events.calibration_note(format!(
                "Session {} stacked uncalibrated: its masters were refused — {error}",
                session + 1
            ));
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
            pool,
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
                events.frame(decision, exposure);

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
        events.observe_batch(batch_started.elapsed(), consumed);
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
            && let Some(point) = pool.install(|| {
                snr::measure_point(stacker.view(), integrated_exposure(&ledger), group.mosaic)
            })
        {
            points.push(point);
            let exposures = accepted_exposures(&ledger);
            events.snr(snr::ProgressiveSnr::for_group(
                order,
                points.clone(),
                &exposures,
                group.mosaic,
            ));
        }
        if !cancelled && !cancel.requested() {
            events.live(&stacker, accepted_frames, false);
        }
    }

    if cancelled {
        // The frames that did land are checkpointed, so building again
        // continues from them.
        save_checkpoint(&stacker, &ledger, &points);
        return Ok(ChannelOutcome::Cancelled);
    }
    if !cancel.requested() {
        // Keep the deepest live estimate visible throughout final rejection.
        events.live(&stacker, stacker.view().accepted_frames, true);
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
        events.calibration_note(format!(
            "{} frame(s) could not be calibrated and were left out — {}",
            calibration_rejections.len(),
            counts
                .iter()
                .map(|(reason, count)| format!("{count}× {reason}"))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    // Last exit before the job writes anything. Orienting and rendering
    // follow, and a stop after this point would have to clean up published
    // artifacts.
    if cancel.requested() {
        return Ok(ChannelOutcome::Cancelled);
    }
    pool.install(|| {
        if group.mosaic {
            let expected = group.frames.iter().map(|f| f.target_id).collect::<HashSet<_>>();
            let joined = group
                .frames
                .iter()
                .zip(&ledger)
                .filter(|(_, record)| record.admitted())
                .map(|(frame, _)| frame.target_id)
                .collect::<HashSet<_>>();
            let missing = expected.difference(&joined).copied().collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(format!("{}: no frames aligned from panel target(s) {missing:?}. Inspect the rejected frames.", mosaic::INCOMPLETE));
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
            events.resume_note(
                "Final pass skipped: PSF Guard cannot yet replay a Bayer drizzle, so this is \
                 the live stack",
            );
            false
        } else {
            reintegrate
        };
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
            events.phase(
                format!(
                    "Rejecting transients: pass {pass}/{passes}, frame {}/{count}",
                    index + 1
                ),
                Some(FinalPassProgress {
                    pass,
                    passes,
                    frame: index + 1,
                    frames: count,
                }),
            );
        };
        let integrated = if reintegrate {
            // The final pass keeps each admitted frame in a scratch file,
            // four bytes a sample, when the scratch folder's disk has room
            // for all of them and a margin; otherwise it reads every frame
            // on each pass instead.
            let view = stacker.view();
            let needed = (view.width * view.height * view.channels) as u64
                * 4
                * u64::from(accepted_frames);
            let volume = outputs
                .scratch
                .ancestors()
                .find(|folder| folder.exists())
                .unwrap_or(&outputs.scratch);
            let room = crate::server::cache_budget::available_bytes(volume);
            let scratch = room
                .is_none_or(|room| needed.saturating_add(needed / 10) <= room)
                .then_some(outputs.scratch.as_path());
            if scratch.is_none() {
                passes.set(3);
                tracing::warn!(
                    job_id,
                    group_index = group.index,
                    needed_mib = needed / (1024 * 1024),
                    "Final pass reading every frame on each pass: the scratch folder's disk is short of room for its scratch files"
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
                group,
                &ledger,
                method,
                gpu,
                plan,
                &session_cosmetics,
                scratch,
                cancel,
                &mut report_pass,
            );
            let result = match result {
                Ok(result) => result,
                Err(seiza_stacking::Error::Cancelled) => return Ok(ChannelOutcome::Cancelled),
                Err(error) => return Err(format!("Final transient rejection failed: {error}")),
            };
            let samples = result.snapshot.image.sample_count() as f32;
            events.integrated_fractions(
                result
                    .frames
                    .iter()
                    .map(|diagnostic| diagnostic.integrated_samples as f32 / samples)
                    .collect(),
            );
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
            return Ok(ChannelOutcome::Cancelled);
        }
        // Registration already absorbs a meridian flip, so a stack is published
        // in the reference frame's own rotation unless the caller asks for the
        // shared north-up grid that a mosaic needs. The one correction it still
        // makes is a half turn when the reference sits on the thinner side of a
        // flip, so the result faces the way most of the night was shot.
        let (image, clipped, sky_orientation, mut output_cards) = match orientation {
            Orientation::MosaicCanvas => {
                let orientation = StackSkyOrientation::source_frame(
                    integrated.width,
                    integrated.height,
                    "mosaic canvas",
                );
                (integrated, clipped, orientation, Vec::new())
            }
            Orientation::NorthUp {
                wcs,
                source: orientation_source,
            } => {
                events.phase("orienting".into(), None);
                let orientation = seiza_stacking::SkyOrientationPlan::new(
                    integrated.width,
                    integrated.height,
                    &wcs,
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
            }
            Orientation::Reference { anchor } => {
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
            }
        };
        events.sky_orientation(sky_orientation);
        events.phase("rendering".into(), None);
        let fits_parent = outputs
            .fits
            .parent()
            .ok_or_else(|| "Stack FITS path has no parent".to_string())?;
        std::fs::create_dir_all(fits_parent).map_err(|error| error.to_string())?;
        let fits_temporary = outputs
            .fits
            .with_extension(format!("{}.tmp.fits", temporary_tag()));
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
        let progressive = snr::ProgressiveSnr::for_group(order, points, &exposures, group.mosaic);
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
        write_clipped(&outputs.clipped, clipped.as_ref())?;
        std::fs::rename(&fits_temporary, &outputs.fits).map_err(|error| error.to_string())?;
        Ok(ChannelOutcome::Built(Box::new(BuiltChannel {
            image,
            clipped,
            progressive,
        })))
    })
}
