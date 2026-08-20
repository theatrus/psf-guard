//! Reading a progressive signal-to-noise curve for a mono stack build.
//!
//! Seiza takes the measurement: `seiza_stacking::checkpoint_depths` picks the
//! depths a build should look at, and `measure_depth` reads the live
//! accumulator without copying it. This module is what PSF Guard does with
//! those readings — the curve it keeps, the trend it fits, and the words it
//! puts on a card.
//!
//! The split is deliberate. What the noise of an integration *is* belongs to
//! whoever owns the accumulator; what it *means* for a night's shooting
//! depends on an exposure model and a vocabulary that are this application's,
//! not a library's.
//!
//! What each depth records, measured upstream:
//!
//! - **Noise** is the robust spread of second differences measured in both
//!   image axes, scaled to a standard deviation. Second differences cancel a
//!   planar sky gradient, the median throws the stars away, and taking the
//!   noisier axis keeps row or column banding visible. What is left is the
//!   pixel-scale noise of the integration — the quantity that should fall as
//!   the square root of the frame count.
//! - **Background** is the median sample.
//! - **Signal** is how far the brightest one percent of samples sits above that
//!   background. The fraction is fixed rather than a multiple of the noise, so
//!   the same part of the sky is measured at every depth. A threshold that
//!   moved with the noise would widen as the stack deepened and write its own
//!   trend into the curve.
//!
//! Both statistics come from a capped subsample of rows, so a 60-megapixel
//! stack costs what a 6-megapixel one costs.
//!
//! The curve is measurement. [`analyze`] turns it into a prediction — the
//! fitted exponent, where the returns flattened, and what more frames would
//! buy — and everything it produces is labelled as the estimate it is.

use serde::{Deserialize, Serialize};

/// Perfect averaging: noise falls as the square root of the frame count.
const IDEAL_EXPONENT: f64 = -0.5;

/// Gains the projection prices, as multipliers on the current signal-to-noise
/// ratio.
const PROJECTED_GAINS: [f64; 2] = [1.05, 1.10];

/// A noise rise smaller than this across one step is measurement scatter, not
/// a frame that hurt.
const REGRESSION_THRESHOLD: f64 = 0.02;

/// A fitted rise must exceed the same two-percent scatter allowance over one
/// doubling before it is called a degrading trend: `ln(1.02) / ln(2)`.
const DEGRADING_EXPONENT_THRESHOLD: f64 = 0.028_569_152_196_770_92;

/// A slope needs to explain at least half the measured variation before it
/// becomes a directional verdict rather than an inconclusive fit.
const MIN_DIRECTIONAL_FIT_R_SQUARED: f64 = 0.5;

/// Exposure durations within one percent are equivalent for the frame-count
/// model. Larger differences need a weighted model that this curve does not
/// currently claim to provide.
const EXPOSURE_VARIATION_TOLERANCE: f64 = 0.01;

const MISSING_EXPOSURE_REASON: &str =
    "Analysis unavailable because exposure duration is missing for one or more accepted frames.";
const MIXED_EXPOSURE_REASON: &str =
    "Analysis unavailable because exposure duration varies between accepted frames.";

/// The order a build pushes its frames in, which decides what its curve
/// answers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackFrameOrder {
    /// Registration reference first, then the remaining frames in capture
    /// order. The curve shows the broad trend through the later data while
    /// keeping registration anchored to the chosen frame.
    #[default]
    Capture,
    /// Best-graded frame first. The curve answers "which of these frames are
    /// worth keeping?" — the depth where it leaves the ideal is where the
    /// weaker frames stop paying for themselves.
    Quality,
}

impl StackFrameOrder {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Quality => "quality",
        }
    }

    /// Whether a build in this order may extend a saved accumulator. Quality
    /// order may not: a frame added later can sort into the middle of the
    /// order, and the accumulator has already integrated everything after it.
    pub fn resumable(self) -> bool {
        matches!(self, Self::Capture)
    }
}

/// One depth on the curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnrPoint {
    /// Frames the accumulator had taken when this was measured.
    pub frames: u32,
    /// Their exposure, summed.
    pub exposure_seconds: f64,
    /// Pixel-to-pixel noise of the integration, in the stack's own units.
    pub noise: f64,
    /// Median sample.
    pub background: f64,
    /// How far the brightest one percent sits above the background.
    pub signal: f64,
    /// `signal / noise`.
    pub snr: f64,
    /// Per-channel noise. One entry for mono, three for a debayered stack.
    #[serde(default)]
    pub channel_noise: Vec<f64>,
}

/// Where the curve stands, in one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnrVerdict {
    /// The deeper measurements do not support a stable directional fit.
    Uncertain,
    /// Noise is still falling at close to the ideal rate. More frames pay.
    Improving,
    /// Noise is still falling, but well short of the ideal rate.
    Diminishing,
    /// Noise has stopped falling. More of the same frames will not help.
    Plateau,
    /// Noise rose over the deeper part of the curve.
    Degrading,
}

impl SnrVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uncertain => "uncertain",
            Self::Improving => "improving",
            Self::Diminishing => "diminishing",
            Self::Plateau => "plateau",
            Self::Degrading => "degrading",
        }
    }
}

/// What the fitted trend says more frames would buy. A prediction, not a
/// measurement.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SnrProjection {
    /// The gain being priced, as a multiplier: 1.05 is five percent.
    pub gain: f64,
    /// Frames the trend says it takes, on top of the ones already stacked.
    pub extra_frames: u64,
    /// Those frames at this stack's average exposure.
    pub extra_seconds: f64,
}

/// A span of the curve where the noise rose instead of falling.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SnrRegression {
    pub from_frames: u32,
    pub to_frames: u32,
    /// How far the noise rose across the span, as a fraction of where it
    /// started.
    pub noise_increase: f64,
}

/// The reading of a finished curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnrAnalysis {
    pub measured_frames: u32,
    pub measured_seconds: f64,
    pub best_snr: f64,
    pub final_noise: f64,
    /// The exponent in `noise ∝ frames^b`, fitted over the deeper part of the
    /// curve — the part that describes where the stack is now.
    pub noise_exponent: f64,
    /// The same fit over every point.
    pub overall_noise_exponent: f64,
    /// How well the deeper part fits a power law, from 0 to 1.
    pub fit_r_squared: f64,
    /// What perfect averaging gives: −0.5.
    pub ideal_exponent: f64,
    /// `noise_exponent` against the ideal, so 1.0 is textbook square-root
    /// improvement and 0 is a flat curve.
    pub efficiency: f64,
    /// The first depth that reached nine tenths of the best measured ratio.
    pub frames_for_90_percent: Option<u32>,
    pub seconds_for_90_percent: Option<f64>,
    /// The same for nineteen twentieths.
    pub frames_for_95_percent: Option<u32>,
    pub seconds_for_95_percent: Option<f64>,
    pub projections: Vec<SnrProjection>,
    pub regressions: Vec<SnrRegression>,
    pub verdict: SnrVerdict,
    /// One sentence a reader can act on.
    pub summary: String,
}

/// A group's curve and what it means.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressiveSnr {
    pub order: StackFrameOrder,
    pub points: Vec<SnrPoint>,
    /// Absent until three depths have been measured, or when their exposure
    /// durations cannot support the frame-count model.
    pub analysis: Option<SnrAnalysis>,
    /// Why a complete measured curve cannot be modeled. An ordinary partial
    /// curve has neither an analysis nor a reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis_reason: Option<String>,
}

impl ProgressiveSnr {
    /// Build a curve from the depths measured so far, putting every depth's
    /// ratio on one signal before reading it.
    ///
    /// A target's flux does not change as a stack deepens; only the noise
    /// does. But the brightest-percent statistic is itself lifted by noise at
    /// shallow depths — a one-frame stack's brightest percent is part star
    /// and part noise peak — so reading each depth against its own signal
    /// would write that bias into the ratio and flatter the early frames.
    /// The deepest measurement is the best estimate of the signal there is,
    /// so every depth is read against it and the shape of the curve comes
    /// from the noise alone. Each depth keeps the signal it measured, which
    /// is worth seeing when it drifts: that is normalization moving, not the
    /// sky.
    ///
    /// `frame_exposures` contains every accepted frame through the deepest
    /// point. The fitted frame-count model is withheld unless each duration
    /// is known and equivalent.
    pub fn new(order: StackFrameOrder, mut points: Vec<SnrPoint>, frame_exposures: &[f64]) -> Self {
        if let Some(signal) = points.last().map(|deepest| deepest.signal) {
            for point in &mut points {
                point.snr = if point.noise > 0.0 {
                    signal / point.noise
                } else {
                    0.0
                };
            }
        }
        let analysis_reason = exposure_analysis_reason(&points, frame_exposures).map(str::to_owned);
        let analysis = if analysis_reason.is_none() {
            analyze_curve(&points)
        } else {
            None
        };
        Self {
            order,
            points,
            analysis,
            analysis_reason,
        }
    }
}

/// Turn a sample into a curve point once its exposure is known.
/// Turn one of Seiza's readings into a curve point, once its exposure is
/// known.
pub fn point(sample: seiza_stacking::SnrSample, exposure_seconds: f64) -> SnrPoint {
    SnrPoint {
        frames: sample.frames,
        exposure_seconds,
        noise: sample.noise,
        background: sample.background,
        signal: sample.signal,
        snr: sample.snr(),
        channel_noise: sample.channel_noise,
    }
}

/// Read a finished curve. `None` until three usable depths exist, because two
/// points fit any power law exactly and say nothing about the fit.
#[cfg(test)]
fn analyze(points: &[SnrPoint]) -> Option<SnrAnalysis> {
    analyze_curve(points)
}

fn analyze_curve(points: &[SnrPoint]) -> Option<SnrAnalysis> {
    let usable: Vec<&SnrPoint> = points
        .iter()
        .filter(|point| point.frames >= 1 && point.noise > 0.0 && point.snr.is_finite())
        .collect();
    if usable.len() < 3 {
        return None;
    }
    let deepest = *usable.last()?;
    let overall = fit(&usable)?;
    // The stack is where its last frames put it, so what more frames would
    // buy is read from the deeper part of the curve. The first frames of any
    // stack improve it steeply and would flatter the estimate.
    let cutoff = f64::from(deepest.frames) / 4.0;
    let recent: Vec<&&SnrPoint> = usable
        .iter()
        .filter(|point| f64::from(point.frames) >= cutoff)
        .collect();
    let recent_fit = if recent.len() >= 3 {
        let recent: Vec<&SnrPoint> = recent.iter().map(|point| **point).collect();
        fit(&recent).unwrap_or(overall)
    } else {
        overall
    };

    let best_snr = usable
        .iter()
        .map(|point| point.snr)
        .fold(f64::NEG_INFINITY, f64::max);
    let reached = |share: f64| -> (Option<u32>, Option<f64>) {
        usable
            .iter()
            .find(|point| point.snr >= best_snr * share)
            .map_or((None, None), |point| {
                (Some(point.frames), Some(point.exposure_seconds))
            })
    };
    let (frames_for_90_percent, seconds_for_90_percent) = reached(0.90);
    let (frames_for_95_percent, seconds_for_95_percent) = reached(0.95);

    let efficiency = (recent_fit.exponent / IDEAL_EXPONENT).max(0.0);
    let verdict = if recent_fit.r_squared < MIN_DIRECTIONAL_FIT_R_SQUARED {
        SnrVerdict::Uncertain
    } else if recent_fit.exponent > DEGRADING_EXPONENT_THRESHOLD {
        SnrVerdict::Degrading
    } else if efficiency >= 0.75 {
        SnrVerdict::Improving
    } else if efficiency >= 0.35 {
        SnrVerdict::Diminishing
    } else {
        SnrVerdict::Plateau
    };

    let average_exposure = if deepest.frames > 0 {
        deepest.exposure_seconds / f64::from(deepest.frames)
    } else {
        0.0
    };
    let projections = if verdict == SnrVerdict::Uncertain {
        Vec::new()
    } else {
        project(deepest.frames, recent_fit.exponent, average_exposure)
    };
    let regressions = regressions(&usable);
    let summary = summarize(
        verdict,
        efficiency,
        recent_fit.exponent,
        &projections,
        &regressions,
    );

    Some(SnrAnalysis {
        measured_frames: deepest.frames,
        measured_seconds: deepest.exposure_seconds,
        best_snr,
        final_noise: deepest.noise,
        noise_exponent: recent_fit.exponent,
        overall_noise_exponent: overall.exponent,
        fit_r_squared: recent_fit.r_squared,
        ideal_exponent: IDEAL_EXPONENT,
        efficiency,
        frames_for_90_percent,
        seconds_for_90_percent,
        frames_for_95_percent,
        seconds_for_95_percent,
        projections,
        regressions,
        verdict,
        summary,
    })
}

/// Confirm that frame count is a meaningful independent variable from every
/// accepted frame, rather than from checkpoint averages that could hide a
/// mixture of short and long exposures.
fn exposure_analysis_reason(points: &[SnrPoint], frame_exposures: &[f64]) -> Option<&'static str> {
    let expected_frames = points.last().map_or(0, |point| point.frames as usize);
    if expected_frames == 0 {
        return None;
    }
    if frame_exposures.len() < expected_frames {
        return Some(MISSING_EXPOSURE_REASON);
    }
    let mut minimum_per_frame = f64::INFINITY;
    let mut maximum_per_frame = f64::NEG_INFINITY;

    for &exposure in &frame_exposures[..expected_frames] {
        if !exposure.is_finite() || exposure <= 0.0 {
            return Some(MISSING_EXPOSURE_REASON);
        }
        minimum_per_frame = minimum_per_frame.min(exposure);
        maximum_per_frame = maximum_per_frame.max(exposure);
    }

    if minimum_per_frame.is_finite()
        && (maximum_per_frame - minimum_per_frame) / maximum_per_frame
            > EXPOSURE_VARIATION_TOLERANCE
    {
        Some(MIXED_EXPOSURE_REASON)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy)]
struct Fit {
    exponent: f64,
    r_squared: f64,
}

/// Least squares on `log(noise)` against `log(frames)`.
fn fit(points: &[&SnrPoint]) -> Option<Fit> {
    let samples: Vec<(f64, f64)> = points
        .iter()
        .map(|point| (f64::from(point.frames).ln(), point.noise.ln()))
        .filter(|(x, y)| x.is_finite() && y.is_finite())
        .collect();
    if samples.len() < 2 {
        return None;
    }
    let count = samples.len() as f64;
    let mean_x = samples.iter().map(|(x, _)| x).sum::<f64>() / count;
    let mean_y = samples.iter().map(|(_, y)| y).sum::<f64>() / count;
    let mut covariance = 0.0;
    let mut variance_x = 0.0;
    for (x, y) in &samples {
        covariance += (x - mean_x) * (y - mean_y);
        variance_x += (x - mean_x) * (x - mean_x);
    }
    if variance_x <= 0.0 {
        return None;
    }
    let exponent = covariance / variance_x;
    let intercept = mean_y - exponent * mean_x;
    let mut residual = 0.0;
    let mut total = 0.0;
    for (x, y) in &samples {
        let predicted = intercept + exponent * x;
        residual += (y - predicted) * (y - predicted);
        total += (y - mean_y) * (y - mean_y);
    }
    let r_squared = if total > 0.0 {
        (1.0 - residual / total).clamp(0.0, 1.0)
    } else {
        1.0
    };
    Some(Fit {
        exponent,
        r_squared,
    })
}

/// Price the standard gains against the fitted trend. A curve flat enough
/// that the answer runs to tens of thousands of frames is reported as no
/// answer rather than as a number nobody can act on.
fn project(frames: u32, exponent: f64, average_exposure: f64) -> Vec<SnrProjection> {
    if exponent >= -0.01 || frames == 0 {
        return Vec::new();
    }
    let frames = f64::from(frames);
    PROJECTED_GAINS
        .iter()
        .filter_map(|&gain| {
            // The ratio rises as frames^|exponent|, so multiplying it by
            // `gain` needs frames to rise by gain^(1/|exponent|).
            let needed = frames * gain.powf(1.0 / exponent.abs());
            let extra = (needed - frames).ceil();
            if !extra.is_finite() || extra < 1.0 || extra > frames * 100.0 {
                return None;
            }
            Some(SnrProjection {
                gain,
                extra_frames: extra as u64,
                extra_seconds: extra * average_exposure,
            })
        })
        .collect()
}

/// Steps where the noise rose. In reference-first capture order these are
/// later parts of the capture sequence that hurt; in quality order they are
/// where the weaker frames start costing more than they add.
fn regressions(points: &[&SnrPoint]) -> Vec<SnrRegression> {
    points
        .windows(2)
        .filter_map(|pair| {
            let (earlier, later) = (pair[0], pair[1]);
            if earlier.noise <= 0.0 {
                return None;
            }
            let increase = (later.noise - earlier.noise) / earlier.noise;
            (increase > REGRESSION_THRESHOLD).then_some(SnrRegression {
                from_frames: earlier.frames,
                to_frames: later.frames,
                noise_increase: increase,
            })
        })
        .collect()
}

fn summarize(
    verdict: SnrVerdict,
    efficiency: f64,
    exponent: f64,
    projections: &[SnrProjection],
    regressions: &[SnrRegression],
) -> String {
    let share = (efficiency * 100.0).round();
    let mut summary = match verdict {
        SnrVerdict::Uncertain =>
            "The deeper measurements are too inconsistent to establish a trend. No projection is made from this curve."
                .to_string(),
        SnrVerdict::Improving => format!(
            "Noise is still falling at {share:.0}% of the ideal rate (exponent {exponent:.2} against -0.50). More frames pay off."
        ),
        SnrVerdict::Diminishing => format!(
            "Noise is falling at {share:.0}% of the ideal rate (exponent {exponent:.2} against -0.50). More frames still help, but each one buys less than it should."
        ),
        SnrVerdict::Plateau => format!(
            "Noise has all but stopped falling (exponent {exponent:.2} against an ideal -0.50). More frames like these will not help much."
        ),
        SnrVerdict::Degrading => format!(
            "Noise rose over the deeper part of this run (exponent {exponent:.2}). Later frames are hurting the stack."
        ),
    };
    if let Some(projection) = projections.first() {
        let hours = projection.extra_seconds / 3600.0;
        let gain = ((projection.gain - 1.0) * 100.0).round();
        summary.push_str(&format!(
            " Another {gain:.0}% needs about {} more frames ({hours:.1} h) at this rate.",
            projection.extra_frames
        ));
    }
    if let Some(first) = regressions.first() {
        summary.push_str(&format!(
            " Noise rose {:.0}% between {} and {} frames.",
            first.noise_increase * 100.0,
            first.from_frames,
            first.to_frames
        ));
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(noise_at: impl Fn(u32) -> f64, depths: &[u32]) -> Vec<SnrPoint> {
        depths
            .iter()
            .map(|&frames| {
                let noise = noise_at(frames);
                SnrPoint {
                    frames,
                    exposure_seconds: f64::from(frames) * 300.0,
                    noise,
                    background: 1000.0,
                    signal: 500.0,
                    snr: 500.0 / noise,
                    channel_noise: vec![noise],
                }
            })
            .collect()
    }

    #[test]
    fn a_textbook_stack_reads_as_still_improving() {
        let points = curve(|n| 20.0 / f64::from(n).sqrt(), &[1, 2, 4, 8, 16, 32, 40]);
        let analysis = analyze(&points).expect("three points is enough");
        assert!((analysis.noise_exponent + 0.5).abs() < 0.01);
        assert!((analysis.efficiency - 1.0).abs() < 0.02);
        assert!(analysis.fit_r_squared > 0.99);
        assert_eq!(analysis.verdict, SnrVerdict::Improving);
        assert!(analysis.regressions.is_empty());
        // Ten percent more ratio at the ideal rate costs 21% more frames.
        let ten = analysis
            .projections
            .iter()
            .find(|projection| (projection.gain - 1.10).abs() < 1e-9)
            .expect("a ten percent projection");
        assert_eq!(ten.extra_frames, 9, "1.10^2 × 40 = 48.4 frames");
        assert!((ten.extra_seconds - 2700.0).abs() < 1.0);
    }

    #[test]
    fn a_stack_that_stopped_improving_reads_as_a_plateau() {
        // Noise falls to a floor it cannot get under: light pollution
        // residual, a fixed pattern, an undithered sensor.
        let points = curve(
            |n| 10.0 + 20.0 / f64::from(n).sqrt(),
            &[1, 2, 4, 8, 16, 32, 64, 128, 256],
        );
        let analysis = analyze(&points).expect("analyzable");
        assert!(analysis.efficiency < 0.35, "{}", analysis.efficiency);
        assert_eq!(analysis.verdict, SnrVerdict::Plateau);
        assert!(analysis.summary.contains("will not help"));
    }

    #[test]
    fn frames_that_hurt_the_stack_are_named() {
        let mut points = curve(|n| 20.0 / f64::from(n).sqrt(), &[1, 2, 4, 8, 16, 32]);
        // Noise at 16 frames is 5.0. The last stretch of frames put it back
        // up a fifth instead of taking it down to 3.54.
        let last = points.last_mut().expect("a last point");
        last.noise = 6.0;
        last.snr = last.signal / last.noise;
        let analysis = analyze(&points).expect("analyzable");
        assert_eq!(analysis.regressions.len(), 1);
        assert_eq!(analysis.regressions[0].from_frames, 16);
        assert_eq!(analysis.regressions[0].to_frames, 32);
        assert!((analysis.regressions[0].noise_increase - 0.2).abs() < 0.01);
        assert!(analysis.summary.contains("Noise rose"));
    }

    #[test]
    fn a_run_that_only_got_worse_reads_as_degrading() {
        let points = curve(|n| 10.0 + f64::from(n), &[1, 2, 4, 8, 16, 32]);
        let analysis = analyze(&points).expect("analyzable");
        assert_eq!(analysis.verdict, SnrVerdict::Degrading);
        assert!(analysis.projections.is_empty(), "nothing to promise");
    }

    #[test]
    fn a_tiny_well_fitted_rise_is_measurement_scatter() {
        let points = curve(
            |n| 10.0 * f64::from(n).powf(0.015),
            &[1, 2, 4, 8, 16, 32, 64],
        );
        let analysis = analyze(&points).expect("analyzable");
        assert!(analysis.noise_exponent > 0.0);
        assert!(analysis.fit_r_squared > 0.99);
        assert_eq!(analysis.verdict, SnrVerdict::Plateau);
    }

    #[test]
    fn a_poorly_fitted_positive_slope_is_not_called_degrading() {
        let mut points = curve(|n| 20.0 / f64::from(n).sqrt(), &[1, 2, 4, 8, 16, 32, 64]);
        points[4].noise = 10.0;
        points[5].noise = 14.0;
        points[6].noise = 10.8;
        for point in &mut points[4..] {
            point.snr = point.signal / point.noise;
        }
        let analysis = analyze_curve(&points).expect("the raw fit is available");
        assert!(
            analysis.noise_exponent > DEGRADING_EXPONENT_THRESHOLD,
            "{}",
            analysis.noise_exponent
        );
        assert!(
            analysis.fit_r_squared < MIN_DIRECTIONAL_FIT_R_SQUARED,
            "{}",
            analysis.fit_r_squared
        );
        let progressive = ProgressiveSnr::new(StackFrameOrder::Capture, points, &[300.0; 64]);
        let analysis = progressive.analysis.expect("an inconclusive analysis");
        assert_eq!(analysis.verdict, SnrVerdict::Uncertain);
        assert!(analysis.projections.is_empty());
        assert!(progressive.analysis_reason.is_none());
    }

    #[test]
    fn a_poorly_fitted_negative_slope_does_not_make_promises() {
        let mut points = curve(|n| 20.0 / f64::from(n).sqrt(), &[1, 2, 4, 8, 16, 32, 64]);
        points[4].noise = 10.0;
        points[5].noise = 30.0;
        points[6].noise = 3.0;
        for point in &mut points[4..] {
            point.snr = point.signal / point.noise;
        }
        let raw = analyze_curve(&points).expect("the raw fit is available");
        assert!(raw.noise_exponent < 0.0);
        assert!(raw.fit_r_squared < MIN_DIRECTIONAL_FIT_R_SQUARED);

        let progressive = ProgressiveSnr::new(StackFrameOrder::Capture, points, &[300.0; 64]);
        let analysis = progressive.analysis.expect("an inconclusive analysis");
        assert_eq!(analysis.verdict, SnrVerdict::Uncertain);
        assert!(analysis.projections.is_empty());
        assert!(progressive.analysis_reason.is_none());
    }

    #[test]
    fn two_points_are_not_a_curve() {
        let points = curve(|n| 20.0 / f64::from(n).sqrt(), &[1, 2]);
        assert!(analyze(&points).is_none());
    }

    #[test]
    fn mixed_exposures_keep_measurements_but_suppress_the_model() {
        let points = curve(|n| 20.0 / f64::from(n).sqrt(), &[1, 2, 4, 8]);
        // The 200 and 400 second frames average to 300 at the four-frame
        // checkpoint. Per-frame validation must still catch the mixture.
        let exposures = [300.0, 300.0, 200.0, 400.0, 300.0, 300.0, 300.0, 300.0];

        let progressive = ProgressiveSnr::new(StackFrameOrder::Capture, points.clone(), &exposures);
        assert_eq!(progressive.points.len(), points.len());
        assert!(progressive.analysis.is_none());
        assert_eq!(
            progressive.analysis_reason.as_deref(),
            Some(MIXED_EXPOSURE_REASON)
        );
    }

    #[test]
    fn missing_exposure_suppresses_the_model_with_a_reason() {
        let points = curve(|n| 20.0 / f64::from(n).sqrt(), &[1, 2, 4, 8]);
        let progressive = ProgressiveSnr::new(
            StackFrameOrder::Capture,
            points,
            &[300.0, 300.0, 0.0, 300.0, 300.0, 300.0, 300.0, 300.0],
        );
        assert!(progressive.analysis.is_none());
        assert_eq!(
            progressive.analysis_reason.as_deref(),
            Some(MISSING_EXPOSURE_REASON)
        );
    }

    #[test]
    fn insignificant_exposure_rounding_keeps_the_model_available() {
        let points = curve(|n| 20.0 / f64::from(n).sqrt(), &[1, 2, 4, 8]);
        let progressive = ProgressiveSnr::new(
            StackFrameOrder::Capture,
            points,
            &[300.0, 302.0, 301.0, 302.0, 300.0, 302.0, 301.0, 302.0],
        );
        assert!(progressive.analysis.is_some());
        assert!(progressive.analysis_reason.is_none());
    }

    #[test]
    fn accepted_frames_after_the_last_measurement_do_not_invalidate_it() {
        let points = curve(|n| 20.0 / f64::from(n).sqrt(), &[1, 2, 4]);
        let progressive = ProgressiveSnr::new(
            StackFrameOrder::Capture,
            points,
            &[300.0, 300.0, 300.0, 300.0, 600.0],
        );

        assert!(progressive.analysis.is_some());
        assert!(progressive.analysis_reason.is_none());
    }

    #[test]
    fn the_depth_that_reached_most_of_the_ratio_is_reported() {
        let points = curve(|n| 20.0 / f64::from(n).sqrt(), &[1, 2, 4, 8, 16, 32, 64]);
        let analysis = analyze(&points).expect("analyzable");
        // Ninety percent of the best ratio needs 0.81 of the frames; the
        // first measured depth at or past that is the whole set.
        assert_eq!(analysis.frames_for_90_percent, Some(64));
        assert_eq!(analysis.seconds_for_90_percent, Some(19_200.0));
    }

    #[test]
    fn every_depth_is_read_against_the_deepest_signal() {
        // The brightest-percent statistic is lifted by noise at shallow
        // depths. Left alone it would make a one-frame stack look worse than
        // it is and flatter every frame after it.
        let mut points = curve(|n| 20.0 / f64::from(n).sqrt(), &[1, 2, 4, 8]);
        points[0].signal = 300.0;
        points[0].snr = 300.0 / points[0].noise;
        let progressive = ProgressiveSnr::new(StackFrameOrder::Capture, points, &[300.0; 8]);
        // Every ratio now comes off the deepest signal, 500.
        for point in &progressive.points {
            assert!((point.snr - 500.0 / point.noise).abs() < 1e-9);
        }
        // The measurement itself is kept: signal that drifts is worth seeing.
        assert_eq!(progressive.points[0].signal, 300.0);
        let analysis = progressive.analysis.expect("analyzable");
        assert!((analysis.noise_exponent + 0.5).abs() < 0.01);
    }

    #[test]
    fn quality_order_never_extends_a_saved_accumulator() {
        assert!(StackFrameOrder::Capture.resumable());
        assert!(!StackFrameOrder::Quality.resumable());
    }
}
