//! Revisit admitted frames without repeating their registration decisions.
//!
//! Each admitted frame is read, calibrated and resampled once, into a scratch
//! file below the stack root, and Seiza's three rejection passes then read the
//! files back band by band. Loading a frame for every pass instead prepared
//! it three times over, and held 40 bytes of statistics per sample against 16
//! to 20: on 45 frames of a 26 MP mono camera, 23 s and 9.9 GB at peak against
//! 9 s and 5.7 GB. The scratch files take four bytes per sample per frame and
//! go when the pass ends; the janitor sweeps any a crash left behind.

use super::{resume, PreparedGroup, SessionCosmetics};
use crate::concurrency::Stop;
use seiza_calibration::MatchTolerances;
use seiza_stacking::{
    BatchStackOptions, BatchStackPass, BatchStackResult, CalibrationMasters, FitsFrame,
    ImpulseFilterOptions, LinearImage, ReferenceRegion, RegisteredFrameMapping,
};

/// Integrate the admitted frames again with Seiza's three-pass rejection.
///
/// With `scratch`, each frame is loaded once into a scratch file there and
/// the passes read the files back band by band. Without it, or when the
/// scratch files cannot be kept, as when the disk fills, each frame is
/// loaded again for each pass instead: slower and in more memory, but the
/// same stack to the bit.
#[allow(clippy::too_many_arguments)]
pub(super) fn integrate(
    group: &PreparedGroup,
    ledger: &[resume::ResumeFrame],
    method: super::StackMethod,
    plan: &crate::calibration::CalibrationPlan,
    cosmetics: &SessionCosmetics,
    scratch: Option<&std::path::Path>,
    cancel: &Stop,
    mut progress: impl FnMut(BatchStackPass, usize, usize),
) -> seiza_stacking::Result<BatchStackResult> {
    let admitted = ledger
        .iter()
        .enumerate()
        .filter(|(_, frame)| frame.admitted())
        .collect::<Vec<_>>();
    let options = BatchStackOptions {
        cancel: Some(cancel.clone().into()),
        frame_weights: replay_weights(method.weighting, &admitted)?,
        ..BatchStackOptions::default()
    };
    let load = |index: usize| {
        let (source_index, record) = admitted[index];
        let source = &group.frames[source_index];
        if super::source_fingerprint(&source.path) != source.source_fingerprint {
            return Err(seiza_stacking::Error::Stack(format!(
                "Image {} changed during stacking; rebuild the stack",
                source.image_id
            )));
        }
        let frame = crate::image_io::open_linear_frame(&source.path)
            .map_err(|error| seiza_stacking::Error::Stack(error.to_string()))?;
        let mapping = record.decision.registered_mapping.as_ref().ok_or_else(|| {
            seiza_stacking::Error::Stack("An admitted frame has no registered mapping".into())
        })?;
        let session = plan.assignments[source_index];
        let masters = (!record.calibration_bypassed).then_some(&plan.sessions[session].masters);
        let cosmetic = cosmetics.for_frame(session, record.calibration_bypassed);
        let image = prepare_registered_frame(
            frame,
            masters,
            &plan.sessions[session].match_tolerances,
            cosmetic,
            mapping,
            method.seiza_interpolation(),
        )?;
        if super::source_fingerprint(&source.path) != source.source_fingerprint {
            return Err(seiza_stacking::Error::Stack(format!(
                "Image {} changed while being read; rebuild the stack",
                source.image_id
            )));
        }
        Ok(image)
    };
    if let Some(scratch) = scratch {
        let banded = std::fs::create_dir_all(scratch)
            .map_err(|error| {
                seiza_stacking::Error::Stack(format!(
                    "Cannot make the final pass's scratch folder {}: {error}",
                    scratch.display()
                ))
            })
            .and_then(|()| {
                let options = BatchStackOptions {
                    scratch_directory: Some(scratch.to_path_buf()),
                    ..options.clone()
                };
                seiza_stacking::integrate_registered_frames_banded(
                    admitted.len(),
                    &options,
                    load,
                    &mut progress,
                )
            });
        match banded {
            Err(error) if is_scratch_failure(&error, scratch) => {
                tracing::warn!("Final pass reading every frame on each pass instead: {error}")
            }
            result => return result,
        }
    }
    seiza_stacking::integrate_registered_frames(admitted.len(), &options, |pass, index| {
        progress(pass, index, admitted.len());
        load(index)
    })
}

/// Whether `error` says the scratch files in `scratch` could not be kept,
/// which the whole-frame passes get by without, rather than that a frame
/// could not be read or the pass was stopped: Seiza's own words when it
/// cannot make its directory or keep a frame there, PSF Guard's when it
/// cannot make the folder, and a frame that could not be read back from
/// below `scratch`.
fn is_scratch_failure(error: &seiza_stacking::Error, scratch: &std::path::Path) -> bool {
    if matches!(error, seiza_stacking::Error::Cancelled) {
        return false;
    }
    let message = error.to_string();
    message.contains("scratch directory")
        || message.contains("final pass's scratch folder")
        || (message.contains(" back from ") && message.contains(&*scratch.to_string_lossy()))
}

/// The weights the live pass gave each admitted frame, in replay order, or
/// `None` for an equal-weight stack. Seiza measured them once; replaying with
/// anything else would publish a different stack from the one the live pass
/// admitted. A weighted ledger that lacks one is refused rather than mixed.
fn replay_weights(
    weighting: super::StackWeighting,
    admitted: &[(usize, &resume::ResumeFrame)],
) -> seiza_stacking::Result<Option<Vec<Vec<f32>>>> {
    if weighting.is_equal() {
        return Ok(None);
    }
    admitted
        .iter()
        .map(|(_, frame)| {
            frame.decision.integration_weight.clone().ok_or_else(|| {
                seiza_stacking::Error::Stack(format!(
                    "Image {} has no recorded frame weight; rebuild the stack",
                    frame.decision.image_id
                ))
            })
        })
        .collect::<seiza_stacking::Result<Vec<_>>>()
        .map(Some)
}

fn prepare_registered_frame(
    mut frame: FitsFrame,
    masters: Option<&CalibrationMasters>,
    gate: &MatchTolerances,
    cosmetic: Option<ImpulseFilterOptions>,
    mapping: &RegisteredFrameMapping,
    interpolation: seiza_stacking::Interpolation,
) -> seiza_stacking::Result<LinearImage> {
    if let Some(masters) = masters {
        // Under the gate its session's lights were admitted under: their
        // rig's tolerances, the ones its masters were matched by.
        masters.validate_light_frame_with(&frame, gate)?;
        masters.apply(&mut frame.image, frame.exposure_seconds, frame.bayer)?;
    }
    if let Some(cosmetic) = cosmetic {
        seiza_stacking::suppress_impulses(&mut frame.image, frame.bayer, &cosmetic)?;
    }
    let frame = frame.into_prepared()?;
    // Through the recorded warp, when registration fitted one, and with the
    // live pass's resampling.
    mapping.extract_region_with(
        &frame.image,
        ReferenceRegion {
            x: 0,
            y: 0,
            width: mapping.reference_width(),
            height: mapping.reference_height(),
        },
        interpolation,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::stack_preview::{StackMethod, StackWeighting};
    use seiza_stacking::Interpolation;
    use seiza_stacking::{
        BayerLayout, FrameDisposition, LiveStacker, NormalizationMap, NormalizationMode,
        RejectionMode, SimilarityTransform, StackOptions,
    };

    fn image(value: f32) -> LinearImage {
        LinearImage::new(9, 9, 1, vec![value; 81]).unwrap()
    }

    fn frame(image: LinearImage) -> FitsFrame {
        FitsFrame {
            image,
            headers: Vec::new(),
            exposure_seconds: Some(60.0),
            bayer: None,
            source: None,
            bounds: None,
        }
    }

    #[test]
    fn replay_applies_calibration_then_cosmetic_and_respects_raw_fallback() {
        let masters = CalibrationMasters::new(Some(image(10.0)), None, None).unwrap();
        let mut raw = image(100.0);
        raw.data[40] = 8000.0;
        let mapping = RegisteredFrameMapping::identity(&raw);
        let corrected = prepare_registered_frame(
            frame(raw.clone()),
            Some(&masters),
            &MatchTolerances::default(),
            Some(ImpulseFilterOptions::default()),
            &mapping,
            Interpolation::Bilinear,
        )
        .unwrap();
        assert!(corrected.data.iter().all(|&value| value == 90.0));
        let calibrated = prepare_registered_frame(
            frame(raw.clone()),
            Some(&masters),
            &MatchTolerances::default(),
            None,
            &mapping,
            Interpolation::Bilinear,
        )
        .unwrap();
        assert_eq!(calibrated.data[40], 7990.0);
        let bypassed = prepare_registered_frame(
            frame(raw.clone()),
            None,
            &MatchTolerances::default(),
            None,
            &mapping,
            Interpolation::Bilinear,
        )
        .unwrap();
        assert_eq!(bypassed.data, raw.data);
    }

    #[test]
    fn replay_preserves_missing_registration_borders() {
        let raw = image(100.0);
        let mapping = RegisteredFrameMapping::new(
            9,
            9,
            SimilarityTransform {
                translation_x: 2.0,
                ..SimilarityTransform::IDENTITY
            },
            NormalizationMap::identity(&raw),
        )
        .unwrap();
        let registered = prepare_registered_frame(
            frame(raw),
            None,
            &MatchTolerances::default(),
            None,
            &mapping,
            Interpolation::Bilinear,
        )
        .unwrap();
        assert!(registered
            .data
            .chunks(9)
            .all(|row| row[..2].iter().all(|sample| sample.is_nan())));
        assert!(registered
            .data
            .chunks(9)
            .all(|row| row[2..].iter().all(|&sample| sample == 100.0)));
    }

    #[test]
    fn cfa_replay_matches_live_calibration_cosmetics_and_global_normalization() {
        let (width, height) = (160, 128);
        let bias = LinearImage::new(
            width,
            height,
            1,
            (0..width * height)
                .map(|index| 50.0 + (index % width % 5) as f32 * 3.0 + (index / width % 7) as f32)
                .collect(),
        )
        .unwrap();
        let make_raw = |gain: f32, offset: f32, shift: (i32, i32)| {
            let positions = [
                (19.7_f32, 16.4_f32),
                (71.3, 28.1),
                (132.2, 34.8),
                (43.1, 49.7),
                (103.4, 58.3),
                (22.8, 70.2),
                (82.7, 76.5),
                (143.1, 87.8),
                (54.4, 96.2),
                (116.8, 104.1),
                (31.2, 113.0),
                (91.5, 118.4),
            ];
            let mut pixels = Vec::with_capacity(width * height);
            for y in 0..height {
                for x in 0..width {
                    let scene_x = x as i32 - shift.0;
                    let scene_y = y as i32 - shift.1;
                    let noise = (scene_x * 17 + scene_y * 31).rem_euclid(23) as f32 * 0.5;
                    let mut value = 1000.0 + noise;
                    for (index, &(star_x, star_y)) in positions.iter().enumerate() {
                        let dx = scene_x as f32 - star_x;
                        let dy = scene_y as f32 - star_y;
                        value += (9000.0 + index as f32 * 1300.0)
                            * (-dx.mul_add(dx, dy * dy) / 8.0).exp();
                    }
                    let color_gain = match (x % 2, y % 2) {
                        (0, 0) => 1.3,
                        (1, 1) => 0.7,
                        _ => 1.0,
                    };
                    pixels.push(gain * color_gain * value + offset + bias.data[y * width + x]);
                }
            }
            pixels[64 * width + 80] += 100_000.0;
            FitsFrame {
                bayer: Some(BayerLayout {
                    pattern: seiza_fits::BayerPattern::Rggb,
                    x_offset: 0,
                    y_offset: 0,
                }),
                ..frame(LinearImage::new(width, height, 1, pixels).unwrap())
            }
        };
        let reference = make_raw(1.0, 0.0, (0, 0));
        let source = make_raw(1.6, 40.0, (2, -2));
        let masters = CalibrationMasters::new(Some(bias), None, None).unwrap();
        let cosmetic = Some(ImpulseFilterOptions::default());
        let mut live = LiveStacker::new(
            reference.clone(),
            masters.clone(),
            StackOptions {
                normalization: NormalizationMode::Global,
                cosmetic,
                rejection: RejectionMode::None,
                ..StackOptions::default()
            },
        )
        .unwrap();
        let prepared = prepare_registered_frame(
            reference.clone(),
            Some(&masters),
            &MatchTolerances::default(),
            cosmetic,
            &live.reference_mapping(),
            Interpolation::Bilinear,
        )
        .unwrap();
        assert_eq!(prepared.channels, 3);
        assert_eq!(prepared.data, live.snapshot().unwrap().image.data);
        let uncorrected = prepare_registered_frame(
            reference,
            Some(&masters),
            &MatchTolerances::default(),
            None,
            &live.reference_mapping(),
            Interpolation::Bilinear,
        )
        .unwrap();
        assert!(
            uncorrected
                .data
                .iter()
                .zip(&prepared.data)
                .any(|(raw, corrected)| raw - corrected > 10_000.0),
            "cosmetics must remove the injected hot pixel"
        );

        let FrameDisposition::Accepted(diagnostics) = live.push(source.clone()).unwrap() else {
            panic!("the translated CFA field must be admitted");
        };
        assert!(diagnostics.transform.translation_x.abs() > 1.0);
        assert!((diagnostics.normalization_mean_gain - 1.0).abs() > 0.1);
        assert!(diagnostics.normalization_mean_offset.abs() > 10.0);
        let registered = prepare_registered_frame(
            source,
            Some(&masters),
            &MatchTolerances::default(),
            cosmetic,
            &diagnostics.mapping,
            Interpolation::Bilinear,
        )
        .unwrap();
        assert!(registered.data.iter().any(|value| value.is_nan()));
        let snapshot = live.snapshot().unwrap();
        for (index, (&reference, &source)) in prepared.data.iter().zip(&registered.data).enumerate()
        {
            let (mean, coverage) = if source.is_finite() {
                (reference + (source - reference) / 2.0, 2)
            } else {
                (reference, 1)
            };
            assert_eq!(snapshot.coverage[index], coverage, "sample {index}");
            assert_eq!(snapshot.image.data[index], mean, "sample {index}");
        }
    }

    fn fixture(directory: &std::path::Path) -> (PreparedGroup, Vec<resume::ResumeFrame>) {
        fixture_with(directory, |index| {
            let mut pixels = image(1000.0);
            if index == 0 {
                pixels.data[40] += 10_000.0;
            }
            pixels
        })
    }

    /// Five frames, the second excluded; `pixels` draws each one.
    fn fixture_with(
        directory: &std::path::Path,
        pixels: impl Fn(i32) -> LinearImage,
    ) -> (PreparedGroup, Vec<resume::ResumeFrame>) {
        let mut frames = Vec::new();
        let mut ledger = Vec::new();
        for index in 0..5 {
            let pixels = pixels(index);
            let path = directory.join(format!("{index}.fits"));
            seiza_stacking::write_processed_image_fits_f32(&path, &pixels, &[], &[]).unwrap();
            let source = super::super::PreparedFrame {
                image_id: index,
                acquired_date: None,
                quality_score: None,
                source_fingerprint: super::super::source_fingerprint(&path),
                path,
                expected_target: None,
                exposure_seconds: 60.0,
            };
            let mut decision = super::super::rejected_decision(&source, "Excluded fixture".into());
            if index != 1 {
                decision.disposition = if index == 0 { "reference" } else { "accepted" }.into();
                decision.reason = None;
                decision.registered_mapping = Some(RegisteredFrameMapping::identity(&pixels));
            }
            ledger.push(resume::ResumeFrame {
                decision,
                exposure_seconds: 60.0,
                calibration_bypassed: false,
                retryable_failure: false,
                rotation_radians: (index != 1).then_some(0.0),
            });
            frames.push(source);
        }
        (
            PreparedGroup {
                flat_star_masking: false,
                index: 0,
                calibration: crate::calibration::CalibrationMode::Off,
                frames,
            },
            ledger,
        )
    }

    #[test]
    fn final_pass_removes_reference_trail_and_replays_only_admitted_frames() {
        let directory = tempfile::tempdir().unwrap();
        let (group, ledger) = fixture(directory.path());
        let plan = crate::calibration::CalibrationPlan::without_calibration(group.frames.len());
        let mut progress = Vec::new();
        let result = integrate(
            &group,
            &ledger,
            StackMethod::classic(),
            &plan,
            &SessionCosmetics::none(plan.sessions.len()),
            Some(&directory.path().join("scratch")),
            &crate::concurrency::Stop::new(),
            |pass, index, count| progress.push((pass, index, count)),
        )
        .unwrap();
        assert_eq!(result.snapshot.accepted_frames, 4);
        assert_eq!(result.snapshot.image.data, vec![1000.0; 81]);
        assert_eq!(result.snapshot.coverage[40], 3);
        assert_eq!(result.frames[0].integrated_samples, 80);
        // Each admitted frame is read once, then integrated, and the scratch
        // folder is left as it was found.
        let expected = [BatchStackPass::Estimate, BatchStackPass::Integrate]
            .into_iter()
            .flat_map(|pass| (0..4).map(move |index| (pass, index, 4)))
            .collect::<Vec<_>>();
        assert_eq!(progress, expected);
        let scratch = std::fs::read_dir(directory.path().join("scratch")).unwrap();
        assert_eq!(scratch.count(), 0);
    }

    /// Without a scratch folder, or with one that cannot be made, the pass
    /// reads every frame on each of three passes instead, to the same stack.
    #[test]
    fn a_final_pass_without_scratch_reads_each_frame_per_pass_to_the_same_stack() {
        let directory = tempfile::tempdir().unwrap();
        let (group, ledger) = fixture(directory.path());
        let plan = crate::calibration::CalibrationPlan::without_calibration(group.frames.len());
        let cosmetics = SessionCosmetics::none(plan.sessions.len());
        let run = |scratch: Option<&std::path::Path>| {
            let mut progress = Vec::new();
            let result = integrate(
                &group,
                &ledger,
                StackMethod::classic(),
                &plan,
                &cosmetics,
                scratch,
                &crate::concurrency::Stop::new(),
                |pass, index, count| progress.push((pass, index, count)),
            )
            .unwrap();
            (result, progress)
        };
        let (banded, _) = run(Some(&directory.path().join("scratch")));
        let three_passes = [
            BatchStackPass::Estimate,
            BatchStackPass::Refine,
            BatchStackPass::Integrate,
        ]
        .into_iter()
        .flat_map(|pass| (0..4).map(move |index| (pass, index, 4)))
        .collect::<Vec<_>>();
        // A file where the folder should be: making it fails.
        let blocked = directory.path().join("blocked");
        std::fs::write(&blocked, b"not a folder").unwrap();
        for scratch in [None, Some(blocked.as_path())] {
            let (whole, progress) = run(scratch);
            assert_eq!(progress, three_passes);
            let bits = |result: &BatchStackResult| {
                result
                    .snapshot
                    .image
                    .data
                    .iter()
                    .map(|value| value.to_bits())
                    .collect::<Vec<_>>()
            };
            assert_eq!(bits(&whole), bits(&banded));
            assert_eq!(whole.snapshot.coverage, banded.snapshot.coverage);
        }
    }

    /// Only the scratch files' own failures send the pass to its three
    /// loads per frame; a source that cannot be read stays an error even
    /// when its path says scratch.
    #[test]
    fn only_scratch_failures_fall_back_to_three_passes() {
        let scratch = std::path::Path::new("/stacks/stack-scratch");
        let error = |message: &str| seiza_stacking::Error::Stack(message.into());
        for message in [
            "cannot initialize replay scratch directory /stacks/stack-scratch (an existing writable directory is required): No space left on device",
            "cannot keep registered frame 3 in the chosen scratch directory",
            "cannot read admitted frame 2 back from /stacks/stack-scratch/.seiza-reintegrate-41-Ab3/frame-1.f32: unexpected end of file",
            "Cannot make the final pass's scratch folder /stacks/stack-scratch: Permission denied",
        ] {
            assert!(is_scratch_failure(&error(message), scratch), "{message}");
        }
        for message in [
            "Image 7 changed during stacking; rebuild the stack",
            "cannot read /Volumes/scratch/lights/m31-0001.fits: unexpected end of file",
        ] {
            assert!(!is_scratch_failure(&error(message), scratch), "{message}");
        }
        assert!(!is_scratch_failure(
            &seiza_stacking::Error::Cancelled,
            scratch
        ));
    }

    #[test]
    fn replay_refuses_changed_files_and_honors_cancellation() {
        let directory = tempfile::tempdir().unwrap();
        let (mut group, ledger) = fixture(directory.path());
        let plan = crate::calibration::CalibrationPlan::without_calibration(group.frames.len());
        let cancel = crate::concurrency::Stop::new();
        group.frames[0].source_fingerprint = "changed".into();
        let none = SessionCosmetics::none(plan.sessions.len());
        let error = integrate(
            &group,
            &ledger,
            StackMethod::classic(),
            &plan,
            &none,
            Some(&directory.path().join("scratch")),
            &cancel,
            |_, _, _| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("changed during stacking"));
        cancel.request();
        assert!(matches!(
            integrate(
                &group,
                &ledger,
                StackMethod::classic(),
                &plan,
                &none,
                Some(&directory.path().join("scratch")),
                &cancel,
                |_, _, _| {}
            ),
            Err(seiza_stacking::Error::Cancelled)
        ));
    }

    #[test]
    fn a_weighted_final_pass_replays_the_weights_the_live_pass_recorded() {
        let directory = tempfile::tempdir().unwrap();
        // Frame 1 is excluded, so the admitted frames are 0, 2, 3, and 4.
        let (group, mut ledger) =
            fixture_with(directory.path(), |index| image(100.0 + index as f32));
        for (frame, weight) in ledger.iter_mut().zip([1.0, 9.0, 2.0, 3.0, 4.0]) {
            if frame.admitted() {
                frame.decision.integration_weight = Some(vec![weight]);
            }
        }
        // The weights reach Seiza through the checkpoint manifest, so they
        // must survive its round trip.
        let ledger: Vec<resume::ResumeFrame> =
            serde_json::from_slice(&serde_json::to_vec(&ledger).unwrap()).unwrap();
        let plan = crate::calibration::CalibrationPlan::without_calibration(group.frames.len());
        let none = SessionCosmetics::none(plan.sessions.len());
        let cancel = crate::concurrency::Stop::new();
        let weighted = integrate(
            &group,
            &ledger,
            StackMethod {
                weighting: StackWeighting::Noise,
                ..StackMethod::classic()
            },
            &plan,
            &none,
            Some(&directory.path().join("scratch")),
            &cancel,
            |_, _, _| {},
        )
        .unwrap();
        // (100·1 + 102·2 + 103·3 + 104·4) / 10, not the plain mean 102.25.
        let expected = (100.0 + 102.0 * 2.0 + 103.0 * 3.0 + 104.0 * 4.0) / 10.0;
        assert_eq!(weighted.snapshot.accepted_frames, 4);
        assert!(weighted
            .snapshot
            .image
            .data
            .iter()
            .all(|&value| (value - expected).abs() < 1.0e-3));
        let equal = integrate(
            &group,
            &ledger,
            StackMethod::classic(),
            &plan,
            &none,
            Some(&directory.path().join("scratch")),
            &cancel,
            |_, _, _| {},
        )
        .unwrap();
        assert!(equal
            .snapshot
            .image
            .data
            .iter()
            .all(|&value| (value - 102.25).abs() < 1.0e-3));
    }

    #[test]
    fn a_weighted_final_pass_refuses_a_frame_without_a_weight() {
        let directory = tempfile::tempdir().unwrap();
        let (group, mut ledger) = fixture(directory.path());
        for frame in ledger.iter_mut().filter(|frame| frame.admitted()) {
            frame.decision.integration_weight = Some(vec![1.0]);
        }
        ledger[3].decision.integration_weight = None;
        let plan = crate::calibration::CalibrationPlan::without_calibration(group.frames.len());
        let error = integrate(
            &group,
            &ledger,
            StackMethod {
                weighting: StackWeighting::Noise,
                ..StackMethod::classic()
            },
            &plan,
            &SessionCosmetics::none(plan.sessions.len()),
            Some(&directory.path().join("scratch")),
            &crate::concurrency::Stop::new(),
            |_, _, _| {},
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("Image 3 has no recorded frame weight"));
    }

    /// A flat 3 mm off the light, matched because its rig allows 5 mm, must
    /// calibrate the light in the final pass too, not only in matching.
    #[test]
    fn a_flat_matched_by_a_wider_focal_length_tolerance_stacks_through_the_final_pass() {
        use crate::calibration::tests::RulesInForce;
        use std::io::Write as _;
        fn write(path: &std::path::Path, kind: &str, focal_mm: u32, value: i16) {
            let mut header = Vec::new();
            for card in [
                "SIMPLE  =                    T".to_string(),
                "BITPIX  =                   16".to_string(),
                "NAXIS   =                    2".to_string(),
                "NAXIS1  =                    4".to_string(),
                "NAXIS2  =                    4".to_string(),
                format!("IMAGETYP= '{kind}'"),
                "FILTER  = 'Ha'".to_string(),
                "EXPTIME =                300.0".to_string(),
                "GAIN    =                  100".to_string(),
                "OFFSET  =                   20".to_string(),
                "XBINNING=                    1".to_string(),
                "YBINNING=                    1".to_string(),
                "CCD-TEMP=                -10.0".to_string(),
                "TELESCOP= 'Scope'".to_string(),
                "INSTRUME= 'Camera'".to_string(),
                format!("FOCALLEN= {focal_mm:20}"),
                "END".to_string(),
            ] {
                let mut bytes = card.into_bytes();
                bytes.resize(80, b' ');
                header.extend(bytes);
            }
            header.resize(header.len().div_ceil(2880) * 2880, b' ');
            let mut payload = Vec::new();
            for _ in 0..16 {
                payload.extend(value.to_be_bytes());
            }
            payload.resize(2880, 0);
            let mut file = std::fs::File::create(path).unwrap();
            file.write_all(&header).unwrap();
            file.write_all(&payload).unwrap();
        }

        let temp = tempfile::tempdir().unwrap();
        // Bias, so a flat master is built at all, and flats 3 mm off the light.
        let mut flats = Vec::new();
        for index in 0..2 {
            let path = temp.path().join(format!("bias-{index}.fits"));
            write(&path, "BIAS", 600, 100);
            flats.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        for index in 0..3 {
            let path = temp.path().join(format!("flat-{index}.fits"));
            write(&path, "FLAT", 603, 1_000);
            flats.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        let light = temp.path().join("light.fits");
        write(&light, "LIGHT", 600, 1_100);
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            crate::calibration::import_calibration_frames(&tx, &flats, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        let rig = crate::calibration::rules::light_rig_uuid(
            &crate::commands::import::headers::read_frame_meta(&light),
        );
        let _rules = RulesInForce::with(
            &[],
            &[(
                rig.as_str(),
                "Scope · Camera",
                &[("focal_length_tolerance_mm", serde_json::json!(5))],
            )],
        );

        let (masters, applied) = crate::calibration::resolve_or_build_masters(
            &conn,
            &temp.path().join("masters"),
            std::slice::from_ref(&light),
            None,
            None,
            crate::calibration::CalibrationMode::Auto,
        )
        .unwrap();
        assert_eq!(applied.state, "applied", "{applied:?}");
        assert!(applied.flat_master.is_some(), "{applied:?}");

        // The gate is the light's rig's: the tolerances its flat was matched by.
        let gate = crate::calibration::light_match_tolerances_at(&light);
        assert_eq!(gate.focal_length_mm, 5.0);
        let replay = |gate: &MatchTolerances| {
            let frame = crate::image_io::open_linear_frame(&light).unwrap();
            let raw = frame.image.clone();
            prepare_registered_frame(
                frame,
                Some(&masters),
                gate,
                None,
                &RegisteredFrameMapping::identity(&raw),
                Interpolation::Bilinear,
            )
        };
        replay(&gate).expect("the final pass calibrates the light with the flat its rig matched");
        // Seiza's default gate would refuse it, which is what the gate is for.
        let refused = replay(&MatchTolerances::default()).unwrap_err().to_string();
        assert!(refused.contains("focal length"), "{refused}");

        // The live pass judges the light as its reference under the same
        // gate: the masters pass, and only this tiny starless frame's
        // registration stops it. Under the default gate the masters are
        // refused first.
        let open = |gate: MatchTolerances| {
            LiveStacker::new(
                crate::image_io::open_linear_frame(&light).unwrap(),
                masters.clone(),
                StackOptions {
                    match_tolerances: gate,
                    ..StackOptions::default()
                },
            )
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default()
        };
        let opened = open(gate);
        assert!(!opened.contains("focal length"), "{opened}");
        assert!(opened.contains("usable stars"), "{opened}");
        let refused = open(MatchTolerances::default());
        assert!(refused.contains("focal length"), "{refused}");
    }

    /// A light before new dust whose only same-side flats are 110.6° off,
    /// in its response family because the rig's flats at the two angles
    /// measure alike, must stack through the live and final passes: its
    /// session's gate reads the light at its flats' angle, and nothing else
    /// changes.
    #[test]
    fn flats_of_the_lights_measured_family_stack_through_the_final_pass() {
        use crate::calibration::test_frames::write_frame_with;
        use seiza_calibration::LightRotation;
        let temp = tempfile::tempdir().unwrap();
        let angle = |degrees: f64| format!("ROTATANG= {degrees:20.1}");
        let mut frames = Vec::new();
        let write = |name: &str, kind: &str, captured: &str, degrees: f64, value: f64| {
            let path = temp.path().join(name);
            let card = angle(degrees);
            write_frame_with(&path, kind, (32, 32), captured, &[card.as_str()], |_, _| {
                value
            });
            path
        };
        for index in 0..2 {
            frames.push(write(
                &format!("bias-{index}.fits"),
                "BIAS",
                "2026-09-10T13:00:00",
                0.0,
                500.0,
            ));
        }
        for (stem, captured, degrees) in [
            ("before-0", "2026-09-10T14:0", 0.0),
            ("after-0", "2026-09-14T14:0", 0.0),
            ("after-110", "2026-09-14T14:3", 110.6),
            ("other-104", "2026-09-14T14:4", 104.0),
        ] {
            for index in 0..3 {
                frames.push(write(
                    &format!("{stem}-{index}.fits"),
                    "FLAT",
                    &format!("{captured}{index}:00"),
                    degrees,
                    30_000.0,
                ));
            }
        }
        let light = write("light.fits", "LIGHT", "2026-09-09T04:00:00", 110.6, 1_100.0);
        let metas: Vec<_> = frames
            .iter()
            .map(|path| crate::commands::import::headers::read_frame_meta(path))
            .collect();
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        {
            let tx = conn.transaction().unwrap();
            crate::calibration::import_calibration_frames(&tx, &metas, Some("profile")).unwrap();
            tx.commit().unwrap();
        }
        // New dust between the mornings at 0°; the after morning's two
        // angles measure alike, which is the rig's angle term. Flats at
        // 104° measure apart from every other: another family the light's
        // angle could reach.
        crate::calibration::flat_groups::testing::measure_as(&conn, |a, b| {
            if a.rotation == Some(104.0) || b.rotation == Some(104.0) || a.night != b.night {
                0.02
            } else {
                0.0
            }
        });
        let light_meta = crate::commands::import::headers::read_frame_meta(&light);
        // Its own pixels chose the family at its angle: the family's flats
        // at its other angle may serve it.
        crate::calibration::flat_groups::testing::lights_confirm_their_family(&conn, &light_meta);
        let selected = crate::calibration::select_for_light(&conn, &light_meta).unwrap();
        let family = selected.flat_family.as_ref().expect("its measured family");
        assert_eq!(family.label, "Ha at 0.0° & 110.6°");
        assert!(family.by_pixels(), "{family:?}");
        assert_eq!(
            family.light_rotation,
            LightRotation::Measured { degrees: 0.0 }
        );
        assert_eq!(selected.flat_basis.rotation_tolerance(), None);

        // The plan gives the light's session the widened gate.
        let plan = crate::calibration::resolve_or_build_master_plan(
            &conn,
            &temp.path().join("masters"),
            std::slice::from_ref(&light),
            None,
            None,
            crate::calibration::CalibrationMode::Auto,
            &[],
        )
        .unwrap();
        let session = &plan.sessions[plan.assignments[0]];
        assert!(
            session.applied.flat_master.is_some(),
            "{:?}",
            session.applied
        );
        let gate = session.match_tolerances;
        assert_eq!(gate.light_rotation, family.light_rotation);
        assert_eq!(
            MatchTolerances {
                light_rotation: LightRotation::Recorded,
                ..gate
            },
            crate::calibration::light_match_tolerances_at(&light),
            "only how the light's angle is read changes"
        );
        let masters = &session.masters;
        let replay = |path: &std::path::Path, gate: &MatchTolerances| {
            let frame = crate::image_io::open_linear_frame(path).unwrap();
            let raw = frame.image.clone();
            prepare_registered_frame(
                frame,
                Some(masters),
                gate,
                None,
                &RegisteredFrameMapping::identity(&raw),
                Interpolation::Bilinear,
            )
        };
        // The final pass calibrates the light under its session's gate, and
        // Seiza's default gate would refuse it.
        replay(&light, &gate)
            .expect("the final pass calibrates the light with its family's flats at another angle");
        let refused = replay(&light, &MatchTolerances::default())
            .unwrap_err()
            .to_string();
        assert!(refused.contains("rotat"), "{refused}");

        // The live pass judges it under the same gate: the masters pass, and
        // only this tiny starless frame's registration stops it.
        let open = |gate: MatchTolerances| {
            LiveStacker::new(
                crate::image_io::open_linear_frame(&light).unwrap(),
                masters.clone(),
                StackOptions {
                    match_tolerances: gate,
                    ..StackOptions::default()
                },
            )
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default()
        };
        let opened = open(gate);
        assert!(!opened.contains("rotat"), "{opened}");
        assert!(opened.contains("usable stars"), "{opened}");
        assert!(open(MatchTolerances::default()).contains("rotat"));

        // The measured angle is an angle, not a pass: read anywhere else,
        // the light is still refused.
        let elsewhere = MatchTolerances {
            light_rotation: LightRotation::Measured { degrees: 90.0 },
            ..gate
        };
        assert!(replay(&light, &elsewhere).is_err());
    }
}
