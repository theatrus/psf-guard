//! Progressive signal-to-noise analysis over a folder of frames.
//!
//! The server measures the same curve as a side effect of every stack it
//! builds. This is the same measurement without a catalog: point it at a
//! night's lights and it says whether the noise is still falling, how far
//! short of perfect averaging the run is, and what more frames would buy.
//!
//! Frames are stacked raw. Calibration lives in the catalog, and a command
//! that takes a folder has none to match against, so what this reads is the
//! curve of the frames exactly as they were shot.

use crate::server::stack_preview::snr;
use anyhow::{bail, Context, Result};
use seiza_fits::HeaderValue;
use std::path::{Path, PathBuf};

pub struct StackSnrOptions {
    pub order: snr::StackFrameOrder,
    pub json: Option<PathBuf>,
    pub csv: Option<PathBuf>,
    pub detector_threads: Option<usize>,
}

/// One input frame and what decides where it lands in the order.
struct Candidate {
    path: PathBuf,
    exposure_seconds: f64,
    acquired: Option<String>,
    /// Higher is better. Only measured for a quality-ordered run.
    quality: Option<f64>,
}

pub fn stack_snr(paths: &[String], options: &StackSnrOptions) -> Result<()> {
    let files = collect_frames(paths)?;
    if files.len() < 3 {
        bail!(
            "A curve needs at least three frames; found {} under {}",
            files.len(),
            paths.join(", ")
        );
    }
    println!("Reading {} frames…", files.len());
    let mut candidates = files
        .into_iter()
        .map(|path| {
            let headers = crate::image_io::read_header(&path).unwrap_or_default();
            Candidate {
                exposure_seconds: exposure_from_headers(&headers),
                acquired: string_card(&headers, "DATE-OBS"),
                quality: None,
                path,
            }
        })
        .collect::<Vec<_>>();

    match options.order {
        snr::StackFrameOrder::Capture => {
            candidates.sort_by(|left, right| {
                left.acquired
                    .cmp(&right.acquired)
                    .then_with(|| left.path.cmp(&right.path))
            });
        }
        snr::StackFrameOrder::Quality => {
            score_frames(&mut candidates, options.detector_threads)?;
            candidates.sort_by(|left, right| {
                right
                    .quality
                    .unwrap_or(0.0)
                    .total_cmp(&left.quality.unwrap_or(0.0))
                    .then_with(|| left.path.cmp(&right.path))
            });
        }
    }

    let (points, accepted_exposures) = accumulate(&candidates)?;
    let progressive = snr::ProgressiveSnr::new(options.order, points, &accepted_exposures);
    report(&progressive);

    if let Some(path) = &options.json {
        let bytes = serde_json::to_vec_pretty(&progressive)?;
        std::fs::write(path, bytes).with_context(|| format!("Writing {}", path.display()))?;
        println!("\nWrote {}", path.display());
    }
    if let Some(path) = &options.csv {
        std::fs::write(path, csv(&progressive))
            .with_context(|| format!("Writing {}", path.display()))?;
        println!("Wrote {}", path.display());
    }
    Ok(())
}

/// Stack the frames in order, reading the accumulator at every depth on the
/// ladder. This is the group build's loop with the catalog taken out.
fn accumulate(candidates: &[Candidate]) -> Result<(Vec<snr::SnrPoint>, Vec<f64>)> {
    use seiza_stacking::{FrameDisposition, LiveStacker, NormalizationMode, StackOptions};

    let reference = crate::image_io::open_linear_frame(&candidates[0].path)
        .map_err(|error| anyhow::anyhow!("Opening the reference frame: {error:?}"))?;
    let mut stacker = LiveStacker::new(
        reference,
        Default::default(),
        StackOptions {
            normalization: NormalizationMode::Global,
            ..StackOptions::default()
        },
    )
    .map_err(|error| anyhow::anyhow!("Starting the stack: {error}"))?;

    let pipeline = seiza_stacking::PipelineOptions {
        normalized_full_scale: Some(crate::image_io::NORMALIZED_FULL_SCALE),
        ..seiza_stacking::PipelineOptions::default()
    };
    let mut points = Vec::new();
    let mut integrated_exposure = candidates[0].exposure_seconds;
    let mut accepted_exposures = vec![candidates[0].exposure_seconds];
    let mut pushed = 1usize;
    if let Some(sample) = snr::measure(stacker.view()) {
        points.push(snr::point(sample, integrated_exposure));
    }

    for depth in snr::checkpoint_depths(candidates.len()) {
        if depth <= pushed {
            continue;
        }
        let batch = &candidates[pushed..depth];
        let paths: Vec<PathBuf> = batch.iter().map(|frame| frame.path.clone()).collect();
        let mut consumed = 0usize;
        // Every frame's outcome is reported in the callback, so the batch
        // summary adds nothing.
        let _report = stacker
            .push_fits_pipelined(&paths, &pipeline, |_, outcome| {
                let frame = &batch[consumed];
                consumed += 1;
                match outcome {
                    Ok(FrameDisposition::Accepted(_)) => {
                        integrated_exposure += frame.exposure_seconds;
                        accepted_exposures.push(frame.exposure_seconds);
                    }
                    Ok(FrameDisposition::Rejected(reason)) => {
                        eprintln!("  turned away {}: {reason}", frame.path.display());
                    }
                    Err(error) => {
                        eprintln!("  could not read {}: {error}", frame.path.display());
                    }
                }
                seiza_stacking::Continue::Yes
            })
            .map_err(|error| anyhow::anyhow!("Stacking: {error}"))?;
        pushed = depth;
        // A step that integrated nothing new — every frame in it turned away —
        // is not a depth on the curve.
        let advanced = points
            .last()
            .is_none_or(|last| last.frames < stacker.view().accepted_frames);
        if advanced && let Some(sample) = snr::measure(stacker.view()) {
            let measured = snr::point(sample, integrated_exposure);
            println!(
                "  {:>5} frames  noise {:>10.2}  ratio {:>8.2}",
                measured.frames, measured.noise, measured.snr
            );
            points.push(measured);
        }
    }
    Ok((points, accepted_exposures))
}

/// Rank frames by detected stars against their sharpness: more stars and
/// tighter stars, in that order, is the frame you would keep. The server's
/// quality order uses the catalog's own grading score instead, so the two
/// need not agree frame for frame.
fn score_frames(candidates: &mut [Candidate], threads: Option<usize>) -> Result<()> {
    use rayon::prelude::*;

    let frame_pixels = candidates
        .first()
        .and_then(|candidate| crate::concurrency::probe_frame_pixels(&candidate.path));
    let budget = quality_worker_budget(threads, frame_pixels);
    println!(
        "Detecting stars in {} frames with {} worker(s) — {}",
        candidates.len(),
        budget.workers,
        budget.rationale
    );
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(budget.workers)
        .build()
        .context("Building the detection pool")?;
    pool.install(|| {
        candidates.par_iter_mut().for_each(|candidate| {
            let Ok(image) = crate::image_analysis::FitsImage::from_file(&candidate.path) else {
                return;
            };
            let params = crate::hocus_focus_star_detection::HocusFocusParams::default();
            let result = crate::hocus_focus_star_detection::detect_stars_hocus_focus(
                &image.data,
                image.width,
                image.height,
                &params,
            );
            if result.stars.is_empty() || result.average_hfr <= 0.0 {
                return;
            }
            candidate.quality = Some(result.stars.len() as f64 / result.average_hfr);
        });
    });
    if candidates
        .iter()
        .all(|candidate| candidate.quality.is_none())
    {
        bail!("No frame yielded a star detection, so there is nothing to rank");
    }
    Ok(())
}

fn quality_worker_budget(
    requested: Option<usize>,
    frame_pixels: Option<usize>,
) -> crate::concurrency::WorkerBudget {
    crate::concurrency::plan_workers(
        requested,
        &crate::concurrency::WorkerPolicy::all_cores(),
        crate::concurrency::Priority::Interactive,
        frame_pixels,
    )
}

fn report(progressive: &snr::ProgressiveSnr) {
    println!(
        "\nProgressive signal-to-noise, {} order",
        progressive.order.as_str()
    );
    println!(
        "{:>7}  {:>9}  {:>12}  {:>10}  {:>10}",
        "frames", "hours", "noise", "signal", "ratio"
    );
    for point in &progressive.points {
        println!(
            "{:>7}  {:>9.2}  {:>12.3}  {:>10.1}  {:>10.2}",
            point.frames,
            point.exposure_seconds / 3600.0,
            point.noise,
            point.signal,
            point.snr
        );
    }
    let Some(analysis) = &progressive.analysis else {
        if let Some(reason) = &progressive.analysis_reason {
            println!("\n{reason}");
            return;
        }
        println!("\nToo few depths to read a trend.");
        return;
    };
    println!("\n{}", analysis.summary);
    println!(
        "  fitted exponent {:.3} (ideal -0.500), fit quality {:.2}",
        analysis.noise_exponent, analysis.fit_r_squared
    );
    if let Some(frames) = analysis.frames_for_90_percent {
        println!(
            "  reached 90% of the best ratio at {frames} frames ({:.2} h)",
            analysis.seconds_for_90_percent.unwrap_or(0.0) / 3600.0
        );
    }
    for projection in &analysis.projections {
        println!(
            "  {:.0}% more ratio: about {} more frames ({:.2} h)",
            (projection.gain - 1.0) * 100.0,
            projection.extra_frames,
            projection.extra_seconds / 3600.0
        );
    }
    for regression in &analysis.regressions {
        println!(
            "  noise rose {:.1}% between {} and {} frames",
            regression.noise_increase * 100.0,
            regression.from_frames,
            regression.to_frames
        );
    }
}

fn csv(progressive: &snr::ProgressiveSnr) -> String {
    let mut out = String::from("frames,exposure_seconds,noise,background,signal,snr\n");
    for point in &progressive.points {
        out.push_str(&format!(
            "{},{},{},{},{},{}\n",
            point.frames,
            point.exposure_seconds,
            point.noise,
            point.background,
            point.signal,
            point.snr
        ));
    }
    out
}

/// Every frame under the given files and directories, directories searched
/// all the way down.
fn collect_frames(paths: &[String]) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for path in paths {
        let path = Path::new(path);
        if path.is_dir() {
            walk(path, &mut found)?;
        } else if crate::image_io::is_image_path(path) {
            found.push(path.to_path_buf());
        } else {
            bail!("{} is not a frame this tool reads", path.display());
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

fn walk(directory: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    let entries =
        std::fs::read_dir(directory).with_context(|| format!("Reading {}", directory.display()))?;
    for entry in entries {
        let path = entry
            .with_context(|| format!("Reading {}", directory.display()))?
            .path();
        if path.is_dir() {
            walk(&path, found)?;
        } else if crate::image_io::is_image_path(&path) {
            found.push(path);
        }
    }
    Ok(())
}

fn exposure_from_headers(headers: &[(String, HeaderValue)]) -> f64 {
    for name in ["EXPTIME", "EXPOSURE"] {
        if let Some(value) = headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .and_then(|(_, value)| value.as_f64())
            && value > 0.0
        {
            return value;
        }
    }
    0.0
}

fn string_card(headers: &[(String, HeaderValue)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, value)| match value {
            HeaderValue::String(text) | HeaderValue::Raw(text) => Some(text.trim().to_string()),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_explicit_quality_worker_count_is_preserved() {
        let budget = super::quality_worker_budget(Some(3), Some(60_000_000));
        assert_eq!(budget.workers, 3);
        assert!(budget.rationale.contains("explicit override"));
    }
}
