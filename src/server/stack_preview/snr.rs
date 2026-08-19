//! Progressive signal-to-noise measurement for a mono stack build.
//!
//! A stack accumulator holds a running mean that is already the integration of
//! every frame pushed so far. Reading how deep a stack needs to go is
//! therefore a matter of looking at that mean a few times on the way past, not
//! of integrating the same frames again once per depth: [`checkpoint_depths`]
//! picks the depths, the group build splits its push batches there, and
//! [`measure`] reads the live accumulator without copying it.
//!
//! What each depth records:
//!
//! - **Noise** is the median absolute difference between horizontally adjacent
//!   samples, scaled to a standard deviation. First differences cancel any sky
//!   gradient and any nebulosity broader than a pixel, and the median throws
//!   the stars away, so what is left is the pixel-to-pixel noise of the
//!   integration — the quantity that should fall as the square root of the
//!   frame count.
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

/// Rows read per measurement, at most. Noise and level statistics converge
/// long before a full frame is consumed, and this keeps the cost of a
/// checkpoint flat across sensor sizes.
const MAX_SAMPLED_ROWS: usize = 512;

/// Fewer samples than this and the medians are not worth reporting.
const MIN_SAMPLES: usize = 1024;

/// The brightest share of the frame that stands in for the target's signal.
const SIGNAL_FRACTION: f64 = 0.01;

/// Median absolute deviation to standard deviation, for a normal
/// distribution.
const MAD_TO_SIGMA: f64 = 1.482_602_218_505_602;

/// Perfect averaging: noise falls as the square root of the frame count.
const IDEAL_EXPONENT: f64 = -0.5;

/// Gains the projection prices, as multipliers on the current signal-to-noise
/// ratio.
const PROJECTED_GAINS: [f64; 2] = [1.05, 1.10];

/// A noise rise smaller than this across one step is measurement scatter, not
/// a frame that hurt.
const REGRESSION_THRESHOLD: f64 = 0.02;

/// The order a build pushes its frames in, which decides what its curve
/// answers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackFrameOrder {
    /// Chronological. The curve answers "did the last night help, and would
    /// another one help?".
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

/// What one read of the live accumulator found.
#[derive(Debug, Clone, PartialEq)]
pub struct SnrSample {
    pub frames: u32,
    pub noise: f64,
    pub background: f64,
    pub signal: f64,
    pub channel_noise: Vec<f64>,
}

/// Where the curve stands, in one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnrVerdict {
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
    /// Absent until three depths have been measured.
    pub analysis: Option<SnrAnalysis>,
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
    pub fn new(order: StackFrameOrder, mut points: Vec<SnrPoint>) -> Self {
        if let Some(signal) = points.last().map(|deepest| deepest.signal) {
            for point in &mut points {
                point.snr = if point.noise > 0.0 {
                    signal / point.noise
                } else {
                    0.0
                };
            }
        }
        Self {
            order,
            analysis: analyze(&points),
            points,
        }
    }
}

/// The depths a build of `total` frames measures at: the doubling ladder, and
/// the full set.
///
/// Doubling keeps the count of checkpoints logarithmic, so a five-hundred
/// frame stack splits its push batches nine times rather than five hundred,
/// and the points still spread evenly once the curve is drawn against a log
/// axis.
pub fn checkpoint_depths(total: usize) -> Vec<usize> {
    let mut depths = Vec::new();
    let mut depth = 1usize;
    while depth < total {
        depths.push(depth);
        depth *= 2;
    }
    if total > 0 {
        depths.push(total);
    }
    depths
}

/// Read the live accumulator. `None` when too little of the frame is covered
/// to measure, or when the samples carry no noise at all.
pub fn measure(view: seiza_stacking::StackView<'_>) -> Option<SnrSample> {
    let channels = view.channels.max(1);
    let stride = view.height.div_ceil(MAX_SAMPLED_ROWS).max(1);
    let mut channel_noise = Vec::with_capacity(channels);
    let mut channel_background = Vec::with_capacity(channels);
    let mut channel_signal = Vec::with_capacity(channels);

    for channel in 0..channels {
        let mut levels: Vec<f32> = Vec::new();
        let mut differences: Vec<f32> = Vec::new();
        let mut y = 0usize;
        while y < view.height {
            let row = y * view.width;
            // Reset at the start of every row: the last sample of one row is
            // not adjacent to the first of the next.
            let mut previous: Option<(f32, u32)> = None;
            for x in 0..view.width {
                let index = (row + x) * channels + channel;
                let coverage = view.coverage[index];
                let value = view.mean[index];
                if coverage == 0 || !value.is_finite() {
                    previous = None;
                    continue;
                }
                // Two samples only compare when the same number of frames
                // reached both. At a dithered edge one neighbour can be
                // thinner than the other, and its extra noise is not a
                // difference between neighbouring pixels.
                if let Some((earlier, earlier_coverage)) = previous
                    && earlier_coverage == coverage
                {
                    differences.push((value - earlier).abs());
                }
                levels.push(value);
                previous = Some((value, coverage));
            }
            y += stride;
        }

        if differences.len() < MIN_SAMPLES || levels.len() < MIN_SAMPLES {
            return None;
        }
        // Differences of two samples of equal noise carry root-two times the
        // noise of one sample.
        let noise = median(&mut differences) * MAD_TO_SIGMA / std::f64::consts::SQRT_2;
        if noise <= 0.0 || !noise.is_finite() {
            return None;
        }
        let background = median(&mut levels);
        channel_noise.push(noise);
        channel_background.push(background);
        channel_signal.push(brightest_above(&mut levels, background));
    }

    let frames = view.accepted_frames;
    let noise = mean(&channel_noise);
    let background = mean(&channel_background);
    let signal = mean(&channel_signal);
    Some(SnrSample {
        frames,
        noise,
        background,
        signal,
        channel_noise,
    })
}

/// Turn a sample into a curve point once its exposure is known.
pub fn point(sample: SnrSample, exposure_seconds: f64) -> SnrPoint {
    SnrPoint {
        frames: sample.frames,
        exposure_seconds,
        noise: sample.noise,
        background: sample.background,
        signal: sample.signal,
        snr: sample.signal / sample.noise,
        channel_noise: sample.channel_noise,
    }
}

/// Read a finished curve. `None` until three usable depths exist, because two
/// points fit any power law exactly and say nothing about the fit.
pub fn analyze(points: &[SnrPoint]) -> Option<SnrAnalysis> {
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
    let verdict = if recent_fit.exponent >= 0.0 {
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
    let projections = project(deepest.frames, recent_fit.exponent, average_exposure);
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

/// Steps where the noise rose. In capture order these are the nights that
/// hurt; in quality order they are where the weaker frames start costing more
/// than they add.
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

fn median(values: &mut [f32]) -> f64 {
    let middle = values.len() / 2;
    let (_, value, _) = values.select_nth_unstable_by(middle, f32::total_cmp);
    f64::from(*value)
}

/// How far the brightest [`SIGNAL_FRACTION`] of the samples sits above the
/// background. Reorders `values`.
fn brightest_above(values: &mut [f32], background: f64) -> f64 {
    let cut = ((values.len() as f64) * (1.0 - SIGNAL_FRACTION)) as usize;
    let cut = cut.min(values.len().saturating_sub(1));
    let (_, pivot, brightest) = values.select_nth_unstable_by(cut, f32::total_cmp);
    let mut total = f64::from(*pivot);
    let mut count = 1usize;
    for value in brightest {
        total += f64::from(*value);
        count += 1;
    }
    (total / count as f64 - background).max(0.0)
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic normal generator, so a noise measurement can be
    /// checked against a number the test chose.
    struct Noise(u64);

    impl Noise {
        fn next_uniform(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 11) as f64 / (1u64 << 53) as f64).mul_add(0.999_999, 5e-7)
        }

        fn next_normal(&mut self) -> f64 {
            let first = self.next_uniform();
            let second = self.next_uniform();
            (-2.0 * first.ln()).sqrt() * (std::f64::consts::TAU * second).cos()
        }
    }

    /// A flat field of the given background plus the given noise, with a
    /// bright patch standing in for a target.
    fn frame(width: usize, height: usize, background: f32, sigma: f64, seed: u64) -> Vec<f32> {
        let mut noise = Noise(seed);
        let mut data = vec![0.0f32; width * height];
        for (index, sample) in data.iter_mut().enumerate() {
            let (x, y) = (index % width, index / width);
            // Two percent of the frame is bright, so the signal statistic has
            // something to find above the background.
            let object = if y < height / 10 { 500.0 } else { 0.0 };
            // A sky gradient the difference estimator must ignore.
            let gradient = x as f32 * 0.05;
            *sample = background + object + gradient + (noise.next_normal() * sigma) as f32;
        }
        data
    }

    fn view<'a>(
        data: &'a [f32],
        coverage: &'a [u32],
        width: usize,
        height: usize,
        frames: u32,
    ) -> seiza_stacking::StackView<'a> {
        seiza_stacking::StackView {
            width,
            height,
            channels: 1,
            mean: data,
            coverage,
            rejected_samples: coverage,
            accepted_frames: frames,
            rejected_frames: 0,
        }
    }

    #[test]
    fn checkpoint_depths_double_and_end_on_the_full_set() {
        assert_eq!(checkpoint_depths(0), Vec::<usize>::new());
        assert_eq!(checkpoint_depths(1), vec![1]);
        assert_eq!(checkpoint_depths(5), vec![1, 2, 4, 5]);
        assert_eq!(checkpoint_depths(16), vec![1, 2, 4, 8, 16]);
        assert_eq!(checkpoint_depths(100), vec![1, 2, 4, 8, 16, 32, 64, 100]);
    }

    #[test]
    fn a_five_hundred_frame_stack_splits_its_batches_nine_times() {
        // The whole point of the doubling ladder: measuring is cheap, but
        // every checkpoint costs a pipelined batch boundary.
        assert_eq!(checkpoint_depths(500).len(), 10);
    }

    #[test]
    fn noise_is_measured_through_a_sky_gradient_and_a_bright_target() {
        let (width, height) = (400, 400);
        let data = frame(width, height, 1000.0, 12.0, 7);
        let coverage = vec![4u32; data.len()];
        let sample = measure(view(&data, &coverage, width, height, 4)).expect("measurable");
        assert!(
            (sample.noise - 12.0).abs() < 0.6,
            "measured {} for a sigma of 12",
            sample.noise
        );
        // The gradient runs to 20 ADU across the frame and the target sits
        // 500 above it; neither may leak into the noise.
        assert!(
            (sample.background - 1010.0).abs() < 5.0,
            "{}",
            sample.background
        );
        assert!(sample.signal > 400.0, "{}", sample.signal);
    }

    #[test]
    fn averaging_four_times_the_frames_halves_the_measured_noise() {
        let (width, height) = (400, 400);
        let coverage_four = vec![4u32; width * height];
        let coverage_sixteen = vec![16u32; width * height];
        let shallow = frame(width, height, 1000.0, 20.0, 11);
        let deep = frame(width, height, 1000.0, 10.0, 13);
        let shallow = measure(view(&shallow, &coverage_four, width, height, 4)).expect("shallow");
        let deep = measure(view(&deep, &coverage_sixteen, width, height, 16)).expect("deep");
        let ratio = deep.noise / shallow.noise;
        assert!((ratio - 0.5).abs() < 0.05, "ratio {ratio}");
    }

    #[test]
    fn uncovered_samples_are_not_measured() {
        let (width, height) = (400, 400);
        let data = frame(width, height, 1000.0, 12.0, 17);
        // Half the frame never had a frame land on it. Its samples are zero
        // and would read as an enormous noise if they were counted.
        let mut coverage = vec![4u32; data.len()];
        for (index, count) in coverage.iter_mut().enumerate() {
            if index % width >= width / 2 {
                *count = 0;
            }
        }
        let sample = measure(view(&data, &coverage, width, height, 4)).expect("measurable");
        assert!((sample.noise - 12.0).abs() < 0.8, "{}", sample.noise);
    }

    #[test]
    fn too_little_coverage_is_not_measured() {
        let (width, height) = (20, 20);
        let data = frame(width, height, 1000.0, 12.0, 19);
        let coverage = vec![1u32; data.len()];
        assert!(measure(view(&data, &coverage, width, height, 1)).is_none());
    }

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
    fn two_points_are_not_a_curve() {
        let points = curve(|n| 20.0 / f64::from(n).sqrt(), &[1, 2]);
        assert!(analyze(&points).is_none());
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
        let progressive = ProgressiveSnr::new(StackFrameOrder::Capture, points);
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
