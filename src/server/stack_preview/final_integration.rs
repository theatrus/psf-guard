//! Revisit admitted frames without repeating their registration decisions.

use super::{resume, PreparedGroup};
use seiza_stacking::{
    BatchStackOptions, BatchStackPass, BatchStackResult, CalibrationMasters, FitsFrame,
    ImpulseFilterOptions, LinearImage, ReferenceRegion, RegisteredFrameMapping,
};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub(super) fn integrate(
    group: &PreparedGroup,
    ledger: &[resume::ResumeFrame],
    plan: &crate::calibration::CalibrationPlan,
    cosmetic: Option<ImpulseFilterOptions>,
    cancel: &Arc<AtomicBool>,
    mut progress: impl FnMut(BatchStackPass, usize, usize),
) -> seiza_stacking::Result<BatchStackResult> {
    let admitted = ledger
        .iter()
        .enumerate()
        .filter(|(_, frame)| {
            matches!(
                frame.decision.disposition.as_str(),
                "reference" | "accepted"
            )
        })
        .collect::<Vec<_>>();
    let options = BatchStackOptions {
        cancel: Some(Arc::clone(cancel).into()),
        ..BatchStackOptions::default()
    };
    seiza_stacking::integrate_registered_frames(admitted.len(), &options, |pass, index| {
        progress(pass, index, admitted.len());
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
        let masters = (!record.calibration_bypassed)
            .then_some(&plan.sessions[plan.assignments[source_index]].masters);
        let image = prepare_registered_frame(frame, masters, cosmetic, mapping)?;
        if super::source_fingerprint(&source.path) != source.source_fingerprint {
            return Err(seiza_stacking::Error::Stack(format!(
                "Image {} changed while being read; rebuild the stack",
                source.image_id
            )));
        }
        Ok(image)
    })
}

fn prepare_registered_frame(
    mut frame: FitsFrame,
    masters: Option<&CalibrationMasters>,
    cosmetic: Option<ImpulseFilterOptions>,
    mapping: &RegisteredFrameMapping,
) -> seiza_stacking::Result<LinearImage> {
    if let Some(masters) = masters {
        masters.validate_light_frame(&frame)?;
        masters.apply(&mut frame.image, frame.exposure_seconds, frame.bayer)?;
    }
    if let Some(cosmetic) = cosmetic {
        seiza_stacking::suppress_impulses(&mut frame.image, frame.bayer, &cosmetic)?;
    }
    let frame = frame.into_prepared()?;
    mapping.extract_region(
        &frame.image,
        ReferenceRegion {
            x: 0,
            y: 0,
            width: mapping.reference_width(),
            height: mapping.reference_height(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use seiza_stacking::{
        BayerLayout, FrameDisposition, LiveStacker, NormalizationMap, NormalizationMode,
        RejectionMode, SimilarityTransform, StackOptions,
    };
    use std::sync::atomic::Ordering;

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
            Some(ImpulseFilterOptions::default()),
            &mapping,
        )
        .unwrap();
        assert!(corrected.data.iter().all(|&value| value == 90.0));
        let calibrated =
            prepare_registered_frame(frame(raw.clone()), Some(&masters), None, &mapping).unwrap();
        assert_eq!(calibrated.data[40], 7990.0);
        let bypassed = prepare_registered_frame(frame(raw.clone()), None, None, &mapping).unwrap();
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
        let registered = prepare_registered_frame(frame(raw), None, None, &mapping).unwrap();
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
            cosmetic,
            &live.reference_mapping(),
        )
        .unwrap();
        assert_eq!(prepared.channels, 3);
        assert_eq!(prepared.data, live.snapshot().unwrap().image.data);
        let uncorrected =
            prepare_registered_frame(reference, Some(&masters), None, &live.reference_mapping())
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
        let registered =
            prepare_registered_frame(source, Some(&masters), cosmetic, &diagnostics.mapping)
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
        let mut frames = Vec::new();
        let mut ledger = Vec::new();
        for index in 0..5 {
            let mut pixels = image(1000.0);
            if index == 0 {
                pixels.data[40] += 10_000.0;
            }
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
            &plan,
            None,
            &Arc::new(AtomicBool::new(false)),
            |pass, index, count| progress.push((pass, index, count)),
        )
        .unwrap();
        assert_eq!(result.snapshot.accepted_frames, 4);
        assert_eq!(result.snapshot.image.data, vec![1000.0; 81]);
        assert_eq!(result.snapshot.coverage[40], 3);
        assert_eq!(result.frames[0].integrated_samples, 80);
        assert_eq!(progress.len(), 8);
        assert!(progress.iter().all(|(_, _, count)| *count == 4));
    }

    #[test]
    fn replay_refuses_changed_files_and_honors_cancellation() {
        let directory = tempfile::tempdir().unwrap();
        let (mut group, ledger) = fixture(directory.path());
        let plan = crate::calibration::CalibrationPlan::without_calibration(group.frames.len());
        let cancel = Arc::new(AtomicBool::new(false));
        group.frames[0].source_fingerprint = "changed".into();
        let error = integrate(&group, &ledger, &plan, None, &cancel, |_, _, _| {}).unwrap_err();
        assert!(error.to_string().contains("changed during stacking"));
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(
            integrate(&group, &ledger, &plan, None, &cancel, |_, _, _| {}),
            Err(seiza_stacking::Error::Cancelled)
        ));
    }
}
