use serde::{Deserialize, Serialize};

/// Issue categories for quality problems detected in image sequences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueCategory {
    LikelyClouds,
    PossibleObstruction,
    FocusDrift,
    TrackingError,
    WindShake,
    SkyBrightening,
    UnknownDegradation,
}

/// Per-image normalized metric values (0.0 = worst in sequence, 1.0 = best).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedMetrics {
    pub star_count: Option<f64>,
    pub hfr: Option<f64>,
    pub eccentricity: Option<f64>,
    pub snr: Option<f64>,
    pub background: Option<f64>,
}

/// Quality analysis result for a single image within its sequence context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageQualityResult {
    pub image_id: i32,
    pub quality_score: f64,
    pub temporal_anomaly_score: f64,
    pub category: Option<IssueCategory>,
    pub normalized_metrics: NormalizedMetrics,
    pub details: Option<String>,
}

/// Reference values representing the best metrics observed in a sequence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceValues {
    pub best_star_count: Option<f64>,
    pub best_hfr: Option<f64>,
    pub best_eccentricity: Option<f64>,
    pub best_snr: Option<f64>,
    pub best_background: Option<f64>,
}

/// Summary statistics for a scored sequence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequenceSummary {
    pub excellent_count: usize,
    pub good_count: usize,
    pub fair_count: usize,
    pub poor_count: usize,
    pub bad_count: usize,
    pub cloud_events_detected: usize,
    pub focus_drift_detected: bool,
    pub tracking_issues_detected: bool,
}

/// A scored sequence of images sharing the same target, filter, and session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredSequence {
    pub target_id: i32,
    pub target_name: String,
    pub filter_name: String,
    pub session_start: Option<i64>,
    pub session_end: Option<i64>,
    pub image_count: usize,
    pub reference_values: ReferenceValues,
    pub images: Vec<ImageQualityResult>,
    pub summary: SequenceSummary,
}

/// Raw metric values extracted from an image's metadata for analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageMetrics {
    pub image_id: i32,
    pub timestamp: Option<i64>,
    pub star_count: Option<f64>,
    pub hfr: Option<f64>,
    pub eccentricity: Option<f64>,
    pub snr: Option<f64>,
    pub background: Option<f64>,
}

/// Configurable weights for composite quality scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityWeights {
    pub star_count: f64,
    pub hfr: f64,
    pub eccentricity: f64,
    pub snr: f64,
    pub background: f64,
}

impl Default for QualityWeights {
    fn default() -> Self {
        Self {
            star_count: 0.30,
            hfr: 0.25,
            eccentricity: 0.10,
            snr: 0.25,
            background: 0.10,
        }
    }
}

impl QualityWeights {
    /// Normalize weights so they sum to 1.0. If all weights are zero, returns defaults.
    pub fn normalized(self) -> Self {
        let sum = self.star_count + self.hfr + self.eccentricity + self.snr + self.background;
        if sum < 1e-10 {
            return Self::default();
        }
        if (sum - 1.0).abs() < 1e-10 {
            return self;
        }
        Self {
            star_count: self.star_count / sum,
            hfr: self.hfr / sum,
            eccentricity: self.eccentricity / sum,
            snr: self.snr / sum,
            background: self.background / sum,
        }
    }
}

/// Configurable weights for temporal anomaly detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalWeights {
    pub star_count: f64,
    pub background: f64,
    pub hfr: f64,
    pub snr: f64,
}

impl Default for TemporalWeights {
    fn default() -> Self {
        Self {
            star_count: 0.40,
            background: 0.25,
            hfr: 0.20,
            snr: 0.15,
        }
    }
}

/// Configuration for the sequence analyzer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequenceAnalyzerConfig {
    pub session_gap_minutes: u64,
    pub min_sequence_length: usize,
    pub ewma_alpha: f64,
    pub quality_weights: QualityWeights,
    pub temporal_weights: TemporalWeights,
    pub star_drop_threshold: f64,
    pub bg_rise_threshold: f64,
    pub hfr_rise_threshold: f64,
    pub sudden_change_rate: f64,
}

impl Default for SequenceAnalyzerConfig {
    fn default() -> Self {
        Self {
            session_gap_minutes: 60,
            min_sequence_length: 3,
            ewma_alpha: 0.3,
            quality_weights: QualityWeights::default(),
            temporal_weights: TemporalWeights::default(),
            star_drop_threshold: 0.25,
            bg_rise_threshold: 0.10,
            hfr_rise_threshold: 0.15,
            sudden_change_rate: 0.15,
        }
    }
}

/// Analyzer that scores image quality within acquisition sequences.
pub struct SequenceAnalyzer {
    config: SequenceAnalyzerConfig,
}

impl SequenceAnalyzer {
    pub fn new(mut config: SequenceAnalyzerConfig) -> Self {
        config.quality_weights = config.quality_weights.normalized();
        Self { config }
    }

    /// Analyze a set of images, grouping them into sequences and scoring each.
    /// All images should share the same target and filter.
    pub fn analyze(
        &self,
        images: &[ImageMetrics],
        target_id: i32,
        target_name: &str,
        filter_name: &str,
    ) -> Vec<ScoredSequence> {
        let sequences = self.split_into_sequences(images);

        sequences
            .into_iter()
            .map(|seq| self.score_sequence(seq, target_id, target_name, filter_name))
            .collect()
    }

    /// Split a time-ordered list of images into contiguous sessions.
    fn split_into_sequences(&self, images: &[ImageMetrics]) -> Vec<Vec<ImageMetrics>> {
        if images.is_empty() {
            return vec![];
        }

        let mut sorted: Vec<ImageMetrics> = images.to_vec();
        sorted.sort_by_key(|img| img.timestamp.unwrap_or(0));

        let gap_seconds = (self.config.session_gap_minutes * 60) as i64;
        let mut sequences: Vec<Vec<ImageMetrics>> = Vec::new();
        let mut current_seq: Vec<ImageMetrics> = vec![sorted[0].clone()];

        for img in sorted.iter().skip(1) {
            let prev_ts = current_seq
                .last()
                .and_then(|prev| prev.timestamp)
                .unwrap_or(0);
            let curr_ts = img.timestamp.unwrap_or(0);

            if curr_ts - prev_ts > gap_seconds {
                sequences.push(std::mem::take(&mut current_seq));
            }
            current_seq.push(img.clone());
        }
        if !current_seq.is_empty() {
            sequences.push(current_seq);
        }

        sequences
    }

    /// Score a single sequence of images.
    fn score_sequence(
        &self,
        images: Vec<ImageMetrics>,
        target_id: i32,
        target_name: &str,
        filter_name: &str,
    ) -> ScoredSequence {
        let image_count = images.len();

        let session_start = images.first().and_then(|i| i.timestamp);
        let session_end = images.last().and_then(|i| i.timestamp);

        // If sequence is too short, return with score 1.0 for all images
        if image_count < self.config.min_sequence_length {
            let results: Vec<ImageQualityResult> = images
                .iter()
                .map(|img| ImageQualityResult {
                    image_id: img.image_id,
                    quality_score: 1.0,
                    temporal_anomaly_score: 0.0,
                    category: None,
                    normalized_metrics: NormalizedMetrics {
                        star_count: Some(1.0),
                        hfr: Some(1.0),
                        eccentricity: Some(1.0),
                        snr: Some(1.0),
                        background: Some(1.0),
                    },
                    details: None,
                })
                .collect();

            return ScoredSequence {
                target_id,
                target_name: target_name.to_string(),
                filter_name: filter_name.to_string(),
                session_start,
                session_end,
                image_count,
                reference_values: ReferenceValues {
                    best_star_count: None,
                    best_hfr: None,
                    best_eccentricity: None,
                    best_snr: None,
                    best_background: None,
                },
                images: results,
                summary: SequenceSummary {
                    excellent_count: image_count,
                    good_count: 0,
                    fair_count: 0,
                    poor_count: 0,
                    bad_count: 0,
                    cloud_events_detected: 0,
                    focus_drift_detected: false,
                    tracking_issues_detected: false,
                },
            };
        }

        // Normalize each metric
        let norm_stars = self.normalize_metric_higher_better(
            &images.iter().map(|i| i.star_count).collect::<Vec<_>>(),
        );
        let norm_hfr =
            self.normalize_metric_lower_better(&images.iter().map(|i| i.hfr).collect::<Vec<_>>());
        let norm_ecc = self.normalize_metric_lower_better(
            &images.iter().map(|i| i.eccentricity).collect::<Vec<_>>(),
        );
        let norm_snr =
            self.normalize_metric_higher_better(&images.iter().map(|i| i.snr).collect::<Vec<_>>());
        let norm_bg = self.normalize_metric_lower_better(
            &images.iter().map(|i| i.background).collect::<Vec<_>>(),
        );

        // Compute EWMA temporal deviation scores
        let temporal_scores = self.compute_temporal_scores(&images);

        // Compute composite quality scores
        let w = &self.config.quality_weights;
        let mut results: Vec<ImageQualityResult> = Vec::with_capacity(image_count);

        for i in 0..image_count {
            let ns = norm_stars[i];
            let nh = norm_hfr[i];
            let ne = norm_ecc[i];
            let nsn = norm_snr[i];
            let nb = norm_bg[i];

            // Weighted sum using available metrics
            let (score, total_weight) = weighted_sum_available(&[
                (ns, w.star_count),
                (nh, w.hfr),
                (ne, w.eccentricity),
                (nsn, w.snr),
                (nb, w.background),
            ]);

            let quality_score = if total_weight > 0.0 {
                score / total_weight
            } else {
                1.0
            };

            // Apply temporal penalty
            let temporal = temporal_scores[i];
            let penalty = 1.0 - temporal.min(0.5);
            let final_score = (quality_score * penalty).clamp(0.0, 1.0);

            results.push(ImageQualityResult {
                image_id: images[i].image_id,
                quality_score: final_score,
                temporal_anomaly_score: temporal,
                category: None, // Classified below
                normalized_metrics: NormalizedMetrics {
                    star_count: ns,
                    hfr: nh,
                    eccentricity: ne,
                    snr: nsn,
                    background: nb,
                },
                details: None,
            });
        }

        // Classify issues
        self.classify_issues(&mut results, &images);

        // Build reference values
        let reference_values = ReferenceValues {
            best_star_count: best_value(&images, |i| i.star_count, true),
            best_hfr: best_value(&images, |i| i.hfr, false),
            best_eccentricity: best_value(&images, |i| i.eccentricity, false),
            best_snr: best_value(&images, |i| i.snr, true),
            best_background: best_value(&images, |i| i.background, false),
        };

        // Build summary
        let summary = self.build_summary(&results);

        ScoredSequence {
            target_id,
            target_name: target_name.to_string(),
            filter_name: filter_name.to_string(),
            session_start,
            session_end,
            image_count,
            reference_values,
            images: results,
            summary,
        }
    }

    /// Normalize values where higher is better (e.g. star count, SNR).
    /// Uses 5th/95th percentile bounds for robustness.
    fn normalize_metric_higher_better(&self, values: &[Option<f64>]) -> Vec<Option<f64>> {
        let valid: Vec<f64> = values.iter().filter_map(|v| *v).collect();
        if valid.is_empty() {
            return values.iter().map(|_| None).collect();
        }

        let (p5, p95) = percentile_bounds(&valid);
        if (p95 - p5).abs() < f64::EPSILON {
            return values.iter().map(|v| v.map(|_| 1.0)).collect();
        }

        values
            .iter()
            .map(|v| v.map(|val| ((val - p5) / (p95 - p5)).clamp(0.0, 1.0)))
            .collect()
    }

    /// Normalize values where lower is better (e.g. HFR, background).
    /// Uses 5th/95th percentile bounds for robustness.
    fn normalize_metric_lower_better(&self, values: &[Option<f64>]) -> Vec<Option<f64>> {
        let valid: Vec<f64> = values.iter().filter_map(|v| *v).collect();
        if valid.is_empty() {
            return values.iter().map(|_| None).collect();
        }

        let (p5, p95) = percentile_bounds(&valid);
        if (p95 - p5).abs() < f64::EPSILON {
            return values.iter().map(|v| v.map(|_| 1.0)).collect();
        }

        values
            .iter()
            .map(|v| v.map(|val| ((p95 - val) / (p95 - p5)).clamp(0.0, 1.0)))
            .collect()
    }

    /// Compute EWMA-based temporal deviation scores for the sequence.
    fn compute_temporal_scores(&self, images: &[ImageMetrics]) -> Vec<f64> {
        let alpha = self.config.ewma_alpha;
        let tw = &self.config.temporal_weights;
        let n = images.len();
        let mut scores = vec![0.0f64; n];

        // EWMA baselines for each metric
        let mut bl_stars: Option<f64> = None;
        let mut bl_bg: Option<f64> = None;
        let mut bl_hfr: Option<f64> = None;
        let mut bl_snr: Option<f64> = None;

        for i in 0..n {
            let img = &images[i];

            const EPSILON: f64 = 1e-10;

            // Star count deviation (drop is bad)
            let star_dev = if let (Some(val), Some(bl)) = (img.star_count, bl_stars) {
                if bl.abs() > EPSILON {
                    ((bl - val) / bl).max(0.0)
                } else {
                    0.0
                }
            } else {
                0.0
            };

            // Background deviation (rise is bad)
            let bg_dev = if let (Some(val), Some(bl)) = (img.background, bl_bg) {
                if bl.abs() > EPSILON {
                    ((val - bl) / bl).max(0.0)
                } else {
                    0.0
                }
            } else {
                0.0
            };

            // HFR deviation (rise is bad)
            let hfr_dev = if let (Some(val), Some(bl)) = (img.hfr, bl_hfr) {
                if bl.abs() > EPSILON {
                    ((val - bl) / bl).max(0.0)
                } else {
                    0.0
                }
            } else {
                0.0
            };

            // SNR deviation (drop is bad, so negate)
            let snr_dev = if let (Some(val), Some(bl)) = (img.snr, bl_snr) {
                if bl.abs() > EPSILON {
                    ((bl - val) / bl).max(0.0)
                } else {
                    0.0
                }
            } else {
                0.0
            };

            scores[i] = tw.star_count * star_dev
                + tw.background * bg_dev
                + tw.hfr * hfr_dev
                + tw.snr * snr_dev;

            // Update EWMA baselines
            if let Some(val) = img.star_count {
                bl_stars = Some(match bl_stars {
                    Some(bl) => alpha * val + (1.0 - alpha) * bl,
                    None => val,
                });
            }
            if let Some(val) = img.background {
                bl_bg = Some(match bl_bg {
                    Some(bl) => alpha * val + (1.0 - alpha) * bl,
                    None => val,
                });
            }
            if let Some(val) = img.hfr {
                bl_hfr = Some(match bl_hfr {
                    Some(bl) => alpha * val + (1.0 - alpha) * bl,
                    None => val,
                });
            }
            if let Some(val) = img.snr {
                bl_snr = Some(match bl_snr {
                    Some(bl) => alpha * val + (1.0 - alpha) * bl,
                    None => val,
                });
            }
        }

        scores
    }

    /// Classify issues for each image based on metric deviations.
    fn classify_issues(&self, results: &mut [ImageQualityResult], images: &[ImageMetrics]) {
        let n = images.len();
        if n < 2 {
            return;
        }

        #[allow(clippy::needless_range_loop)]
        for i in 0..n {
            if results[i].quality_score >= 0.7 {
                continue; // No classification needed for good frames
            }

            let star_drop = self.compute_fractional_drop(images, i, |m| m.star_count);
            let bg_rise = self.compute_fractional_rise(images, i, |m| m.background);
            let hfr_rise = self.compute_fractional_rise(images, i, |m| m.hfr);
            let ecc_rise = self.compute_fractional_rise(images, i, |m| m.eccentricity);

            let is_gradual_hfr = self.is_gradual_change(images, i, |m| m.hfr, 3);
            let is_gradual_bg = self.is_gradual_change(images, i, |m| m.background, 3);

            let star_stable = star_drop < self.config.star_drop_threshold;
            let bg_stable = bg_rise < self.config.bg_rise_threshold;
            let ecc_stable = ecc_rise < 0.15;

            // Classification rules from the design document
            let (category, details) = if star_drop > self.config.star_drop_threshold
                && bg_rise > self.config.bg_rise_threshold
            {
                (
                    Some(IssueCategory::LikelyClouds),
                    Some(format!(
                        "Star count dropped {:.0}% from baseline while background increased {:.0}%. Pattern consistent with cloud passage.",
                        star_drop * 100.0,
                        bg_rise * 100.0
                    )),
                )
            } else if star_drop > self.config.star_drop_threshold && bg_stable {
                (
                    Some(IssueCategory::PossibleObstruction),
                    Some(format!(
                        "Star count dropped {:.0}% with stable background. Possible obstruction (tree, dome slit, dew cap).",
                        star_drop * 100.0
                    )),
                )
            } else if hfr_rise > self.config.hfr_rise_threshold && is_gradual_hfr && ecc_stable {
                (
                    Some(IssueCategory::FocusDrift),
                    Some(format!(
                        "HFR increased {:.0}% gradually over multiple frames with stable eccentricity. Consistent with focus drift.",
                        hfr_rise * 100.0
                    )),
                )
            } else if ecc_rise > 0.15 && star_stable {
                (
                    Some(IssueCategory::TrackingError),
                    Some(format!(
                        "Eccentricity increased by {:.2} with stable star count. Consistent with tracking/guiding error.",
                        ecc_rise
                    )),
                )
            } else if hfr_rise > self.config.hfr_rise_threshold
                && star_drop > self.config.star_drop_threshold
                && !ecc_stable
            {
                (
                    Some(IssueCategory::WindShake),
                    Some("HFR increased, star count dropped, and eccentricity changed. Consistent with wind shake affecting guiding and seeing.".to_string()),
                )
            } else if bg_rise > self.config.bg_rise_threshold && is_gradual_bg && star_stable {
                (
                    Some(IssueCategory::SkyBrightening),
                    Some(format!(
                        "Background increased {:.0}% gradually with stable star count. Consistent with sky brightening (dawn, moon rise).",
                        bg_rise * 100.0
                    )),
                )
            } else if results[i].quality_score < 0.5 {
                (
                    Some(IssueCategory::UnknownDegradation),
                    Some(
                        "Quality degraded but no clear pattern matches known issue types."
                            .to_string(),
                    ),
                )
            } else {
                (None, None)
            };

            results[i].category = category;
            results[i].details = details;
        }
    }

    /// Compute fractional drop relative to a local baseline (preceding frames).
    fn compute_fractional_drop(
        &self,
        images: &[ImageMetrics],
        idx: usize,
        f: impl Fn(&ImageMetrics) -> Option<f64>,
    ) -> f64 {
        let current = match f(&images[idx]) {
            Some(v) => v,
            None => return 0.0,
        };

        let baseline = self.local_baseline(images, idx, &f);
        if baseline.abs() < 1e-10 {
            return 0.0;
        }
        ((baseline - current) / baseline).max(0.0)
    }

    /// Compute fractional rise relative to a local baseline (preceding frames).
    fn compute_fractional_rise(
        &self,
        images: &[ImageMetrics],
        idx: usize,
        f: impl Fn(&ImageMetrics) -> Option<f64>,
    ) -> f64 {
        let current = match f(&images[idx]) {
            Some(v) => v,
            None => return 0.0,
        };

        let baseline = self.local_baseline(images, idx, &f);
        if baseline.abs() < 1e-10 {
            return 0.0;
        }
        ((current - baseline) / baseline).max(0.0)
    }

    /// Compute local baseline as median of up to 5 preceding frames.
    fn local_baseline(
        &self,
        images: &[ImageMetrics],
        idx: usize,
        f: &impl Fn(&ImageMetrics) -> Option<f64>,
    ) -> f64 {
        let start = idx.saturating_sub(5);
        let mut vals: Vec<f64> = (start..idx).filter_map(|j| f(&images[j])).collect();
        if vals.is_empty() {
            return f(&images[idx]).unwrap_or(0.0);
        }
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        vals[vals.len() / 2]
    }

    /// Check if a metric changed gradually over the last `window` frames.
    fn is_gradual_change(
        &self,
        images: &[ImageMetrics],
        idx: usize,
        f: impl Fn(&ImageMetrics) -> Option<f64>,
        window: usize,
    ) -> bool {
        if idx < window {
            return false;
        }

        let mut consecutive_small = 0;
        for j in (idx - window + 1)..=idx {
            let prev = match f(&images[j - 1]) {
                Some(v) if v > 0.0 => v,
                _ => continue,
            };
            let curr = match f(&images[j]) {
                Some(v) => v,
                None => continue,
            };
            let rate = ((curr - prev) / prev).abs();
            if rate < self.config.sudden_change_rate {
                consecutive_small += 1;
            }
        }

        consecutive_small >= window - 1
    }

    /// Build summary from scored results.
    fn build_summary(&self, results: &[ImageQualityResult]) -> SequenceSummary {
        let mut summary = SequenceSummary {
            excellent_count: 0,
            good_count: 0,
            fair_count: 0,
            poor_count: 0,
            bad_count: 0,
            cloud_events_detected: 0,
            focus_drift_detected: false,
            tracking_issues_detected: false,
        };

        for r in results {
            match r.quality_score {
                s if s >= 0.90 => summary.excellent_count += 1,
                s if s >= 0.70 => summary.good_count += 1,
                s if s >= 0.50 => summary.fair_count += 1,
                s if s >= 0.30 => summary.poor_count += 1,
                _ => summary.bad_count += 1,
            }

            match &r.category {
                Some(IssueCategory::LikelyClouds) => summary.cloud_events_detected += 1,
                Some(IssueCategory::FocusDrift) => summary.focus_drift_detected = true,
                Some(IssueCategory::TrackingError) => summary.tracking_issues_detected = true,
                _ => {}
            }
        }

        summary
    }
}

/// Compute the 5th and 95th percentile values from a slice.
fn percentile_bounds(values: &[f64]) -> (f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    let p5_idx = ((n as f64 * 0.05).floor() as usize).min(n - 1);
    let p95_idx = ((n as f64 * 0.95).ceil() as usize).min(n - 1);
    (sorted[p5_idx], sorted[p95_idx])
}

/// Weighted sum of available (non-None) metric values.
fn weighted_sum_available(items: &[(Option<f64>, f64)]) -> (f64, f64) {
    let mut sum = 0.0;
    let mut total_weight = 0.0;
    for (value, weight) in items {
        if let Some(v) = value {
            sum += v * weight;
            total_weight += weight;
        }
    }
    (sum, total_weight)
}

/// Find the best value for a metric across images (max if higher_better, min otherwise).
fn best_value(
    images: &[ImageMetrics],
    f: impl Fn(&ImageMetrics) -> Option<f64>,
    higher_better: bool,
) -> Option<f64> {
    let vals: Vec<f64> = images.iter().filter_map(&f).collect();
    if vals.is_empty() {
        return None;
    }
    if higher_better {
        vals.into_iter()
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    } else {
        vals.into_iter()
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    }
}

/// Parse image metrics from an AcquiredImage's metadata JSON.
pub fn extract_metrics_from_metadata(
    image_id: i32,
    metadata_json: &str,
    acquired_date: Option<i64>,
) -> ImageMetrics {
    let metadata: serde_json::Value =
        serde_json::from_str(metadata_json).unwrap_or(serde_json::Value::Null);

    // Use acquireddate (i64 from DB) as primary sort key; fall back to parsing
    // ExposureStartTime from metadata JSON if acquireddate is NULL
    let timestamp = acquired_date.or_else(|| {
        metadata["ExposureStartTime"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp())
    });

    let star_count = metadata["DetectedStars"]
        .as_f64()
        .or_else(|| metadata["DetectedStars"].as_i64().map(|v| v as f64));

    let hfr = metadata["HFR"].as_f64();

    let eccentricity = metadata["Eccentricity"].as_f64();
    let snr = metadata["SNR"].as_f64();

    // Background can be stored under several keys
    let background = metadata["Background"]
        .as_f64()
        .or_else(|| metadata["Median"].as_f64());

    ImageMetrics {
        image_id,
        timestamp,
        star_count,
        hfr,
        eccentricity,
        snr,
        background,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_image(id: i32, ts: i64, stars: f64, hfr: f64) -> ImageMetrics {
        ImageMetrics {
            image_id: id,
            timestamp: Some(ts),
            star_count: Some(stars),
            hfr: Some(hfr),
            eccentricity: None,
            snr: None,
            background: None,
        }
    }

    fn make_full_image(
        id: i32,
        ts: i64,
        stars: f64,
        hfr: f64,
        bg: f64,
        snr: f64,
        ecc: f64,
    ) -> ImageMetrics {
        ImageMetrics {
            image_id: id,
            timestamp: Some(ts),
            star_count: Some(stars),
            hfr: Some(hfr),
            eccentricity: Some(ecc),
            snr: Some(snr),
            background: Some(bg),
        }
    }

    #[test]
    fn test_percentile_bounds_basic() {
        let values: Vec<f64> = (1..=100).map(|x| x as f64).collect();
        let (p5, p95) = percentile_bounds(&values);
        assert!(p5 <= 6.0, "p5 should be around 5: got {}", p5);
        assert!(p95 >= 95.0, "p95 should be around 95: got {}", p95);
    }

    #[test]
    fn test_percentile_bounds_identical() {
        let values = vec![5.0, 5.0, 5.0, 5.0];
        let (p5, p95) = percentile_bounds(&values);
        assert_eq!(p5, 5.0);
        assert_eq!(p95, 5.0);
    }

    #[test]
    fn test_percentile_bounds_empty() {
        let values: Vec<f64> = vec![];
        let (p5, p95) = percentile_bounds(&values);
        assert_eq!(p5, 0.0);
        assert_eq!(p95, 0.0);
    }

    #[test]
    fn test_normalize_higher_better() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let values = vec![
            Some(100.0),
            Some(200.0),
            Some(300.0),
            Some(400.0),
            Some(500.0),
        ];
        let normalized = analyzer.normalize_metric_higher_better(&values);

        // 500 should be the best (1.0), 100 the worst (0.0)
        assert!(normalized[4].unwrap() > normalized[0].unwrap());
        // The best value should be close to 1.0
        assert!(normalized[4].unwrap() >= 0.9);
    }

    #[test]
    fn test_normalize_lower_better() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let values = vec![Some(1.0), Some(2.0), Some(3.0), Some(4.0), Some(5.0)];
        let normalized = analyzer.normalize_metric_lower_better(&values);

        // 1.0 should be the best (close to 1.0), 5.0 the worst (close to 0.0)
        assert!(normalized[0].unwrap() > normalized[4].unwrap());
        assert!(normalized[0].unwrap() >= 0.9);
    }

    #[test]
    fn test_normalize_all_identical() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let values = vec![Some(3.0), Some(3.0), Some(3.0)];
        let normalized = analyzer.normalize_metric_higher_better(&values);
        for v in &normalized {
            assert_eq!(v.unwrap(), 1.0);
        }
    }

    #[test]
    fn test_normalize_with_nones() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let values = vec![Some(100.0), None, Some(300.0), None, Some(500.0)];
        let normalized = analyzer.normalize_metric_higher_better(&values);

        assert!(normalized[0].is_some());
        assert!(normalized[1].is_none());
        assert!(normalized[2].is_some());
        assert!(normalized[3].is_none());
        assert!(normalized[4].is_some());
    }

    #[test]
    fn test_ewma_temporal_scores_stable() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let images: Vec<ImageMetrics> = (0..10)
            .map(|i| make_image(i, i as i64 * 300, 300.0, 2.5))
            .collect();

        let scores = analyzer.compute_temporal_scores(&images);
        // All frames are identical, so temporal scores should be 0
        for s in &scores {
            assert!(
                *s < 0.01,
                "Stable sequence should have near-zero temporal score: {}",
                s
            );
        }
    }

    #[test]
    fn test_ewma_temporal_scores_cloud_event() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let mut images: Vec<ImageMetrics> = (0..10)
            .map(|i| make_image(i, i as i64 * 300, 300.0, 2.5))
            .collect();

        // Simulate cloud event at frame 7: star count drops 50%, HFR rises 30%
        images[7].star_count = Some(150.0);
        images[7].hfr = Some(3.25);

        let scores = analyzer.compute_temporal_scores(&images);
        // Frame 7 should have a significantly higher temporal score
        assert!(
            scores[7] > scores[5],
            "Cloud event frame should have higher temporal score: {} vs {}",
            scores[7],
            scores[5]
        );
        assert!(
            scores[7] > 0.1,
            "Cloud event should produce notable temporal score: {}",
            scores[7]
        );
    }

    #[test]
    fn test_sequence_splitting_single_session() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let images: Vec<ImageMetrics> = (0..10)
            .map(|i| make_image(i, 1000 + i as i64 * 300, 300.0, 2.5)) // 5-min gaps
            .collect();

        let sequences = analyzer.split_into_sequences(&images);
        assert_eq!(sequences.len(), 1, "Should be a single session");
        assert_eq!(sequences[0].len(), 10);
    }

    #[test]
    fn test_sequence_splitting_two_sessions() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let mut images: Vec<ImageMetrics> = (0..5)
            .map(|i| make_image(i, 1000 + i as i64 * 300, 300.0, 2.5))
            .collect();

        // Second session 2 hours later
        for i in 5..10 {
            images.push(make_image(
                i,
                1000 + 5 * 300 + 7200 + (i as i64 - 5) * 300,
                300.0,
                2.5,
            ));
        }

        let sequences = analyzer.split_into_sequences(&images);
        assert_eq!(sequences.len(), 2, "Should split into two sessions");
        assert_eq!(sequences[0].len(), 5);
        assert_eq!(sequences[1].len(), 5);
    }

    #[test]
    fn test_scoring_good_sequence() {
        let config = SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        };
        let analyzer = SequenceAnalyzer::new(config);

        // All good frames clustered tightly around the same values
        let images: Vec<ImageMetrics> = (0..10)
            .map(|i| {
                // Small oscillation around center values
                let offset = if i % 2 == 0 { 1.0 } else { -1.0 };
                make_full_image(
                    i,
                    i as i64 * 300,
                    300.0 + offset * 3.0,  // Tight star count variation
                    2.5 + offset * 0.02,   // Tight HFR variation
                    1200.0 + offset * 5.0, // Tight background variation
                    45.0 + offset * 0.3,   // Tight SNR variation
                    0.35,                  // Constant eccentricity
                )
            })
            .collect();

        let results = analyzer.analyze(&images, 1, "M42", "L");
        assert_eq!(results.len(), 1);
        let seq = &results[0];
        assert_eq!(seq.image_count, 10);

        // All frames should score reasonably since they're all similar
        // With tight clustering, even after normalization scores should not be extremely low
        let above_fair = seq.images.iter().filter(|r| r.quality_score >= 0.3).count();
        assert!(
            above_fair >= 8,
            "Most frames in a tight sequence should score above fair: {} out of 10",
            above_fair
        );
    }

    #[test]
    fn test_scoring_with_cloud_event() {
        let config = SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        };
        let analyzer = SequenceAnalyzer::new(config);

        let mut images: Vec<ImageMetrics> = (0..10)
            .map(|i| make_full_image(i, i as i64 * 300, 300.0, 2.5, 1200.0, 45.0, 0.35))
            .collect();

        // Cloud event at frame 7
        images[7] = make_full_image(7, 7 * 300, 100.0, 4.0, 1800.0, 15.0, 0.35);
        // Another cloud frame
        images[8] = make_full_image(8, 8 * 300, 80.0, 4.5, 2000.0, 10.0, 0.35);

        let results = analyzer.analyze(&images, 1, "M42", "L");
        assert_eq!(results.len(), 1);
        let seq = &results[0];

        // Cloud-affected frames should score worse than good frames
        let good_score = seq.images[3].quality_score;
        let cloud_score = seq.images[7].quality_score;
        assert!(
            cloud_score < good_score,
            "Cloud frame should score worse: {} vs {}",
            cloud_score,
            good_score
        );

        // Cloud frame should be classified
        assert!(
            seq.images[7].category.is_some(),
            "Cloud frame should have a category"
        );
    }

    #[test]
    fn test_classify_likely_clouds() {
        let config = SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        };
        let analyzer = SequenceAnalyzer::new(config);

        let mut images: Vec<ImageMetrics> = (0..8)
            .map(|i| make_full_image(i, i as i64 * 300, 300.0, 2.5, 1200.0, 45.0, 0.35))
            .collect();

        // Frame 6: star drop + background rise = likely clouds
        images[6] = make_full_image(6, 6 * 300, 100.0, 3.5, 1800.0, 15.0, 0.35);

        let results = analyzer.analyze(&images, 1, "test", "L");
        let seq = &results[0];

        let cloud_frame = &seq.images[6];
        assert_eq!(
            cloud_frame.category,
            Some(IssueCategory::LikelyClouds),
            "Should classify as LikelyClouds, got {:?}",
            cloud_frame.category
        );
    }

    #[test]
    fn test_classify_possible_obstruction() {
        let config = SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        };
        let analyzer = SequenceAnalyzer::new(config);

        let mut images: Vec<ImageMetrics> = (0..8)
            .map(|i| make_full_image(i, i as i64 * 300, 300.0, 2.5, 1200.0, 45.0, 0.35))
            .collect();

        // Frame 6: star drop but stable background = obstruction
        images[6] = make_full_image(6, 6 * 300, 100.0, 2.6, 1200.0, 40.0, 0.35);

        let results = analyzer.analyze(&images, 1, "test", "L");
        let seq = &results[0];

        let obst_frame = &seq.images[6];
        assert_eq!(
            obst_frame.category,
            Some(IssueCategory::PossibleObstruction),
            "Should classify as PossibleObstruction, got {:?}",
            obst_frame.category
        );
    }

    #[test]
    fn test_short_sequence_returns_perfect_scores() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        // 2 images is below min_sequence_length of 3
        let images: Vec<ImageMetrics> = (0..2)
            .map(|i| make_image(i, i as i64 * 300, 300.0, 2.5))
            .collect();

        let results = analyzer.analyze(&images, 1, "test", "L");
        assert_eq!(results.len(), 1);

        for img in &results[0].images {
            assert_eq!(
                img.quality_score, 1.0,
                "Short sequence images should get 1.0"
            );
        }
    }

    #[test]
    fn test_empty_input() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let images: Vec<ImageMetrics> = vec![];
        let results = analyzer.analyze(&images, 1, "test", "L");
        assert!(results.is_empty());
    }

    #[test]
    fn test_weighted_sum_available() {
        let items = [
            (Some(0.8), 0.3),
            (None, 0.25),
            (Some(0.6), 0.10),
            (None, 0.25),
            (Some(0.9), 0.10),
        ];
        let (sum, weight) = weighted_sum_available(&items);
        let expected_sum = 0.8 * 0.3 + 0.6 * 0.10 + 0.9 * 0.10;
        let expected_weight = 0.3 + 0.10 + 0.10;
        assert!((sum - expected_sum).abs() < 1e-10);
        assert!((weight - expected_weight).abs() < 1e-10);
    }

    #[test]
    fn test_extract_metrics_from_metadata() {
        let json = r#"{
            "FileName": "test.fits",
            "FilterName": "Ha",
            "HFR": 2.5,
            "DetectedStars": 342,
            "ExposureStartTime": "2024-01-15T22:00:00Z"
        }"#;

        let metrics = extract_metrics_from_metadata(1, json, None);
        assert_eq!(metrics.image_id, 1);
        assert_eq!(metrics.star_count, Some(342.0));
        assert_eq!(metrics.hfr, Some(2.5));
        assert!(metrics.timestamp.is_some());
        assert!(metrics.eccentricity.is_none());
        assert!(metrics.snr.is_none());
        assert!(metrics.background.is_none());
    }

    #[test]
    fn test_extract_metrics_acquireddate_preferred_over_exposure_start() {
        // When both acquireddate and ExposureStartTime exist, acquireddate takes priority
        let json = r#"{
            "FileName": "test.fits",
            "ExposureStartTime": "2024-01-15T22:00:00Z"
        }"#;
        let acquired_ts = 1705400000_i64; // Different from ExposureStartTime
        let metrics = extract_metrics_from_metadata(1, json, Some(acquired_ts));
        assert_eq!(metrics.timestamp, Some(acquired_ts));
    }

    #[test]
    fn test_extract_metrics_fallback_to_exposure_start_time() {
        // When acquireddate is NULL, fall back to ExposureStartTime
        let json = r#"{
            "FileName": "test.fits",
            "ExposureStartTime": "2024-01-15T22:00:00Z"
        }"#;
        let metrics = extract_metrics_from_metadata(1, json, None);
        assert!(metrics.timestamp.is_some());
    }

    #[test]
    fn test_extract_metrics_no_timestamp_sources() {
        let json = r#"{"FileName": "test.fits", "FilterName": "Ha"}"#;
        let metrics = extract_metrics_from_metadata(1, json, None);
        assert_eq!(metrics.timestamp, None);
    }

    #[test]
    fn test_extract_metrics_invalid_json() {
        let metrics = extract_metrics_from_metadata(1, "not json", Some(123));
        assert_eq!(metrics.image_id, 1);
        assert_eq!(metrics.timestamp, Some(123));
        assert!(metrics.star_count.is_none());
        assert!(metrics.hfr.is_none());
    }

    #[test]
    fn test_summary_counts() {
        let results = vec![
            ImageQualityResult {
                image_id: 1,
                quality_score: 0.95,
                temporal_anomaly_score: 0.0,
                category: None,
                normalized_metrics: NormalizedMetrics {
                    star_count: Some(1.0),
                    hfr: Some(1.0),
                    eccentricity: None,
                    snr: None,
                    background: None,
                },
                details: None,
            },
            ImageQualityResult {
                image_id: 2,
                quality_score: 0.75,
                temporal_anomaly_score: 0.0,
                category: None,
                normalized_metrics: NormalizedMetrics {
                    star_count: Some(0.8),
                    hfr: Some(0.7),
                    eccentricity: None,
                    snr: None,
                    background: None,
                },
                details: None,
            },
            ImageQualityResult {
                image_id: 3,
                quality_score: 0.25,
                temporal_anomaly_score: 0.4,
                category: Some(IssueCategory::LikelyClouds),
                normalized_metrics: NormalizedMetrics {
                    star_count: Some(0.1),
                    hfr: Some(0.3),
                    eccentricity: None,
                    snr: None,
                    background: None,
                },
                details: Some("Cloud".to_string()),
            },
        ];

        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let summary = analyzer.build_summary(&results);

        assert_eq!(summary.excellent_count, 1);
        assert_eq!(summary.good_count, 1);
        assert_eq!(summary.bad_count, 1);
        assert_eq!(summary.cloud_events_detected, 1);
    }

    #[test]
    fn test_custom_weights() {
        let config = SequenceAnalyzerConfig {
            min_sequence_length: 3,
            quality_weights: QualityWeights {
                star_count: 0.5,
                hfr: 0.5,
                eccentricity: 0.0,
                snr: 0.0,
                background: 0.0,
            },
            ..Default::default()
        };

        let analyzer = SequenceAnalyzer::new(config);
        let images: Vec<ImageMetrics> = (0..6)
            .map(|i| {
                make_image(
                    i,
                    i as i64 * 300,
                    300.0 - i as f64 * 30.0,
                    2.5 + i as f64 * 0.2,
                )
            })
            .collect();

        let results = analyzer.analyze(&images, 1, "test", "L");
        assert_eq!(results.len(), 1);

        // First frame should score better than last (more stars, lower HFR)
        assert!(
            results[0].images[0].quality_score >= results[0].images[5].quality_score,
            "First frame should score >= last: {} vs {}",
            results[0].images[0].quality_score,
            results[0].images[5].quality_score,
        );
    }

    #[test]
    fn test_weight_normalization() {
        let weights = QualityWeights {
            star_count: 2.0,
            hfr: 2.0,
            eccentricity: 1.0,
            snr: 0.0,
            background: 0.0,
        };
        let normalized = weights.normalized();
        let sum = normalized.star_count
            + normalized.hfr
            + normalized.eccentricity
            + normalized.snr
            + normalized.background;
        assert!(
            (sum - 1.0).abs() < 1e-10,
            "Normalized weights should sum to 1.0, got {}",
            sum
        );
        assert!((normalized.star_count - 0.4).abs() < 1e-10);
        assert!((normalized.hfr - 0.4).abs() < 1e-10);
        assert!((normalized.eccentricity - 0.2).abs() < 1e-10);
    }

    #[test]
    fn test_weight_normalization_already_normalized() {
        let weights = QualityWeights::default();
        let normalized = weights.normalized();
        assert!((normalized.star_count - 0.30).abs() < 1e-10);
        assert!((normalized.hfr - 0.25).abs() < 1e-10);
    }

    #[test]
    fn test_weight_normalization_all_zero_returns_defaults() {
        let weights = QualityWeights {
            star_count: 0.0,
            hfr: 0.0,
            eccentricity: 0.0,
            snr: 0.0,
            background: 0.0,
        };
        let normalized = weights.normalized();
        let defaults = QualityWeights::default();
        assert!((normalized.star_count - defaults.star_count).abs() < 1e-10);
    }

    #[test]
    fn test_weight_redistribution_with_missing_metrics() {
        // When some metrics are None, weighted_sum_available divides by total
        // available weight, effectively redistributing proportionally
        let items_all = [
            (Some(0.8), 0.3),
            (Some(0.7), 0.25),
            (Some(0.6), 0.10),
            (Some(0.9), 0.25),
            (Some(0.5), 0.10),
        ];
        let (sum_all, weight_all) = weighted_sum_available(&items_all);
        let score_all = sum_all / weight_all;

        // With two metrics missing
        let items_partial = [
            (Some(0.8), 0.3),
            (Some(0.7), 0.25),
            (None, 0.10),
            (None, 0.25),
            (Some(0.5), 0.10),
        ];
        let (sum_partial, weight_partial) = weighted_sum_available(&items_partial);
        let score_partial = sum_partial / weight_partial;

        // Both should produce a score between 0.0 and 1.0
        assert!((0.0..=1.0).contains(&score_all));
        assert!((0.0..=1.0).contains(&score_partial));
        // Weight redistribution means remaining weights are scaled up proportionally
        assert!(
            (weight_partial - 0.65).abs() < 1e-10,
            "Available weight should be 0.3+0.25+0.10=0.65, got {}",
            weight_partial
        );
    }

    #[test]
    fn test_best_value_higher_better() {
        let images = vec![
            make_image(1, 100, 200.0, 2.5),
            make_image(2, 200, 350.0, 2.3),
            make_image(3, 300, 100.0, 2.8),
        ];
        let best = best_value(&images, |i| i.star_count, true);
        assert_eq!(best, Some(350.0));
    }

    #[test]
    fn test_best_value_lower_better() {
        let images = vec![
            make_image(1, 100, 200.0, 2.5),
            make_image(2, 200, 350.0, 2.3),
            make_image(3, 300, 100.0, 2.8),
        ];
        let best = best_value(&images, |i| i.hfr, false);
        assert_eq!(best, Some(2.3));
    }
}
