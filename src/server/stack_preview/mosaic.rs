//! Plan one canvas from panel references calibrated under their own sessions.
//!
//! Only the small reference set is written to scratch. Integration still reads
//! every original light, with its session's masters, through the normal pipeline.
use super::*;
use seiza_stacking::{CalibrationMasters, Continue, LiveStacker};
use std::collections::BTreeMap;

pub(super) fn maximum_samples(
    available: Option<u64>,
    policy: &crate::concurrency::WorkerPolicy,
    plan: &crate::calibration::CalibrationPlan,
) -> Result<usize, String> {
    let masters: u64 = plan.sessions.iter().fold(0_u64, |total, s| {
        total.saturating_add(s.masters.image_buffer_bytes() as u64)
    });
    let largest = plan
        .sessions
        .iter()
        .map(|s| s.masters.image_buffer_bytes() as u64)
        .max()
        .unwrap_or(0);
    let budget =
        (available.unwrap_or(1024 * 1024 * 1024) as f64 * policy.memory_budget_fraction) as u64;
    let samples = budget.saturating_sub(masters.saturating_add(largest.saturating_mul(2)))
        / STACK_BYTES_PER_OUTPUT_SAMPLE;
    if samples == 0 {
        return Err("Insufficient memory to plan a mosaic canvas".into());
    }
    Ok(usize::try_from(samples).unwrap_or(usize::MAX))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan(
    group: &PreparedGroup,
    masters: &crate::calibration::CalibrationPlan,
    mut options: seiza_stacking::StackOptions,
    scratch: &FsPath,
    maximum_samples: usize,
    cancel: &Stop,
    mut progress: impl FnMut(usize, usize, usize),
) -> Result<LiveStacker, String> {
    if options.cfa_integration == seiza_stacking::CfaIntegration::BayerDrizzle {
        return Err("Mosaic integration does not support Bayer drizzle. Turn off Bayer drizzle in Settings → Stacking, then build again.".into());
    }
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let temporary = tempfile::Builder::new()
        .prefix("mosaic-reference-")
        .tempdir_in(scratch)
        .map_err(|e| e.to_string())?;
    // Keep the highest-scored reference from each panel. Capture order does
    // not decide which night supplies the panel's registration reference.
    let mut references = BTreeMap::<i32, usize>::new();
    for (index, frame) in group.frames.iter().enumerate() {
        let chosen = references.entry(frame.target_id).or_insert(index);
        if frame.quality_score.unwrap_or(0.0) > group.frames[*chosen].quality_score.unwrap_or(0.0) {
            *chosen = index;
        }
    }
    let missing = group
        .mosaic_panels
        .iter()
        .filter(|id| !references.contains_key(id))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "Mosaic channel has no eligible frames for panel target(s) {missing:?}"
        ));
    }
    if references.len() < 2 {
        return Err("A mosaic channel needs eligible frames from at least two panels".into());
    }
    let mut indices = references.into_values().collect::<Vec<_>>();
    // Anchor on the same first panel in every channel, rather than on the
    // globally best frame (which can be a different panel for each filter).
    indices.sort_by_key(|&index| group.frames[index].target_id);
    let mut paths = Vec::new();
    for index in indices {
        if cancel.requested() {
            return Err("Mosaic planning cancelled".into());
        }
        let source = &group.frames[index];
        if source_fingerprint(&source.path) != source.source_fingerprint {
            return Err(format!(
                "Image {} changed before mosaic planning; rebuild the stack",
                source.image_id
            ));
        }
        let mut frame =
            crate::image_io::open_linear_frame(&source.path).map_err(|e| e.to_string())?;
        let session = masters.assignments[index];
        let calibration = &masters.sessions[session];
        // Never seed the canvas with raw photometry when its lights will be
        // calibrated. A refused reference must be fixed or excluded explicitly.
        calibration
            .masters
            .validate_light_frame_with(&frame, &calibration.match_tolerances)
            .map_err(|e| e.to_string())?;
        calibration
            .masters
            .apply(&mut frame.image, frame.exposure_seconds, frame.bayer)
            .map_err(|e| e.to_string())?;
        let frame = frame
            .into_prepared_with_gpu(&options.gpu)
            .map_err(|e| e.to_string())?;
        let path = temporary.path().join(format!("{}.fits", source.image_id));
        seiza_stacking::write_processed_image_fits_f32(&path, &frame.image, &frame.headers, &[])
            .map_err(|e| e.to_string())?;
        if source_fingerprint(&source.path) != source.source_fingerprint {
            return Err(format!(
                "Image {} changed while planning the mosaic; rebuild the stack",
                source.image_id
            ));
        }
        paths.push(path);
    }
    options.acceptance.minimum_overlap_fraction =
        seiza_stacking::FrameAcceptanceCriteria::EXPANDED_MINIMUM_OVERLAP_FRACTION;
    LiveStacker::with_expanding_reference(
        &paths,
        CalibrationMasters::default(),
        options,
        maximum_samples,
        |_, count, width, height| {
            progress(count, width, height);
            if cancel.requested() {
                Continue::No
            } else {
                Continue::Yes
            }
        },
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use seiza_stacking::{FrameDisposition, LinearImage, NormalizationMode};

    fn frame(path: PathBuf, panel: i32, id: i32) -> PreparedFrame {
        PreparedFrame {
            image_id: id,
            target_id: panel,
            acquired_date: None,
            quality_score: Some(1.0),
            source_fingerprint: source_fingerprint(&path),
            path,
            expected_target: None,
            exposure_seconds: 300.0,
        }
    }

    fn group(frames: Vec<PreparedFrame>) -> PreparedGroup {
        PreparedGroup {
            index: 0,
            mosaic: true,
            mosaic_panels: vec![1, 2, 3, 4],
            calibration: crate::calibration::CalibrationMode::Off,
            flat_star_masking: false,
            frames,
        }
    }

    fn integrate(group: &PreparedGroup, scratch: &FsPath) -> seiza_stacking::StackSnapshot {
        let plan = crate::calibration::CalibrationPlan::without_calibration(group.frames.len());
        integrate_with_plan(group, scratch, &plan, None)
    }

    fn integrate_with_plan(
        group: &PreparedGroup,
        scratch: &FsPath,
        plan: &crate::calibration::CalibrationPlan,
        normalization: Option<NormalizationMode>,
    ) -> seiza_stacking::StackSnapshot {
        let cosmetic = SessionCosmetics::none(plan.sessions.len());
        let method = StackMethod {
            weighting: method::StackWeighting::Equal,
            ..Default::default()
        };
        let cpu = StackGpu {
            enabled: false,
            ..Default::default()
        };
        let mut options = method.stack_options(None, cpu);
        if let Some(normalization) = normalization {
            options.normalization = normalization;
        }
        let cancel = Stop::new();
        let mut stacker = super::plan(
            group,
            plan,
            options,
            scratch,
            150_000_000,
            &cancel,
            |_, _, _| {},
        )
        .unwrap();
        assert_eq!(
            stacker.view().accepted_frames,
            0,
            "planning must not integrate its anchor"
        );
        let mut ledger = Vec::new();
        for (index, source) in group.frames.iter().enumerate() {
            use_session(&mut stacker, &plan.sessions[plan.assignments[index]]).unwrap();
            let frame = crate::image_io::open_linear_frame(&source.path).unwrap();
            let disposition = stacker.push(frame).unwrap();
            let FrameDisposition::Accepted(diagnostics) = disposition else {
                panic!("{}: {disposition:?}", source.path.display());
            };
            let mut decision = rejected_decision(source, String::new());
            decision.disposition = "accepted".into();
            decision.reason = None;
            decision.registered_mapping = Some(*diagnostics.mapping);
            ledger.push(resume::ResumeFrame {
                decision,
                exposure_seconds: 300.0,
                calibration_bypassed: false,
                retryable_failure: false,
                rotation_radians: Some(0.0),
            });
        }
        let live = stacker.into_snapshot().unwrap();
        let final_stack = final_integration::integrate(
            group,
            &ledger,
            StackMethod {
                weighting: method::StackWeighting::Equal,
                ..Default::default()
            },
            StackGpu {
                enabled: false,
                ..Default::default()
            },
            plan,
            &cosmetic,
            Some(scratch),
            &cancel,
            |_, _, _| {},
        )
        .unwrap()
        .snapshot;
        assert_eq!(
            (live.image.width, live.image.height),
            (final_stack.image.width, final_stack.image.height)
        );
        assert_eq!(final_stack.accepted_frames as usize, group.frames.len());
        final_stack
    }

    #[test]
    fn mosaic_planning_checks_memory_missing_panels_and_unsupported_modes() {
        let scratch = tempfile::tempdir().unwrap();
        let plan = crate::calibration::CalibrationPlan::without_calibration(2);
        let policy = crate::concurrency::WorkerPolicy {
            memory_budget_fraction: 0.5,
            ..Default::default()
        };
        assert_eq!(
            maximum_samples(Some(96 * 200), &policy, &plan).unwrap(),
            100
        );
        assert!(maximum_samples(Some(1), &policy, &plan).is_err());
        let group = group(vec![
            frame("missing-1.fits".into(), 1, 1),
            frame("missing-2.fits".into(), 2, 2),
        ]);
        let cancel = Stop::new();
        let result = super::plan(
            &group,
            &plan,
            Default::default(),
            scratch.path(),
            100,
            &cancel,
            |_, _, _| {},
        );
        assert!(result.err().unwrap().contains("[3, 4]"));
        let options = seiza_stacking::StackOptions {
            cfa_integration: seiza_stacking::CfaIntegration::BayerDrizzle,
            ..Default::default()
        };
        assert!(super::plan(
            &group,
            &plan,
            options,
            scratch.path(),
            100,
            &cancel,
            |_, _, _| {}
        )
        .err()
        .unwrap()
        .contains("Bayer drizzle"));
    }

    #[test]
    fn four_low_overlap_panels_integrate_on_one_canvas_and_replay_the_mappings() {
        let scratch = tempfile::tempdir().unwrap();
        let (width, height) = (600, 440);
        let mut stars = Vec::new();
        let mut seed = 93_u64;
        let mut random = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 32) as u32
        };
        for _ in 0..1700 {
            stars.push((
                random() as usize % 1160,
                random() as usize % 850,
                800.0 + (random() % 5000) as f32,
            ));
        }
        let mut frames = Vec::new();
        for panel in 0..4 {
            for repeat in 0..2 {
                let (left, top) = ((panel % 2) * 550 + repeat, (panel / 2) * 400 + repeat);
                let mut data = vec![100.0 + panel as f32 * 100.0; width * height];
                for &(sx, sy, amplitude) in &stars {
                    for dy in -5_i32..=5 {
                        for dx in -5_i32..=5 {
                            let x = sx as i32 + dx - left;
                            let y = sy as i32 + dy - top;
                            if x >= 0 && y >= 0 && x < width as i32 && y < height as i32 {
                                data[y as usize * width + x as usize] +=
                                    amplitude * (-(dx * dx + dy * dy) as f32 / 4.5).exp();
                            }
                        }
                    }
                }
                let path = scratch.path().join(format!("panel-{panel}-{repeat}.fits"));
                seiza_stacking::write_processed_image_fits_f32(
                    &path,
                    &LinearImage::new(width, height, 1, data).unwrap(),
                    &[],
                    &[],
                )
                .unwrap();
                frames.push(frame(path, panel + 1, panel * 2 + repeat));
            }
        }
        let group = group(frames);
        let mut plan = crate::calibration::CalibrationPlan::without_calibration(group.frames.len());
        plan.assignments = group
            .frames
            .iter()
            .map(|frame| (frame.target_id - 1) as usize)
            .collect();
        plan.sessions = (0..4)
            .map(|panel| crate::calibration::CalibrationSession {
                masters: CalibrationMasters::new(
                    Some(
                        LinearImage::new(
                            width,
                            height,
                            1,
                            vec![panel as f32 * 100.0; width * height],
                        )
                        .unwrap(),
                    ),
                    None,
                    None,
                )
                .unwrap(),
                applied: Default::default(),
                match_tolerances: Default::default(),
            })
            .collect();
        let result =
            integrate_with_plan(&group, scratch.path(), &plan, Some(NormalizationMode::None));
        let mut sky = result
            .image
            .data
            .iter()
            .copied()
            .filter(|sample| sample.is_finite())
            .collect::<Vec<_>>();
        sky.sort_by(f32::total_cmp);
        assert!(
            (sky[sky.len() / 10] - 100.0).abs() < 1.0,
            "each panel must subtract its own bias"
        );
        assert!(
            result.image.width >= 1140 && result.image.height >= 830,
            "{} × {}",
            result.image.width,
            result.image.height
        );
        assert!(result.coverage.iter().filter(|&&n| n > 0).count() > 900_000);
        assert!(
            !std::fs::read_dir(scratch.path()).unwrap().any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("mosaic-reference-"))
        );
    }

    /// A local regression using all four Heart and Soul panels. Input paths
    /// are explicit; this never opens a catalog or changes source images.
    #[test]
    #[ignore = "requires PSF_GUARD_MOSAIC_INPUTS: JSON array of four panel FITS paths"]
    fn heart_and_soul_real_panels() {
        let paths: Vec<PathBuf> =
            serde_json::from_str(&std::env::var("PSF_GUARD_MOSAIC_INPUTS").unwrap()).unwrap();
        assert_eq!(paths.len(), 4);
        let scratch = tempfile::tempdir().unwrap();
        let paths = if let Ok(factor) = std::env::var("PSF_GUARD_MOSAIC_INPUT_BIN") {
            let factor: u32 = factor.parse().unwrap();
            paths
                .into_iter()
                .enumerate()
                .map(|(index, path)| {
                    let frame = crate::image_io::open_linear_frame(path)
                        .unwrap()
                        .binned(factor)
                        .unwrap();
                    let output = scratch.path().join(format!("panel-{index}.fits"));
                    seiza_stacking::write_processed_image_fits_f32(
                        &output,
                        &frame.image,
                        &frame.headers,
                        &[],
                    )
                    .unwrap();
                    output
                })
                .collect()
        } else {
            paths
        };
        let group = group(
            paths
                .into_iter()
                .enumerate()
                .map(|(index, path)| frame(path, index as i32 + 1, index as i32))
                .collect(),
        );
        let sensor = crate::image_io::open_linear_frame(&group.frames[0].path)
            .unwrap()
            .image;
        let result = integrate(&group, scratch.path());
        assert!(result.image.width > sensor.width && result.image.height > sensor.height);
        if let Ok(destination) = std::env::var("PSF_GUARD_MOSAIC_OUTPUT") {
            let destination = PathBuf::from(destination);
            std::fs::create_dir_all(&destination).unwrap();
            seiza_stacking::write_processed_image_fits_f32(
                destination.join("mosaic.fits"),
                &result.image,
                &[],
                &[],
            )
            .unwrap();
            stretch::render_image_preview_atomic(
                &result.image,
                &stretch::default_linear_config(),
                stretch::StackStretchSourceTransfer::Linear,
                &destination.join("mosaic.png"),
            )
            .unwrap();
        }
        eprintln!(
            "Heart and Soul: {} × {}, {} frames",
            result.image.width, result.image.height, result.accepted_frames
        );
    }
}
