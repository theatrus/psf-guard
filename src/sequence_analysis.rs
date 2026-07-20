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
    OffTarget,
    /// The whole segment is consistently displaced from the intended target.
    /// Likely deliberate framing: surfaced as a warning, never auto-rejected.
    StableOffset,
    PointingJump,
    PointingDrift,
    PlateSolveFailed,
    /// A solved single exposure has a predicted sunlit satellite crossing.
    /// This is orbital prediction evidence, not a pixel-trail detection.
    SatelliteTrailRisk,
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
    /// Spatial star coverage: 1.0 = stars across the whole frame, 0.0 = half
    /// or more of the frame's grid cells without stars. Unlike the other
    /// metrics this is an absolute mapping, not sequence-relative: a dead
    /// cell fraction is already dimensionless and comparable across setups.
    #[serde(default)]
    pub spatial_coverage: Option<f64>,
    /// Photometric transparency mapped to 0..1 (1.0 at nominal flux, 0.0 at
    /// <= 60% of reference flux). Absolute mapping like spatial_coverage.
    #[serde(default)]
    pub transparency: Option<f64>,
    /// Absolute pointing quality from a pixel-derived plate solution.
    #[serde(default)]
    pub pointing: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointingQuality {
    pub pixel_solved: bool,
    pub solve_failed: bool,
    pub image_quality_evidence: bool,
    /// True when offsets use an authoritative intended target. False means
    /// they use the sequence's first solved center for relative motion only.
    pub expected_target: bool,
    #[serde(default)]
    pub flags: Vec<IssueCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub east_offset_arcsec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub north_offset_arcsec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub separation_arcsec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_fraction_offset: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_offset_arcsec: Option<f64>,
    /// Residual from the segment's own robust center as a fraction of the
    /// short field axis. For stable-offset segments this, not the absolute
    /// target offset, is what pointing scoring uses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_field_fraction: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift_rate_arcsec_per_hour: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_stars: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rms_arcsec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AstrometryFrameMetrics {
    pub pixel_solved: bool,
    pub solve_failed: bool,
    pub image_quality_evidence: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solved_center_ra_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solved_center_dec_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub east_offset_arcsec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub north_offset_arcsec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub separation_arcsec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_in_frame: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_short_axis_arcsec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_stars: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rms_arcsec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SatelliteFrameMetrics {
    pub predicted_tracks: usize,
    pub potentially_bright_count: usize,
    pub high_risk_count: usize,
    pub maximum_bright_trail_risk: f64,
    #[serde(default)]
    pub pixel_alignment_attempted: bool,
    #[serde(default)]
    pub pixel_aligned_count: usize,
    #[serde(default)]
    pub pixel_aligned_high_risk_count: usize,
    pub reject_recommended: bool,
    pub association: String,
}

impl From<&crate::satellites::SatelliteAnalysis> for SatelliteFrameMetrics {
    fn from(analysis: &crate::satellites::SatelliteAnalysis) -> Self {
        Self {
            predicted_tracks: analysis.risk.track_count,
            potentially_bright_count: analysis.risk.potentially_bright_count,
            high_risk_count: analysis.risk.high_risk_count,
            maximum_bright_trail_risk: analysis.risk.maximum_bright_trail_risk,
            pixel_alignment_attempted: analysis.risk.pixel_alignment_attempted,
            pixel_aligned_count: analysis.risk.pixel_aligned_count,
            pixel_aligned_high_risk_count: analysis.risk.pixel_aligned_high_risk_count,
            reject_recommended: analysis.risk.reject_recommended,
            association: analysis.association.clone(),
        }
    }
}

pub fn astrometry_metrics_from_analysis(
    analysis: &crate::astrometry::AstrometryAnalysis,
) -> Option<AstrometryFrameMetrics> {
    use crate::astrometry::{
        AstrometryAnalysisStatus, AstrometryAttemptOutcome, AstrometrySolveMode,
    };

    let attempt = analysis.solve_attempt.as_ref()?;
    let pixel_solved = analysis.status == AstrometryAnalysisStatus::Solved
        && matches!(
            analysis.mode,
            Some(AstrometrySolveMode::Hinted | AstrometrySolveMode::Blind)
        )
        && attempt.outcome == AstrometryAttemptOutcome::Solved;
    let solve_failed = analysis.status == AstrometryAnalysisStatus::Failed
        && attempt.outcome != AstrometryAttemptOutcome::Solved;
    let field_short_axis_arcsec = analysis.solution.as_ref().map(|solution| {
        solution.pixel_scale_arcsec_per_pixel
            * f64::from(solution.image_width.min(solution.image_height))
    });
    Some(AstrometryFrameMetrics {
        pixel_solved,
        solve_failed,
        image_quality_evidence: attempt.image_quality_evidence,
        solved_center_ra_deg: analysis.solution.as_ref().map(|s| s.center_ra_deg),
        solved_center_dec_deg: analysis.solution.as_ref().map(|s| s.center_dec_deg),
        east_offset_arcsec: analysis
            .pointing
            .as_ref()
            .and_then(|p| p.east_offset_arcsec),
        north_offset_arcsec: analysis
            .pointing
            .as_ref()
            .and_then(|p| p.north_offset_arcsec),
        separation_arcsec: analysis.pointing.as_ref().map(|p| p.separation_arcsec),
        target_in_frame: analysis.pointing.as_ref().map(|p| p.target_in_frame),
        field_short_axis_arcsec,
        matched_stars: analysis.solution.as_ref().map(|s| s.matched_stars),
        rms_arcsec: analysis.solution.as_ref().map(|s| s.rms_arcsec),
        error: analysis.error.clone(),
    })
}

/// Quality analysis result for a single image within its sequence context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageQualityResult {
    pub image_id: i32,
    pub quality_score: f64,
    pub temporal_anomaly_score: f64,
    pub category: Option<IssueCategory>,
    #[serde(default)]
    pub flags: Vec<IssueCategory>,
    pub normalized_metrics: NormalizedMetrics,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointing: Option<PointingQuality>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub satellite: Option<SatelliteFrameMetrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regrade_reason: Option<String>,
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
    #[serde(default)]
    pub out_of_target_count: usize,
    #[serde(default)]
    pub plate_solve_failed_count: usize,
    #[serde(default)]
    pub satellite_risk_count: usize,
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
    #[serde(default)]
    pub session_id: Option<String>,
    pub star_count: Option<f64>,
    pub hfr: Option<f64>,
    pub eccentricity: Option<f64>,
    pub snr: Option<f64>,
    pub background: Option<f64>,
    /// Fraction of frame grid cells with collapsed star density
    /// (see `spatial_analysis::SpatialMetrics::star_dead_cell_fraction`).
    /// Detects partial occlusion (trees, dome, stray light) that global star
    /// counts miss.
    #[serde(default)]
    pub dead_cell_fraction: Option<f64>,
    /// Relative spread of per-cell background medians
    /// (see `spatial_analysis::SpatialMetrics::bg_cell_spread`). Detects
    /// stray-light gradients, often before any stars are lost.
    #[serde(default)]
    pub bg_cell_spread: Option<f64>,
    /// Median flux ratio of matched stars vs the sequence reference
    /// (photometry module): 1.0 nominal, 0.7 = whole frame ~0.4 mag dimmer.
    /// Detects thin uniform cloud long before stars are lost.
    #[serde(default)]
    pub transparency: Option<f64>,
    /// Fraction of grid cells with localized extinction (cell flux ratio
    /// well below the frame's global transparency): a small cloud.
    #[serde(default)]
    pub extinction_cell_fraction: Option<f64>,
    /// Fraction of cells with a hard transient drop in their share of the
    /// frame's stars vs their own temporal baseline (small opaque cloud).
    #[serde(default)]
    pub star_cell_drop_fraction: Option<f64>,
    /// Fraction of cells with a transient, gradient-detrended background
    /// rise vs their own temporal baseline (errant light).
    #[serde(default)]
    pub bg_cell_rise_fraction: Option<f64>,
    /// Fraction of cells with a transient, gradient-detrended background
    /// FALL vs their own temporal baseline: a dark occluder or cloud shadow
    /// blocking skyglow (affected regions read darker, not milky).
    #[serde(default)]
    pub bg_cell_fall_fraction: Option<f64>,
    /// Largest positive within-frame deviation above the frame's own robust
    /// plane model (fraction of sky). Catches STATIC localized glow - haze
    /// or a lit occluder edge present from a sequence's first frame - which
    /// temporal baselines can never see.
    #[serde(default)]
    pub bg_glow_max: Option<f64>,
    /// Pixel-derived astrometry merged from PSF Guard's astrometry cache.
    #[serde(default)]
    pub astrometry: Option<AstrometryFrameMetrics>,
    /// Cached orbital prediction for this exact source file and WCS.
    #[serde(default)]
    pub satellite: Option<SatelliteFrameMetrics>,
}

/// Configurable weights for composite quality scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityWeights {
    pub star_count: f64,
    pub hfr: f64,
    pub eccentricity: f64,
    pub snr: f64,
    pub background: f64,
    /// Weight for spatial star coverage (occlusion). Only contributes when
    /// spatial metrics are available (FITS-computed); DB-metadata-only
    /// analysis is unaffected because missing metrics are renormalized away.
    #[serde(default = "default_spatial_quality_weight")]
    pub spatial: f64,
    /// Weight for photometric transparency (thin-cloud veiling). Additive
    /// and missing-metric-safe like `spatial`.
    #[serde(default = "default_transparency_quality_weight")]
    pub transparency: f64,
    /// Weight for absolute pointing quality. Missing astrometry is
    /// renormalized away, so databases that have not run a quality scan keep
    /// their historical scores.
    #[serde(default = "default_pointing_quality_weight")]
    pub pointing: f64,
}

fn default_pointing_quality_weight() -> f64 {
    0.25
}

fn default_transparency_quality_weight() -> f64 {
    0.15
}

fn default_spatial_quality_weight() -> f64 {
    0.20
}

impl Default for QualityWeights {
    fn default() -> Self {
        Self {
            star_count: 0.30,
            hfr: 0.25,
            eccentricity: 0.10,
            snr: 0.25,
            background: 0.10,
            spatial: default_spatial_quality_weight(),
            transparency: default_transparency_quality_weight(),
            pointing: default_pointing_quality_weight(),
        }
    }
}

impl QualityWeights {
    /// Normalize weights so they sum to 1.0. If all weights are zero, returns defaults.
    pub fn normalized(self) -> Self {
        let sum = self.star_count
            + self.hfr
            + self.eccentricity
            + self.snr
            + self.background
            + self.spatial
            + self.transparency
            + self.pointing;
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
            spatial: self.spatial / sum,
            transparency: self.transparency / sum,
            pointing: self.pointing / sum,
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
    /// Weight for a rise in the dead-cell fraction relative to the EWMA
    /// baseline. The deviation is the absolute rise (already a fraction of
    /// the frame), not a relative change, since the baseline is often 0.
    #[serde(default = "default_spatial_temporal_weight")]
    pub spatial: f64,
}

fn default_spatial_temporal_weight() -> f64 {
    0.40
}

impl Default for TemporalWeights {
    fn default() -> Self {
        Self {
            star_count: 0.40,
            background: 0.25,
            hfr: 0.20,
            snr: 0.15,
            spatial: default_spatial_temporal_weight(),
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
    /// Rise in dead-cell fraction over the local baseline that flags a
    /// localized occlusion (absolute fraction of the frame). Clean-frame
    /// jitter stays <= 0.04 across validated nights/filters, so 0.08 leaves
    /// a 2x margin while catching occlusion onset (0.08-0.2).
    #[serde(default = "default_dead_cell_rise_threshold")]
    pub dead_cell_rise_threshold: f64,
    /// Absolute dead-cell fraction above which a frame is considered
    /// occluded regardless of the baseline (half the frame without stars).
    #[serde(default = "default_dead_cell_abs_threshold")]
    pub dead_cell_abs_threshold: f64,
    /// Rise of the per-cell background spread over the local baseline that
    /// flags a stray-light gradient (clean frames stay <= ~0.1; gradients
    /// reach 0.3+ before stars are lost).
    #[serde(default = "default_bg_spread_rise_threshold")]
    pub bg_spread_rise_threshold: f64,
    /// Temporal-anomaly score above which a frame is excluded from the EWMA
    /// baseline update. Without this, a slowly growing occlusion (tree drift
    /// across 20+ frames) gets absorbed into the baseline and stops being
    /// anomalous ("boiling frog"). Set >= 1.0 to disable freezing.
    #[serde(default = "default_baseline_freeze_threshold")]
    pub baseline_freeze_threshold: f64,
    /// Localized-extinction cell fraction above which a frame is classified
    /// as a small passing cloud (photometric evidence: a coherent flux dip
    /// in a patch of matched stars).
    #[serde(default = "default_extinction_cells_threshold")]
    pub extinction_cells_threshold: f64,
    /// Global transparency below which a frame is classified as veiled by
    /// thin cloud (median matched-star flux ratio vs sequence reference).
    #[serde(default = "default_transparency_threshold")]
    pub transparency_threshold: f64,
    /// Star-share drop cell fraction above which a frame is classified as a
    /// small opaque cloud (per-cell temporal baseline).
    #[serde(default = "default_star_drop_cells_threshold")]
    pub star_drop_cells_threshold: f64,
    /// Background-rise cell fraction above which a frame is flagged for
    /// errant light (per-cell temporal baseline, gradient-detrended).
    #[serde(default = "default_bg_rise_cells_threshold")]
    pub bg_rise_cells_threshold: f64,
    /// Static within-frame glow (max positive robust-plane residual as a
    /// fraction of sky) above which a frame is flagged for stray light.
    /// Clean-frame envelope: 0.014-0.021; static haze: 0.030-0.065.
    #[serde(default = "default_bg_glow_threshold")]
    pub bg_glow_threshold: f64,
    /// Maximum consecutive frames the EWMA baselines stay frozen. A run of
    /// anomalous frames longer than this is accepted as a new steady state
    /// (moonrise, light dome) and the baselines re-seed from the current
    /// frame, so a permanent condition change cannot mark the entire rest of
    /// a session anomalous. Occluded frames remain penalized regardless via
    /// the absolute spatial-coverage term.
    #[serde(default = "default_baseline_freeze_max_frames")]
    pub baseline_freeze_max_frames: usize,
}

fn default_dead_cell_rise_threshold() -> f64 {
    0.08
}

fn default_dead_cell_abs_threshold() -> f64 {
    0.5
}

fn default_bg_spread_rise_threshold() -> f64 {
    0.15
}

fn default_baseline_freeze_threshold() -> f64 {
    0.15
}

fn default_baseline_freeze_max_frames() -> usize {
    15
}

fn default_extinction_cells_threshold() -> f64 {
    0.06
}

fn default_transparency_threshold() -> f64 {
    0.80
}

fn default_star_drop_cells_threshold() -> f64 {
    0.06
}

fn default_bg_rise_cells_threshold() -> f64 {
    0.06
}

fn default_bg_glow_threshold() -> f64 {
    0.025
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
            dead_cell_rise_threshold: default_dead_cell_rise_threshold(),
            dead_cell_abs_threshold: default_dead_cell_abs_threshold(),
            bg_spread_rise_threshold: default_bg_spread_rise_threshold(),
            baseline_freeze_threshold: default_baseline_freeze_threshold(),
            baseline_freeze_max_frames: default_baseline_freeze_max_frames(),
            extinction_cells_threshold: default_extinction_cells_threshold(),
            transparency_threshold: default_transparency_threshold(),
            star_drop_cells_threshold: default_star_drop_cells_threshold(),
            bg_rise_cells_threshold: default_bg_rise_cells_threshold(),
            bg_glow_threshold: default_bg_glow_threshold(),
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

            let explicit_session_changed = current_seq.last().is_some_and(|previous| {
                previous
                    .session_id
                    .as_ref()
                    .zip(img.session_id.as_ref())
                    .is_some_and(|(left, right)| left != right)
            });
            if explicit_session_changed || curr_ts - prev_ts > gap_seconds {
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
        let pointing_quality = self.analyze_pointing(&images);

        // If sequence is too short, return with score 1.0 for all images
        if image_count < self.config.min_sequence_length {
            let mut results: Vec<ImageQualityResult> = images
                .iter()
                .enumerate()
                .map(|(idx, img)| ImageQualityResult {
                    image_id: img.image_id,
                    quality_score: apply_pointing_score(1.0, pointing_quality[idx].as_ref()),
                    temporal_anomaly_score: 0.0,
                    category: None,
                    flags: Vec::new(),
                    normalized_metrics: NormalizedMetrics {
                        star_count: Some(1.0),
                        hfr: Some(1.0),
                        eccentricity: Some(1.0),
                        snr: Some(1.0),
                        background: Some(1.0),
                        spatial_coverage: Some(1.0),
                        transparency: Some(1.0),
                        pointing: pointing_quality[idx].as_ref().and_then(pointing_normalized),
                    },
                    pointing: pointing_quality[idx].clone(),
                    satellite: img.satellite.clone(),
                    regrade_reason: None,
                    details: None,
                })
                .collect();
            self.merge_pointing_issues(&mut results);
            self.merge_satellite_issues(&mut results, &images);
            let summary = self.build_summary(&results);

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
                summary,
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
        // Spatial coverage uses an absolute mapping instead of the
        // sequence-relative percentile normalization: dead_cell_fraction is
        // already a dimensionless fraction of the frame, and relative
        // normalization would blow tiny variations on an all-clean sequence
        // up to the full 0..1 range.
        let norm_spatial: Vec<Option<f64>> = images
            .iter()
            .map(|i| {
                i.dead_cell_fraction
                    .map(|dead| 1.0 - (dead / self.config.dead_cell_abs_threshold).clamp(0.0, 1.0))
            })
            .collect();
        // Transparency is already sequence-relative (flux ratio vs the
        // sequence reference), so map it absolutely: nominal (>= 1.0) -> 1.0,
        // 40% flux loss or worse -> 0.0.
        let norm_transparency: Vec<Option<f64>> = images
            .iter()
            .map(|i| i.transparency.map(|t| ((t - 0.6) / 0.4).clamp(0.0, 1.0)))
            .collect();
        let norm_pointing: Vec<Option<f64>> = pointing_quality
            .iter()
            .map(|quality| quality.as_ref().and_then(pointing_normalized))
            .collect();

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
            let nsp = norm_spatial[i];
            let ntr = norm_transparency[i];
            let npt = norm_pointing[i];

            // Weighted sum using available metrics
            let (score, total_weight) = weighted_sum_available(&[
                (ns, w.star_count),
                (nh, w.hfr),
                (ne, w.eccentricity),
                (nsn, w.snr),
                (nb, w.background),
                (nsp, w.spatial),
                (ntr, w.transparency),
                (npt, w.pointing),
            ]);

            let quality_score = if total_weight > 0.0 {
                score / total_weight
            } else {
                1.0
            };

            // Apply temporal penalty
            let temporal = temporal_scores[i];
            let penalty = 1.0 - temporal.min(0.5);
            let final_score = apply_pointing_score(
                (quality_score * penalty).clamp(0.0, 1.0),
                pointing_quality[i].as_ref(),
            );

            results.push(ImageQualityResult {
                image_id: images[i].image_id,
                quality_score: final_score,
                temporal_anomaly_score: temporal,
                category: None, // Classified below
                flags: Vec::new(),
                normalized_metrics: NormalizedMetrics {
                    star_count: ns,
                    hfr: nh,
                    eccentricity: ne,
                    snr: nsn,
                    background: nb,
                    spatial_coverage: nsp,
                    transparency: ntr,
                    pointing: npt,
                },
                pointing: pointing_quality[i].clone(),
                satellite: images[i].satellite.clone(),
                regrade_reason: None,
                details: None,
            });
        }

        // Classify issues
        self.classify_issues(&mut results, &images);
        self.merge_pointing_issues(&mut results);
        self.merge_satellite_issues(&mut results, &images);

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

    fn merge_satellite_issues(&self, results: &mut [ImageQualityResult], images: &[ImageMetrics]) {
        for (result, image) in results.iter_mut().zip(images) {
            let Some(satellite) = image.satellite.as_ref() else {
                continue;
            };
            if satellite.potentially_bright_count == 0 && satellite.pixel_aligned_count == 0 {
                continue;
            }
            if !result.flags.contains(&IssueCategory::SatelliteTrailRisk) {
                result.flags.push(IssueCategory::SatelliteTrailRisk);
            }
            result
                .category
                .get_or_insert(IssueCategory::SatelliteTrailRisk);

            let pixel_evidence = if satellite.pixel_aligned_count > 0 {
                format!(
                    "Pixel corridor alignment found {} matching trail(s), including {} high-risk candidate(s).",
                    satellite.pixel_aligned_count, satellite.pixel_aligned_high_risk_count
                )
            } else if satellite.pixel_alignment_attempted {
                "Pixel corridor alignment found no matching trail.".to_string()
            } else {
                "Pixel alignment was unavailable; this remains orbital prediction only.".to_string()
            };
            let detail = format!(
                "Predicted satellite crossing: {} track(s), {} potentially bright, {} high risk; maximum heuristic risk {:.2}. {}",
                satellite.predicted_tracks,
                satellite.potentially_bright_count,
                satellite.high_risk_count,
                satellite.maximum_bright_trail_risk,
                pixel_evidence,
            );
            result.details = Some(match result.details.take() {
                Some(existing) => format!("{detail} {existing}"),
                None => detail,
            });

            if satellite.reject_recommended && satellite.pixel_aligned_high_risk_count > 0 {
                result.quality_score = result.quality_score.min(0.35);
                let reason = format!(
                    "[Auto] Pixel-aligned bright satellite trail - {} high-risk candidate(s), risk {:.2}; verify overlay",
                    satellite.pixel_aligned_high_risk_count,
                    satellite.maximum_bright_trail_risk,
                );
                result.regrade_reason = Some(match result.regrade_reason.take() {
                    Some(existing) => format!("{existing}; {reason}"),
                    None => reason,
                });
            } else {
                result.quality_score = result.quality_score.min(0.75);
            }
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
        let mut bl_dead: Option<f64> = None;
        // Consecutive frames excluded from the baseline update; bounds the
        // freeze so a permanent condition change becomes the new baseline.
        let mut frozen_streak: usize = 0;

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

            // Dead-cell fraction deviation (absolute rise; the baseline is
            // typically ~0 on clean frames, so a relative change is
            // meaningless).
            let dead_dev = if let (Some(val), Some(bl)) = (img.dead_cell_fraction, bl_dead) {
                (val - bl).max(0.0)
            } else {
                0.0
            };

            scores[i] = tw.star_count * star_dev
                + tw.background * bg_dev
                + tw.hfr * hfr_dev
                + tw.snr * snr_dev
                + tw.spatial * dead_dev;

            // Exclude anomalous frames from the baseline so a slow-growing
            // problem (tree drifting through the field over many frames)
            // cannot normalize itself into the baseline — but only for a
            // bounded number of frames. A longer run is a new steady state
            // (moonrise, light dome): re-seed the baselines from the current
            // frame so the rest of the session is not scored anomalous.
            if scores[i] > self.config.baseline_freeze_threshold {
                frozen_streak += 1;
                if frozen_streak < self.config.baseline_freeze_max_frames.max(1) {
                    continue;
                }
                // Regime change accepted: hard re-seed instead of EWMA blend.
                bl_stars = img.star_count.or(bl_stars);
                bl_bg = img.background.or(bl_bg);
                bl_hfr = img.hfr.or(bl_hfr);
                bl_snr = img.snr.or(bl_snr);
                bl_dead = img.dead_cell_fraction.or(bl_dead);
                frozen_streak = 0;
                continue;
            }
            frozen_streak = 0;

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
            if let Some(val) = img.dead_cell_fraction {
                bl_dead = Some(match bl_dead {
                    Some(bl) => alpha * val + (1.0 - alpha) * bl,
                    None => val,
                });
            }
        }

        scores
    }

    /// Convert per-frame pixel solutions into absolute pointing quality plus
    /// robust sequence-relative jump/drift evidence.
    fn analyze_pointing(&self, images: &[ImageMetrics]) -> Vec<Option<PointingQuality>> {
        let solved_candidates: Vec<usize> = images
            .iter()
            .enumerate()
            .filter_map(|(idx, image)| {
                image
                    .astrometry
                    .as_ref()
                    .filter(|a| a.pixel_solved)
                    .map(|_| idx)
            })
            .collect();
        let has_expected_target = !solved_candidates.is_empty()
            && solved_candidates.iter().all(|&idx| {
                images[idx]
                    .astrometry
                    .as_ref()
                    .is_some_and(|a| a.separation_arcsec.is_some())
            });
        let tangent_origin = solved_candidates.iter().find_map(|&idx| {
            let a = images[idx].astrometry.as_ref()?;
            Some((a.solved_center_ra_deg?, a.solved_center_dec_deg?))
        });
        let solved_points: Vec<(usize, f64, f64)> = solved_candidates
            .iter()
            .filter_map(|&idx| {
                let a = images[idx].astrometry.as_ref()?;
                let (east, north) = if has_expected_target {
                    (a.east_offset_arcsec?, a.north_offset_arcsec?)
                } else {
                    tangent_plane_offset_arcsec(
                        tangent_origin?,
                        (a.solved_center_ra_deg?, a.solved_center_dec_deg?),
                    )?
                };
                Some((idx, east, north))
            })
            .collect();
        let solved_indices = solved_points
            .iter()
            .map(|(idx, _, _)| *idx)
            .collect::<Vec<_>>();

        let mut quality: Vec<Option<PointingQuality>> = images
            .iter()
            .map(|image| {
                let a = image.astrometry.as_ref()?;
                let fraction = has_expected_target
                    .then_some(a.separation_arcsec)
                    .flatten()
                    .zip(a.field_short_axis_arcsec)
                    .filter(|(_, field)| *field > 0.0)
                    .map(|(separation, field)| separation / field);
                // Absolute off-target flags are assigned below once the
                // segment's own cluster is known: a consistently displaced
                // segment is deliberate framing, not a pointing failure.
                let mut flags = Vec::new();
                if a.pixel_solved && a.target_in_frame == Some(false) {
                    flags.push(IssueCategory::OffTarget);
                }
                if a.solve_failed && a.image_quality_evidence {
                    flags.push(IssueCategory::PlateSolveFailed);
                }
                Some(PointingQuality {
                    pixel_solved: a.pixel_solved,
                    solve_failed: a.solve_failed,
                    image_quality_evidence: a.image_quality_evidence,
                    expected_target: has_expected_target && a.pixel_solved,
                    flags,
                    east_offset_arcsec: has_expected_target
                        .then_some(a.east_offset_arcsec)
                        .flatten(),
                    north_offset_arcsec: has_expected_target
                        .then_some(a.north_offset_arcsec)
                        .flatten(),
                    separation_arcsec: has_expected_target.then_some(a.separation_arcsec).flatten(),
                    field_fraction_offset: fraction,
                    reference_offset_arcsec: None,
                    reference_field_fraction: None,
                    drift_rate_arcsec_per_hour: None,
                    matched_stars: a.matched_stars,
                    rms_arcsec: a.rms_arcsec,
                    error: a.error.clone(),
                })
            })
            .collect();

        for &(idx, east, north) in &solved_points {
            if let Some(pointing) = quality[idx].as_mut() {
                pointing.east_offset_arcsec = Some(east);
                pointing.north_offset_arcsec = Some(north);
                pointing.separation_arcsec = Some(east.hypot(north));
            }
        }

        if solved_indices.len() < 3 {
            // Too few solves to know the segment's own cluster. A target fully
            // outside the footprint is still unambiguous; a large in-frame
            // offset could be deliberate framing, so warn instead of flagging.
            if has_expected_target {
                for &idx in &solved_indices {
                    let target_outside = images[idx]
                        .astrometry
                        .as_ref()
                        .and_then(|a| a.target_in_frame)
                        == Some(false);
                    if let Some(pointing) = quality[idx].as_mut() {
                        let far = pointing
                            .field_fraction_offset
                            .is_some_and(|fraction| fraction >= 0.20);
                        if target_outside {
                            push_issue(&mut pointing.flags, IssueCategory::OffTarget);
                        } else if far {
                            push_issue(&mut pointing.flags, IssueCategory::StableOffset);
                        }
                    }
                }
            }
            return quality;
        }

        let field = median(
            &solved_indices
                .iter()
                .filter_map(|&idx| images[idx].astrometry.as_ref()?.field_short_axis_arcsec)
                .collect::<Vec<_>>(),
        );

        // Discover stable framing clusters before measuring residuals. A
        // single global median turns an intentional mid-session composition
        // change into an outlier cluster and can auto-reject every frame in
        // the second composition. Single-link clustering keeps gradual drift
        // connected while separating genuine step changes.
        let cluster_threshold = (0.08 * field).max(30.0);
        let cluster_labels = pointing_cluster_labels(&solved_points, cluster_threshold);
        let cluster_count = cluster_labels
            .iter()
            .copied()
            .max()
            .map_or(0, |label| label + 1);
        let mut cluster_east = vec![Vec::new(); cluster_count];
        let mut cluster_north = vec![Vec::new(); cluster_count];
        for ((_, east, north), &label) in solved_points.iter().zip(&cluster_labels) {
            cluster_east[label].push(*east);
            cluster_north[label].push(*north);
        }
        let cluster_centers = cluster_east
            .iter()
            .zip(&cluster_north)
            .map(|(east, north)| (median(east), median(north)))
            .collect::<Vec<_>>();
        let distances = solved_points
            .iter()
            .zip(&cluster_labels)
            .map(|((_, east, north), &label)| {
                let center = cluster_centers[label];
                (*east - center.0).hypot(*north - center.1)
            })
            .collect::<Vec<_>>();

        // Build contiguous runs of cluster membership. A run that leaves a
        // cluster and returns to that same cluster is a tracking excursion;
        // any other run with at least three solves is enough evidence for a
        // stable deliberate framing segment, including A -> B -> C mosaics.
        let mut runs = Vec::new();
        let mut position_to_run = vec![0usize; cluster_labels.len()];
        let mut start = 0usize;
        while start < cluster_labels.len() {
            let label = cluster_labels[start];
            let mut end = start;
            while end + 1 < cluster_labels.len() && cluster_labels[end + 1] == label {
                end += 1;
            }
            let run_idx = runs.len();
            for entry in &mut position_to_run[start..=end] {
                *entry = run_idx;
            }
            runs.push((start, end, label));
            start = end + 1;
        }
        let mut jump_run = vec![false; runs.len()];
        let mut stable_run = vec![false; runs.len()];
        for (run_idx, &(start, end, label)) in runs.iter().enumerate() {
            let returns_to_same_cluster = start > 0
                && end + 1 < cluster_labels.len()
                && cluster_labels[start - 1] == cluster_labels[end + 1]
                && cluster_labels[start - 1] != label;
            let run_len = end - start + 1;
            jump_run[run_idx] = returns_to_same_cluster && run_len * 2 < solved_points.len();
            stable_run[run_idx] = run_len >= 3 && !returns_to_same_cluster;
        }

        for (position, &idx) in solved_indices.iter().enumerate() {
            let target_outside = images[idx]
                .astrometry
                .as_ref()
                .and_then(|a| a.target_in_frame)
                == Some(false);
            if let Some(pointing) = quality[idx].as_mut() {
                pointing.reference_offset_arcsec = Some(distances[position]);
                if field > 0.0 {
                    pointing.reference_field_fraction = Some(distances[position] / field);
                }
                if has_expected_target {
                    // A target outside the solved footprint is unambiguous.
                    // Otherwise a far but stable framing run is advisory; a
                    // short/unclustered departure remains rejectable.
                    let far = pointing
                        .field_fraction_offset
                        .is_some_and(|fraction| fraction >= 0.20);
                    let run_idx = position_to_run[position];
                    if target_outside || (far && !stable_run[run_idx]) {
                        push_issue(&mut pointing.flags, IssueCategory::OffTarget);
                    } else if far {
                        push_issue(&mut pointing.flags, IssueCategory::StableOffset);
                    }
                }
            }
        }

        for (run_idx, &(start, end, _)) in runs.iter().enumerate() {
            if jump_run[run_idx] {
                for &idx in &solved_indices[start..=end] {
                    if let Some(pointing) = quality[idx].as_mut() {
                        push_issue(&mut pointing.flags, IssueCategory::PointingJump);
                    }
                }
            }
        }

        // Fit drift within each contiguous framing segment. A step between
        // stable compositions must not manufacture a session-wide slope.
        for (run_idx, &(start, end, _)) in runs.iter().enumerate() {
            if jump_run[run_idx] {
                continue;
            }
            let time_values: Vec<(usize, f64, f64, f64)> = solved_points[start..=end]
                .iter()
                .filter_map(|&(idx, east, north)| {
                    let timestamp = images[idx].timestamp? as f64;
                    Some((idx, timestamp, east, north))
                })
                .collect();
            if time_values.len() < 4 {
                continue;
            }
            let origin = time_values[0].1;
            let times_hours = time_values
                .iter()
                .map(|(_, timestamp, _, _)| (timestamp - origin) / 3600.0)
                .collect::<Vec<_>>();
            let east_values = time_values
                .iter()
                .map(|(_, _, east, _)| *east)
                .collect::<Vec<_>>();
            let north_values = time_values
                .iter()
                .map(|(_, _, _, north)| *north)
                .collect::<Vec<_>>();
            let east_slope = theil_sen_slope(&times_hours, &east_values);
            let north_slope = theil_sen_slope(&times_hours, &north_values);
            let drift_rate = east_slope.hypot(north_slope);
            let duration_hours = times_hours.last().copied().unwrap_or(0.0);
            // Estimate dither/noise after removing the robust trend. Using
            // scatter around the raw median would let a real monotonic drift
            // inflate its own threshold until it became undetectable.
            let east_intercept = median(
                &east_values
                    .iter()
                    .zip(&times_hours)
                    .map(|(value, hours)| value - east_slope * hours)
                    .collect::<Vec<_>>(),
            );
            let north_intercept = median(
                &north_values
                    .iter()
                    .zip(&times_hours)
                    .map(|(value, hours)| value - north_slope * hours)
                    .collect::<Vec<_>>(),
            );
            let trend_residuals = east_values
                .iter()
                .zip(&north_values)
                .zip(&times_hours)
                .map(|((east, north), hours)| {
                    (east - (east_intercept + east_slope * hours))
                        .hypot(north - (north_intercept + north_slope * hours))
                })
                .collect::<Vec<_>>();
            let residual_median = median(&trend_residuals);
            let residual_mad = median(
                &trend_residuals
                    .iter()
                    .map(|residual| (residual - residual_median).abs())
                    .collect::<Vec<_>>(),
            );
            let drift_threshold = (6.0 * residual_mad).max(0.08 * field).max(30.0);
            if drift_rate * duration_hours > drift_threshold {
                let first_east = time_values[0].2;
                let first_north = time_values[0].3;
                for ((idx, _, east, north), &hours) in time_values.iter().zip(&times_hours) {
                    let idx = *idx;
                    if let Some(pointing) = quality[idx].as_mut() {
                        pointing.drift_rate_arcsec_per_hour = Some(drift_rate);
                        let from_start = (*east - first_east).hypot(*north - first_north);
                        if hours > 0.0 && from_start > drift_threshold {
                            push_issue(&mut pointing.flags, IssueCategory::PointingDrift);
                        }
                    }
                }
            }
        }

        quality
    }

    fn merge_pointing_issues(&self, results: &mut [ImageQualityResult]) {
        for result in results {
            if let Some(category) = result.category.clone() {
                push_issue(&mut result.flags, category);
            }
            let Some(pointing) = result.pointing.as_ref() else {
                continue;
            };
            for flag in &pointing.flags {
                push_issue(&mut result.flags, flag.clone());
            }

            let original_category = result.category.clone();
            let (category, astrometry_detail, reason) = if pointing
                .flags
                .contains(&IssueCategory::OffTarget)
            {
                let fraction = pointing.field_fraction_offset.unwrap_or(0.0) * 100.0;
                (
                    Some(IssueCategory::OffTarget),
                    Some(format!(
                        "Plate solution places the intended target {:.0}% of the short field dimension from center{}.",
                        fraction,
                        if fraction >= 100.0 { " (outside the frame)" } else { "" }
                    )),
                    Some(format!(
                        "[Auto] Astrometry: Off target - score {:.2}; offset {:.0}% of field",
                        result.quality_score, fraction
                    )),
                )
            } else if pointing.flags.contains(&IssueCategory::PointingJump) {
                (
                    Some(IssueCategory::PointingJump),
                    Some(format!(
                        "Solved center jumped {:.0} arcsec outside the sequence dither envelope.",
                        pointing.reference_offset_arcsec.unwrap_or(0.0)
                    )),
                    Some(format!(
                        "[Auto] Astrometry: Tracking lost - score {:.2}; pointing jump {:.0} arcsec",
                        result.quality_score,
                        pointing.reference_offset_arcsec.unwrap_or(0.0)
                    )),
                )
            } else if pointing.flags.contains(&IssueCategory::PointingDrift) {
                (
                    Some(IssueCategory::PointingDrift),
                    Some(format!(
                        "Solved center drifted at {:.0} arcsec/hour beyond the sequence dither envelope.",
                        pointing.drift_rate_arcsec_per_hour.unwrap_or(0.0)
                    )),
                    Some(format!(
                        "[Auto] Astrometry: Tracking drift - score {:.2}; {:.0} arcsec/hour",
                        result.quality_score,
                        pointing.drift_rate_arcsec_per_hour.unwrap_or(0.0)
                    )),
                )
            } else if pointing.flags.contains(&IssueCategory::PlateSolveFailed) {
                let corroborated = matches!(
                    original_category,
                    Some(
                        IssueCategory::LikelyClouds
                            | IssueCategory::PossibleObstruction
                            | IssueCategory::TrackingError
                            | IssueCategory::WindShake
                    )
                );
                if corroborated {
                    result.quality_score = result.quality_score.min(0.30);
                }
                (
                    original_category.or(Some(IssueCategory::PlateSolveFailed)),
                    Some(format!(
                        "Pixel plate solve failed{}{}.",
                        if corroborated {
                            " and the frame has independent quality degradation"
                        } else {
                            ""
                        },
                        pointing
                            .error
                            .as_deref()
                            .map(|error| format!(": {error}"))
                            .unwrap_or_default()
                    )),
                    corroborated.then(|| {
                        format!(
                            "[Auto] Quality: Plate solve failed + image degradation - score {:.2}",
                            result.quality_score
                        )
                    }),
                )
            } else if pointing.flags.contains(&IssueCategory::StableOffset) {
                (
                    original_category.or(Some(IssueCategory::StableOffset)),
                    Some(format!(
                        "Sequence is consistently offset {:.0}% of the short field dimension from the intended target; likely deliberate framing, not auto-rejected.",
                        pointing.field_fraction_offset.unwrap_or(0.0) * 100.0
                    )),
                    None,
                )
            } else {
                (original_category, None, None)
            };
            result.category = category;
            result.regrade_reason = reason;
            if let Some(astrometry_detail) = astrometry_detail {
                result.details = Some(match result.details.take() {
                    Some(existing) => format!("{astrometry_detail} {existing}"),
                    None => astrometry_detail,
                });
            }
        }
    }

    /// Classify issues for each image based on metric deviations.
    fn classify_issues(&self, results: &mut [ImageQualityResult], images: &[ImageMetrics]) {
        let n = images.len();
        if n < 2 {
            return;
        }

        // Frames already flagged as temporally anomalous are excluded from
        // local baselines so that a long-running problem does not become its
        // own reference.
        let anomalous: Vec<bool> = results
            .iter()
            .map(|r| r.temporal_anomaly_score > self.config.baseline_freeze_threshold)
            .collect();

        // Per-frame dead-cell rise, precomputed so each frame can check its
        // neighbors for corroboration.
        let dead_rises: Vec<f64> = (0..n)
            .map(|i| self.compute_additive_rise(images, &anomalous, i, |m| m.dead_cell_fraction))
            .collect();

        #[allow(clippy::needless_range_loop)]
        for i in 0..n {
            // Spatial occlusion signals: checked even for frames whose
            // composite score is still good, because a subtle occlusion
            // (10-20% of the frame) barely moves the global metrics.
            let dead_abs = images[i].dead_cell_fraction.unwrap_or(0.0);
            let dead_rise = dead_rises[i];
            // A rise-based occlusion call needs corroboration from an
            // adjacent frame: real occluders (trees, dome) persist across
            // frames, while a single-frame blip (wind gust, passing wisp)
            // must not reject an otherwise excellent frame.
            let neighbor_elevated = (i > 0
                && dead_rises[i - 1] > self.config.dead_cell_rise_threshold * 0.5)
                || (i + 1 < n && dead_rises[i + 1] > self.config.dead_cell_rise_threshold * 0.5);
            let occluded = dead_abs > self.config.dead_cell_abs_threshold
                || (dead_rise > self.config.dead_cell_rise_threshold && neighbor_elevated);

            let bg_spread_rise =
                self.compute_additive_rise(images, &anomalous, i, |m| m.bg_cell_spread);
            let stray_gradient = bg_spread_rise > self.config.bg_spread_rise_threshold;

            // Photometric small-transient signals. These are already
            // sequence-relative and rest on multi-star evidence, so no
            // neighbor corroboration is needed (small clouds are often
            // single-frame events - they move).
            let extinction = images[i].extinction_cell_fraction.unwrap_or(0.0);
            let star_cell_drop = images[i].star_cell_drop_fraction.unwrap_or(0.0);
            // A transient localized background FALL (dark occluder / cloud
            // shadow) corroborates weaker star-based evidence: with a dark
            // patch present, half-threshold extinction or star loss is
            // enough.
            let dark_patch = images[i].bg_cell_fall_fraction.unwrap_or(0.0)
                > self.config.bg_rise_cells_threshold;
            let small_cloud = extinction > self.config.extinction_cells_threshold
                || star_cell_drop > self.config.star_drop_cells_threshold
                || (dark_patch
                    && (extinction > self.config.extinction_cells_threshold * 0.5
                        || star_cell_drop > self.config.star_drop_cells_threshold * 0.5));
            let veiled = images[i]
                .transparency
                .is_some_and(|t| t < self.config.transparency_threshold);
            let bg_cell_rise = images[i].bg_cell_rise_fraction.unwrap_or(0.0);
            let errant_light = bg_cell_rise > self.config.bg_rise_cells_threshold;
            // Static glow is invisible to every temporal detector when the
            // haze is present from the sequence's first frame.
            let static_glow = images[i].bg_glow_max.unwrap_or(0.0) > self.config.bg_glow_threshold;

            if results[i].quality_score >= 0.7
                && !occluded
                && !stray_gradient
                && !small_cloud
                && !veiled
                && !errant_light
                && !static_glow
            {
                continue; // No classification needed for good frames
            }

            let star_drop = self.compute_fractional_drop(images, &anomalous, i, |m| m.star_count);
            let bg_rise = self.compute_fractional_rise(images, &anomalous, i, |m| m.background);
            let hfr_rise = self.compute_fractional_rise(images, &anomalous, i, |m| m.hfr);
            let ecc_rise = self.compute_fractional_rise(images, &anomalous, i, |m| m.eccentricity);

            let is_gradual_hfr = self.is_gradual_change(images, i, |m| m.hfr, 3);
            let is_gradual_bg = self.is_gradual_change(images, i, |m| m.background, 3);

            let star_stable = star_drop < self.config.star_drop_threshold;
            let bg_stable = bg_rise < self.config.bg_rise_threshold;
            let ecc_stable = ecc_rise < 0.15;

            // Classification rules. A localized dead region is checked first:
            // it is the most specific signature (clouds veil the frame
            // uniformly, occluders kill a contiguous part of it).
            let (category, details) = if occluded {
                (
                    Some(IssueCategory::PossibleObstruction),
                    Some(format!(
                        "{:.0}% of frame grid cells have no stars (baseline {:.0}%). Localized occlusion (trees, dome, dew shield, or foreground lit by stray light).",
                        dead_abs * 100.0,
                        (dead_abs - dead_rise).max(0.0) * 100.0,
                    )),
                )
            } else if small_cloud {
                (
                    Some(IssueCategory::LikelyClouds),
                    Some(format!(
                        "Localized extinction over {:.0}% of the field (flux dip {:.0}% of cells, star loss {:.0}% of cells{}) with global transparency {:.0}%. Small cloud passing through.",
                        extinction.max(star_cell_drop) * 100.0,
                        extinction * 100.0,
                        star_cell_drop * 100.0,
                        if dark_patch {
                            ", corroborated by a localized background darkening"
                        } else {
                            ""
                        },
                        images[i].transparency.unwrap_or(1.0) * 100.0,
                    )),
                )
            } else if veiled {
                (
                    Some(IssueCategory::LikelyClouds),
                    Some(format!(
                        "Frame transparency is {:.0}% of the sequence reference (matched-star flux ratio). Thin cloud veiling the whole field.",
                        images[i].transparency.unwrap_or(0.0) * 100.0,
                    )),
                )
            } else if errant_light && star_stable {
                (
                    Some(IssueCategory::SkyBrightening),
                    Some(format!(
                        "Transient localized background rise over {:.0}% of the field that the frame's own gradient does not explain, with stable stars. Errant light (headlights, flashlight, neighbor lighting).",
                        bg_cell_rise * 100.0,
                    )),
                )
            } else if static_glow && star_stable {
                (
                    Some(IssueCategory::SkyBrightening),
                    Some(format!(
                        "Static localized glow: a region reads {:.0}% above the frame's own gradient model in every frame of the session. Haze or a lit occluder edge at the field boundary.",
                        images[i].bg_glow_max.unwrap_or(0.0) * 100.0
                    )),
                )
            } else if stray_gradient && star_stable {
                (
                    Some(IssueCategory::SkyBrightening),
                    Some(format!(
                        "Background non-uniformity rose by {:.2} with stable star coverage. Stray light or moonlight gradient entering the optical path.",
                        bg_spread_rise
                    )),
                )
            } else if star_drop > self.config.star_drop_threshold
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
        anomalous: &[bool],
        idx: usize,
        f: impl Fn(&ImageMetrics) -> Option<f64>,
    ) -> f64 {
        let current = match f(&images[idx]) {
            Some(v) => v,
            None => return 0.0,
        };

        let baseline = self.local_baseline(images, anomalous, idx, &f);
        if baseline.abs() < 1e-10 {
            return 0.0;
        }
        ((baseline - current) / baseline).max(0.0)
    }

    /// Compute fractional rise relative to a local baseline (preceding frames).
    fn compute_fractional_rise(
        &self,
        images: &[ImageMetrics],
        anomalous: &[bool],
        idx: usize,
        f: impl Fn(&ImageMetrics) -> Option<f64>,
    ) -> f64 {
        let current = match f(&images[idx]) {
            Some(v) => v,
            None => return 0.0,
        };

        let baseline = self.local_baseline(images, anomalous, idx, &f);
        if baseline.abs() < 1e-10 {
            return 0.0;
        }
        ((current - baseline) / baseline).max(0.0)
    }

    /// Compute an absolute (additive) rise over the local baseline, for
    /// metrics that are already dimensionless fractions (dead-cell fraction,
    /// background spread) where a baseline of 0 makes relative change
    /// meaningless.
    fn compute_additive_rise(
        &self,
        images: &[ImageMetrics],
        anomalous: &[bool],
        idx: usize,
        f: impl Fn(&ImageMetrics) -> Option<f64>,
    ) -> f64 {
        let current = match f(&images[idx]) {
            Some(v) => v,
            None => return 0.0,
        };
        let baseline = self.local_baseline(images, anomalous, idx, &f);
        (current - baseline).max(0.0)
    }

    /// Compute local baseline as the median of up to 5 preceding
    /// non-anomalous frames. Anomalous frames are skipped (looking back up to
    /// 15 frames) so that an ongoing problem does not become its own
    /// baseline; if no clean frame is found, fall back to the plain
    /// 5-preceding-frames median.
    fn local_baseline(
        &self,
        images: &[ImageMetrics],
        anomalous: &[bool],
        idx: usize,
        f: &impl Fn(&ImageMetrics) -> Option<f64>,
    ) -> f64 {
        let start = idx.saturating_sub(15);
        let mut vals: Vec<f64> = (start..idx)
            .rev()
            .filter(|&j| !anomalous.get(j).copied().unwrap_or(false))
            .filter_map(|j| f(&images[j]))
            .take(5)
            .collect();
        if vals.is_empty() {
            let start = idx.saturating_sub(5);
            vals = (start..idx).filter_map(|j| f(&images[j])).collect();
        }
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
            out_of_target_count: 0,
            plate_solve_failed_count: 0,
            satellite_risk_count: 0,
        };

        for r in results {
            match r.quality_score {
                s if s >= 0.90 => summary.excellent_count += 1,
                s if s >= 0.70 => summary.good_count += 1,
                s if s >= 0.50 => summary.fair_count += 1,
                s if s >= 0.30 => summary.poor_count += 1,
                _ => summary.bad_count += 1,
            }

            if r.flags.contains(&IssueCategory::LikelyClouds) {
                summary.cloud_events_detected += 1;
            }
            if r.flags.contains(&IssueCategory::FocusDrift) {
                summary.focus_drift_detected = true;
            }
            if r.flags.iter().any(|flag| {
                matches!(
                    flag,
                    IssueCategory::TrackingError
                        | IssueCategory::PointingJump
                        | IssueCategory::PointingDrift
                )
            }) {
                summary.tracking_issues_detected = true;
            }
            if r.flags.contains(&IssueCategory::OffTarget) {
                summary.out_of_target_count += 1;
            }
            if r.flags.contains(&IssueCategory::PlateSolveFailed) {
                summary.plate_solve_failed_count += 1;
            }
            if r.flags.contains(&IssueCategory::SatelliteTrailRisk) {
                summary.satellite_risk_count += 1;
            }
        }

        summary
    }
}

fn push_issue(flags: &mut Vec<IssueCategory>, issue: IssueCategory) {
    if !flags.contains(&issue) {
        flags.push(issue);
    }
}

fn pointing_normalized(pointing: &PointingQuality) -> Option<f64> {
    if pointing.pixel_solved {
        // A stable-offset segment is deliberately framed: score its frames on
        // the residual from the segment's own center, not the target offset,
        // so consistent framing does not depress the whole sequence.
        let fraction = if pointing.flags.contains(&IssueCategory::StableOffset) {
            pointing
                .reference_field_fraction
                .or(pointing.field_fraction_offset)?
        } else {
            pointing.field_fraction_offset?
        };
        let base = (1.0 - ((fraction - 0.02) / 0.18)).clamp(0.0, 1.0);
        if pointing.flags.iter().any(|flag| {
            matches!(
                flag,
                IssueCategory::OffTarget
                    | IssueCategory::PointingJump
                    | IssueCategory::PointingDrift
            )
        }) {
            Some(base.min(0.10))
        } else {
            Some(base)
        }
    } else if pointing.solve_failed && pointing.image_quality_evidence {
        // Failure is weak evidence by itself. It lowers the score modestly
        // when comparable frames solve, while regrading still requires an
        // independent quality signal.
        Some(0.50)
    } else {
        None
    }
}

fn apply_pointing_score(score: f64, pointing: Option<&PointingQuality>) -> f64 {
    let Some(pointing) = pointing else {
        return score;
    };
    if pointing.flags.contains(&IssueCategory::OffTarget) {
        score.min(0.20)
    } else if pointing.flags.iter().any(|flag| {
        matches!(
            flag,
            IssueCategory::PointingJump | IssueCategory::PointingDrift
        )
    }) {
        score.min(0.30)
    } else {
        score
    }
}

/// Group solved centers by spatial connectivity. Single-link clustering is
/// intentional: a gradual tracking drift remains one cluster through its
/// short consecutive steps, while a discrete framing change forms another.
fn pointing_cluster_labels(points: &[(usize, f64, f64)], threshold_arcsec: f64) -> Vec<usize> {
    fn find(parent: &mut [usize], value: usize) -> usize {
        if parent[value] != value {
            parent[value] = find(parent, parent[value]);
        }
        parent[value]
    }

    let mut parent = (0..points.len()).collect::<Vec<_>>();
    for left in 0..points.len() {
        for right in (left + 1)..points.len() {
            let distance =
                (points[left].1 - points[right].1).hypot(points[left].2 - points[right].2);
            if distance <= threshold_arcsec {
                let left_root = find(&mut parent, left);
                let right_root = find(&mut parent, right);
                if left_root != right_root {
                    parent[right_root] = left_root;
                }
            }
        }
    }

    let mut labels = std::collections::HashMap::new();
    (0..points.len())
        .map(|position| {
            let root = find(&mut parent, position);
            let next = labels.len();
            *labels.entry(root).or_insert(next)
        })
        .collect()
}

/// Project a solved sky position into a gnomonic plane centered on `origin`.
/// This supports relative tracking analysis when no authoritative target
/// coordinate is available and remains correct across RA=0 and near poles.
fn tangent_plane_offset_arcsec(origin: (f64, f64), position: (f64, f64)) -> Option<(f64, f64)> {
    let delta_ra = (position.0 - origin.0).to_radians();
    let dec = position.1.to_radians();
    let origin_dec = origin.1.to_radians();
    let denominator = origin_dec.sin() * dec.sin() + origin_dec.cos() * dec.cos() * delta_ra.cos();
    if denominator <= 1e-12 {
        return None;
    }
    let radians_to_arcsec = 180.0 / std::f64::consts::PI * 3600.0;
    let east = dec.cos() * delta_ra.sin() / denominator * radians_to_arcsec;
    let north = (origin_dec.cos() * dec.sin() - origin_dec.sin() * dec.cos() * delta_ra.cos())
        / denominator
        * radians_to_arcsec;
    (east.is_finite() && north.is_finite()).then_some((east, north))
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) * 0.5
    } else {
        sorted[middle]
    }
}

fn theil_sen_slope(times: &[f64], values: &[f64]) -> f64 {
    let mut slopes = Vec::new();
    for i in 0..times.len() {
        for j in (i + 1)..times.len() {
            let dt = times[j] - times[i];
            if dt.abs() > f64::EPSILON {
                slopes.push((values[j] - values[i]) / dt);
            }
        }
    }
    median(&slopes)
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

    // Spatial metrics are only present when computed by psf-guard itself
    // (N.I.N.A. does not produce them).
    let dead_cell_fraction = metadata["DeadCellFraction"].as_f64();
    let bg_cell_spread = metadata["BgCellSpread"].as_f64();

    ImageMetrics {
        image_id,
        timestamp,
        session_id: metadata["SessionId"]
            .as_str()
            .or_else(|| metadata["SessionID"].as_str())
            .map(str::to_string),
        star_count,
        hfr,
        eccentricity,
        snr,
        background,
        dead_cell_fraction,
        bg_cell_spread,
        transparency: metadata["Transparency"].as_f64(),
        extinction_cell_fraction: metadata["ExtinctionCellFraction"].as_f64(),
        star_cell_drop_fraction: None,
        bg_cell_rise_fraction: None,
        bg_cell_fall_fraction: None,
        bg_glow_max: None,
        astrometry: None,
        satellite: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_image(id: i32, ts: i64, stars: f64, hfr: f64) -> ImageMetrics {
        ImageMetrics {
            image_id: id,
            timestamp: Some(ts),
            session_id: None,
            star_count: Some(stars),
            hfr: Some(hfr),
            eccentricity: None,
            snr: None,
            background: None,
            dead_cell_fraction: None,
            bg_cell_spread: None,
            transparency: None,
            extinction_cell_fraction: None,
            star_cell_drop_fraction: None,
            bg_cell_rise_fraction: None,
            bg_cell_fall_fraction: None,
            bg_glow_max: None,
            astrometry: None,
            satellite: None,
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
            session_id: None,
            star_count: Some(stars),
            hfr: Some(hfr),
            eccentricity: Some(ecc),
            snr: Some(snr),
            background: Some(bg),
            dead_cell_fraction: None,
            bg_cell_spread: None,
            transparency: None,
            extinction_cell_fraction: None,
            star_cell_drop_fraction: None,
            bg_cell_rise_fraction: None,
            bg_cell_fall_fraction: None,
            bg_glow_max: None,
            astrometry: None,
            satellite: None,
        }
    }

    fn make_spatial_image(
        id: i32,
        ts: i64,
        stars: f64,
        hfr: f64,
        dead: f64,
        bg_spread: f64,
    ) -> ImageMetrics {
        ImageMetrics {
            image_id: id,
            timestamp: Some(ts),
            session_id: None,
            star_count: Some(stars),
            hfr: Some(hfr),
            eccentricity: None,
            snr: None,
            background: None,
            dead_cell_fraction: Some(dead),
            bg_cell_spread: Some(bg_spread),
            transparency: None,
            extinction_cell_fraction: None,
            star_cell_drop_fraction: None,
            bg_cell_rise_fraction: None,
            bg_cell_fall_fraction: None,
            bg_glow_max: None,
            astrometry: None,
            satellite: None,
        }
    }

    /// Clean spatial frame with photometric signals attached.
    fn make_photometric_image(
        id: i32,
        ts: i64,
        transparency: f64,
        extinction: f64,
        star_cell_drop: f64,
        bg_cell_rise: f64,
    ) -> ImageMetrics {
        let mut m = make_spatial_image(id, ts, 4700.0, 2.55, 0.02, 0.05);
        m.transparency = Some(transparency);
        m.extinction_cell_fraction = Some(extinction);
        m.star_cell_drop_fraction = Some(star_cell_drop);
        m.bg_cell_rise_fraction = Some(bg_cell_rise);
        m.bg_cell_fall_fraction = Some(0.0);
        m
    }

    fn solved_astrometry(
        east: f64,
        north: f64,
        field: f64,
        target_in_frame: bool,
    ) -> AstrometryFrameMetrics {
        AstrometryFrameMetrics {
            pixel_solved: true,
            solve_failed: false,
            image_quality_evidence: true,
            solved_center_ra_deg: None,
            solved_center_dec_deg: None,
            east_offset_arcsec: Some(east),
            north_offset_arcsec: Some(north),
            separation_arcsec: Some(east.hypot(north)),
            target_in_frame: Some(target_in_frame),
            field_short_axis_arcsec: Some(field),
            matched_stars: Some(30),
            rms_arcsec: Some(0.8),
            error: None,
        }
    }

    fn failed_astrometry() -> AstrometryFrameMetrics {
        AstrometryFrameMetrics {
            solve_failed: true,
            image_quality_evidence: true,
            error: Some("no matching field".to_string()),
            ..Default::default()
        }
    }

    fn relative_solved_astrometry(ra_deg: f64, dec_deg: f64, field: f64) -> AstrometryFrameMetrics {
        AstrometryFrameMetrics {
            pixel_solved: true,
            image_quality_evidence: true,
            solved_center_ra_deg: Some(ra_deg),
            solved_center_dec_deg: Some(dec_deg),
            field_short_axis_arcsec: Some(field),
            matched_stars: Some(30),
            rms_arcsec: Some(0.8),
            ..Default::default()
        }
    }

    #[test]
    fn out_of_target_solution_reduces_score_and_recommends_regrade() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let mut images: Vec<_> = (0..6)
            .map(|i| make_full_image(i, i as i64 * 300, 500.0, 2.5, 1000.0, 20.0, 0.4))
            .collect();
        for image in &mut images {
            image.astrometry = Some(solved_astrometry(10.0, 5.0, 2000.0, true));
        }
        images[3].astrometry = Some(solved_astrometry(600.0, 0.0, 2000.0, false));

        let sequence = &analyzer.analyze(&images, 1, "target", "L")[0];
        let result = &sequence.images[3];
        assert!(result.quality_score <= 0.20);
        assert_eq!(result.category, Some(IssueCategory::OffTarget));
        assert!(result.flags.contains(&IssueCategory::OffTarget));
        assert!(result
            .regrade_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("Off target")));
        assert_eq!(sequence.summary.out_of_target_count, 1);
    }

    #[test]
    fn opposite_hemisphere_solution_remains_off_target_without_plane_offsets() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let mut images: Vec<_> = (0..6)
            .map(|i| make_full_image(i, i as i64 * 300, 500.0, 2.5, 1000.0, 20.0, 0.4))
            .collect();
        for image in &mut images {
            image.astrometry = Some(solved_astrometry(10.0, 5.0, 2000.0, true));
        }
        let mut far = solved_astrometry(0.0, 0.0, 2000.0, false);
        far.east_offset_arcsec = None;
        far.north_offset_arcsec = None;
        far.separation_arcsec = Some(648_000.0);
        images[3].astrometry = Some(far);

        let result = &analyzer.analyze(&images, 1, "target", "L")[0].images[3];
        assert!(result.flags.contains(&IssueCategory::OffTarget));
        assert!(result.regrade_reason.is_some());
        assert!(result.quality_score <= 0.20);
    }

    #[test]
    fn isolated_solve_failure_warns_without_automatic_regrade() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let mut images: Vec<_> = (0..6)
            .map(|i| make_full_image(i, i as i64 * 300, 500.0, 2.5, 1000.0, 20.0, 0.4))
            .collect();
        for image in &mut images {
            image.astrometry = Some(solved_astrometry(10.0, 5.0, 2000.0, true));
        }
        images[3].astrometry = Some(failed_astrometry());

        let sequence = &analyzer.analyze(&images, 1, "target", "L")[0];
        let result = &sequence.images[3];
        assert_eq!(result.category, Some(IssueCategory::PlateSolveFailed));
        assert!(result.regrade_reason.is_none());
        assert!(result.quality_score > 0.30);
    }

    #[test]
    fn solve_failure_plus_cloud_is_regradeable() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let mut images: Vec<_> = (0..6)
            .map(|i| make_photometric_image(i, i as i64 * 300, 1.0, 0.0, 0.0, 0.0))
            .collect();
        for image in &mut images {
            image.astrometry = Some(solved_astrometry(10.0, 5.0, 2000.0, true));
        }
        images[3].extinction_cell_fraction = Some(0.12);
        images[3].astrometry = Some(failed_astrometry());

        let result = &analyzer.analyze(&images, 1, "target", "L")[0].images[3];
        assert!(result.flags.contains(&IssueCategory::LikelyClouds));
        assert!(result.flags.contains(&IssueCategory::PlateSolveFailed));
        assert!(result
            .regrade_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("Plate solve failed")));
        assert!(result.quality_score <= 0.30);
    }

    #[test]
    fn relative_solved_centers_detect_tracking_jump_without_target_coordinates() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let mut images: Vec<_> = (0..6)
            .map(|i| make_full_image(i, i as i64 * 300, 500.0, 2.5, 1000.0, 20.0, 0.4))
            .collect();
        let centers = [359.990, 359.991, 0.200, 359.992, 359.991, 359.990];
        for (image, ra) in images.iter_mut().zip(centers) {
            image.astrometry = Some(relative_solved_astrometry(ra, 0.0, 2000.0));
        }

        let result = &analyzer.analyze(&images, 1, "target", "L")[0].images[2];
        assert!(!result.pointing.as_ref().unwrap().expected_target);
        assert!(result.flags.contains(&IssueCategory::PointingJump));
        assert!(result
            .regrade_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("Tracking lost")));
        assert!(result.quality_score <= 0.30);
    }

    #[test]
    fn detrended_scatter_does_not_hide_progressive_tracking_drift() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let mut images: Vec<_> = (0..6)
            .map(|i| make_full_image(i, i as i64 * 600, 500.0, 2.5, 1000.0, 20.0, 0.4))
            .collect();
        for (i, image) in images.iter_mut().enumerate() {
            image.astrometry = Some(relative_solved_astrometry(
                120.0 + i as f64 * 0.02,
                10.0,
                2000.0,
            ));
        }

        let sequence = &analyzer.analyze(&images, 1, "target", "L")[0];
        let affected = sequence
            .images
            .iter()
            .filter(|result| result.flags.contains(&IssueCategory::PointingDrift))
            .collect::<Vec<_>>();
        assert!(!affected.is_empty());
        assert!(affected.iter().all(|result| result.quality_score <= 0.30));
        assert!(affected
            .iter()
            .all(|result| result.regrade_reason.is_some()));
    }

    #[test]
    fn stable_framing_offset_warns_without_regrade_or_score_cap() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let mut images: Vec<_> = (0..6)
            .map(|i| make_full_image(i, i as i64 * 300, 500.0, 2.5, 1000.0, 20.0, 0.4))
            .collect();
        // Deliberate framing: every frame is 25% of the field from the target,
        // target still in frame, and the whole segment agrees.
        for image in &mut images {
            image.astrometry = Some(solved_astrometry(500.0, 0.0, 2000.0, true));
        }

        let sequence = &analyzer.analyze(&images, 1, "target", "L")[0];
        for result in &sequence.images {
            assert!(result.flags.contains(&IssueCategory::StableOffset));
            assert!(!result.flags.contains(&IssueCategory::OffTarget));
            assert!(
                result.regrade_reason.is_none(),
                "stable offsets must never auto-reject"
            );
            assert!(result.quality_score > 0.30);
        }
        assert_eq!(sequence.summary.out_of_target_count, 0);
    }

    #[test]
    fn deliberate_mid_session_reframing_forms_two_stable_clusters() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let mut images: Vec<_> = (0..6)
            .map(|i| make_full_image(i, i as i64 * 300, 500.0, 2.5, 1000.0, 20.0, 0.4))
            .collect();
        for image in &mut images[..3] {
            image.astrometry = Some(solved_astrometry(10.0, 0.0, 2000.0, true));
        }
        // A deliberate second composition, 25% of the field from target.
        for image in &mut images[3..] {
            image.astrometry = Some(solved_astrometry(500.0, 0.0, 2000.0, true));
        }

        let sequence = &analyzer.analyze(&images, 1, "target", "L")[0];
        for result in &sequence.images[3..] {
            assert!(result.flags.contains(&IssueCategory::StableOffset));
            assert!(!result.flags.contains(&IssueCategory::OffTarget));
            assert!(!result.flags.contains(&IssueCategory::PointingDrift));
            assert!(result.regrade_reason.is_none());
            assert!(result.quality_score > 0.30);
        }
        assert_eq!(sequence.summary.out_of_target_count, 0);
    }

    #[test]
    fn departure_from_a_stably_offset_segment_is_off_target() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let mut images: Vec<_> = (0..6)
            .map(|i| make_full_image(i, i as i64 * 300, 500.0, 2.5, 1000.0, 20.0, 0.4))
            .collect();
        for image in &mut images {
            image.astrometry = Some(solved_astrometry(500.0, 0.0, 2000.0, true));
        }
        images[3].astrometry = Some(solved_astrometry(1100.0, 0.0, 2000.0, true));

        let sequence = &analyzer.analyze(&images, 1, "target", "L")[0];
        let excursion = &sequence.images[3];
        assert_eq!(excursion.category, Some(IssueCategory::OffTarget));
        assert!(excursion.regrade_reason.is_some());
        assert!(excursion.quality_score <= 0.20);
        assert!(sequence.images[1].regrade_reason.is_none());
    }

    #[test]
    fn multi_frame_excursion_that_returns_is_flagged_as_a_jump() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig::default());
        let mut images: Vec<_> = (0..6)
            .map(|i| make_full_image(i, i as i64 * 300, 500.0, 2.5, 1000.0, 20.0, 0.4))
            .collect();
        let centers = [120.00, 120.00, 120.05, 120.05, 120.00, 120.00];
        for (image, ra) in images.iter_mut().zip(centers) {
            image.astrometry = Some(relative_solved_astrometry(ra, 0.0, 2000.0));
        }

        let sequence = &analyzer.analyze(&images, 1, "target", "L")[0];
        for idx in [2, 3] {
            let result = &sequence.images[idx];
            assert!(
                result.flags.contains(&IssueCategory::PointingJump),
                "frame {idx} should be part of the jump run"
            );
            assert!(result.regrade_reason.is_some());
            assert!(result.quality_score <= 0.30);
        }
        assert!(!sequence.images[1]
            .flags
            .contains(&IssueCategory::PointingJump));
    }

    #[test]
    fn tangent_plane_offsets_cross_ra_zero_in_the_short_direction() {
        let (east, north) = tangent_plane_offset_arcsec((359.99, 0.0), (0.01, 0.0)).unwrap();
        assert!((east - 72.0).abs() < 0.1, "east offset was {east}");
        assert!(north.abs() < 1e-6);
    }

    #[test]
    fn test_small_cloud_classified_from_localized_extinction() {
        // A small cloud dims a patch of stars in one frame: star count, HFR
        // and even the composite score stay good - only the photometric
        // extinction map notices.
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        });
        let mut images: Vec<ImageMetrics> = (0..8)
            .map(|i| make_photometric_image(i, i as i64 * 300, 1.0, 0.0, 0.0, 0.0))
            .collect();
        images[5] = make_photometric_image(5, 5 * 300, 0.96, 0.10, 0.04, 0.0);

        let results = analyzer.analyze(&images, 1, "test", "R");
        let seq = &results[0];
        assert_eq!(
            seq.images[5].category,
            Some(IssueCategory::LikelyClouds),
            "localized extinction should classify as small cloud: {:?}",
            seq.images[5].details
        );
        assert_eq!(seq.images[4].category, None);
    }

    #[test]
    fn test_dark_patch_corroborates_weak_extinction_as_cloud() {
        // A dark cloud blocks skyglow: localized background FALL plus
        // extinction just below the standalone threshold. The dark patch
        // corroborates it into a cloud classification.
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        });
        let mut images: Vec<ImageMetrics> = (0..8)
            .map(|i| make_photometric_image(i, i as i64 * 300, 1.0, 0.0, 0.0, 0.0))
            .collect();
        let mut dark = make_photometric_image(5, 5 * 300, 0.97, 0.04, 0.0, 0.0);
        dark.bg_cell_fall_fraction = Some(0.10);
        images[5] = dark;

        let results = analyzer.analyze(&images, 1, "test", "R");
        let seq = &results[0];
        assert_eq!(
            seq.images[5].category,
            Some(IssueCategory::LikelyClouds),
            "dark patch + weak extinction should classify as cloud: {:?}",
            seq.images[5].details
        );
        // Without the dark patch the same weak extinction stays unclassified.
        let mut images2: Vec<ImageMetrics> = (0..8)
            .map(|i| make_photometric_image(i, i as i64 * 300, 1.0, 0.0, 0.0, 0.0))
            .collect();
        images2[5] = make_photometric_image(5, 5 * 300, 0.97, 0.04, 0.0, 0.0);
        let results2 = analyzer.analyze(&images2, 1, "test", "R");
        assert_eq!(results2[0].images[5].category, None);
    }

    #[test]
    fn test_thin_veil_classified_from_transparency() {
        // Uniform 30% dimming: star counts unchanged, no dead cells, no
        // localized extinction - only the global flux ratio drops.
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        });
        let mut images: Vec<ImageMetrics> = (0..8)
            .map(|i| make_photometric_image(i, i as i64 * 300, 1.0, 0.0, 0.0, 0.0))
            .collect();
        images[6] = make_photometric_image(6, 6 * 300, 0.70, 0.0, 0.0, 0.0);

        let results = analyzer.analyze(&images, 1, "test", "R");
        let seq = &results[0];
        assert_eq!(
            seq.images[6].category,
            Some(IssueCategory::LikelyClouds),
            "transparency dip should classify as veil: {:?}",
            seq.images[6].details
        );
        // The veiled frame must also score worse than its clean neighbors.
        assert!(seq.images[6].quality_score < seq.images[3].quality_score);
    }

    #[test]
    fn test_errant_light_classified_from_bg_cell_rise() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        });
        let mut images: Vec<ImageMetrics> = (0..8)
            .map(|i| make_photometric_image(i, i as i64 * 300, 1.0, 0.0, 0.0, 0.0))
            .collect();
        images[4] = make_photometric_image(4, 4 * 300, 1.0, 0.0, 0.0, 0.10);

        let results = analyzer.analyze(&images, 1, "test", "R");
        let seq = &results[0];
        assert_eq!(
            seq.images[4].category,
            Some(IssueCategory::SkyBrightening),
            "transient localized bg rise should classify as errant light: {:?}",
            seq.images[4].details
        );
    }

    #[test]
    fn test_static_glow_classified_even_when_present_all_session() {
        // Glow present from frame 1 (lit haze at the field edge): temporal
        // detectors see nothing, the within-frame signal still flags it.
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        });
        let images: Vec<ImageMetrics> = (0..8)
            .map(|i| {
                let mut m = make_photometric_image(i, i as i64 * 300, 1.0, 0.0, 0.0, 0.0);
                m.bg_glow_max = Some(0.045); // static, every frame
                m
            })
            .collect();

        let results = analyzer.analyze(&images, 1, "NGC 6820", "R");
        for r in &results[0].images {
            assert_eq!(
                r.category,
                Some(IssueCategory::SkyBrightening),
                "static glow should classify every affected frame: {:?}",
                r.details
            );
        }
    }

    #[test]
    fn test_clean_photometric_sequence_has_no_classifications() {
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        });
        // Realistic clean jitter: transparency 0.97-1.03, tiny fractions.
        let images: Vec<ImageMetrics> = (0..15)
            .map(|i| {
                let jitter = (i % 3) as f64;
                make_photometric_image(
                    i,
                    i as i64 * 300,
                    0.97 + jitter * 0.03,
                    jitter * 0.01,
                    jitter * 0.01,
                    jitter * 0.01,
                )
            })
            .collect();

        let results = analyzer.analyze(&images, 1, "test", "R");
        for r in &results[0].images {
            assert_eq!(
                r.category, None,
                "clean frame {} classified: {:?}",
                r.image_id, r.details
            );
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
                flags: vec![],
                normalized_metrics: NormalizedMetrics {
                    star_count: Some(1.0),
                    hfr: Some(1.0),
                    eccentricity: None,
                    snr: None,
                    background: None,
                    spatial_coverage: None,
                    transparency: None,
                    pointing: None,
                },
                pointing: None,
                satellite: None,
                regrade_reason: None,
                details: None,
            },
            ImageQualityResult {
                image_id: 2,
                quality_score: 0.75,
                temporal_anomaly_score: 0.0,
                category: None,
                flags: vec![],
                normalized_metrics: NormalizedMetrics {
                    star_count: Some(0.8),
                    hfr: Some(0.7),
                    eccentricity: None,
                    snr: None,
                    background: None,
                    spatial_coverage: None,
                    transparency: None,
                    pointing: None,
                },
                pointing: None,
                satellite: None,
                regrade_reason: None,
                details: None,
            },
            ImageQualityResult {
                image_id: 3,
                quality_score: 0.25,
                temporal_anomaly_score: 0.4,
                category: Some(IssueCategory::LikelyClouds),
                flags: vec![IssueCategory::LikelyClouds],
                normalized_metrics: NormalizedMetrics {
                    star_count: Some(0.1),
                    hfr: Some(0.3),
                    eccentricity: None,
                    snr: None,
                    background: None,
                    spatial_coverage: None,
                    transparency: None,
                    pointing: None,
                },
                pointing: None,
                satellite: None,
                regrade_reason: None,
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
    fn pixel_aligned_high_satellite_risk_recommends_reviewed_rejection() {
        let mut images = vec![
            make_image(1, 1000, 100.0, 2.0),
            make_image(2, 1060, 100.0, 2.0),
            make_image(3, 1120, 100.0, 2.0),
        ];
        images[1].satellite = Some(SatelliteFrameMetrics {
            predicted_tracks: 2,
            potentially_bright_count: 1,
            high_risk_count: 1,
            maximum_bright_trail_risk: 0.82,
            pixel_alignment_attempted: true,
            pixel_aligned_count: 1,
            pixel_aligned_high_risk_count: 1,
            reject_recommended: true,
            association: "predicted_with_pixel_alignment".into(),
        });

        let result = SequenceAnalyzer::new(SequenceAnalyzerConfig::default())
            .analyze(&images, 1, "target", "L");
        let affected = result
            .iter()
            .flat_map(|sequence| &sequence.images)
            .find(|image| image.image_id == 2)
            .unwrap();
        assert!(affected.flags.contains(&IssueCategory::SatelliteTrailRisk));
        assert!(affected.quality_score <= 0.35);
        assert!(affected
            .regrade_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("Pixel-aligned bright satellite")));
        assert!(affected
            .details
            .as_deref()
            .is_some_and(|details| details.contains("Pixel corridor alignment found 1")));
        assert_eq!(result[0].summary.satellite_risk_count, 1);
    }

    #[test]
    fn possible_satellite_risk_warns_without_regrade() {
        let mut images = vec![
            make_image(1, 1000, 100.0, 2.0),
            make_image(2, 1060, 100.0, 2.0),
            make_image(3, 1120, 100.0, 2.0),
        ];
        images[1].satellite = Some(SatelliteFrameMetrics {
            predicted_tracks: 1,
            potentially_bright_count: 1,
            high_risk_count: 0,
            maximum_bright_trail_risk: 0.42,
            pixel_alignment_attempted: true,
            pixel_aligned_count: 0,
            pixel_aligned_high_risk_count: 0,
            reject_recommended: false,
            association: "predicted_pixel_checked".into(),
        });

        let result = SequenceAnalyzer::new(SequenceAnalyzerConfig::default())
            .analyze(&images, 1, "target", "L");
        let affected = result[0]
            .images
            .iter()
            .find(|image| image.image_id == 2)
            .unwrap();
        assert!(affected.flags.contains(&IssueCategory::SatelliteTrailRisk));
        assert!(affected.quality_score <= 0.75);
        assert!(affected.regrade_reason.is_none());
    }

    #[test]
    fn high_satellite_prediction_without_pixel_alignment_does_not_regrade() {
        let mut images = vec![
            make_image(1, 1000, 100.0, 2.0),
            make_image(2, 1060, 100.0, 2.0),
            make_image(3, 1120, 100.0, 2.0),
        ];
        images[1].satellite = Some(SatelliteFrameMetrics {
            predicted_tracks: 2,
            potentially_bright_count: 2,
            high_risk_count: 2,
            maximum_bright_trail_risk: 0.94,
            pixel_alignment_attempted: true,
            pixel_aligned_count: 0,
            pixel_aligned_high_risk_count: 0,
            reject_recommended: false,
            association: "predicted_pixel_checked".into(),
        });

        let result = SequenceAnalyzer::new(SequenceAnalyzerConfig::default())
            .analyze(&images, 1, "target", "L");
        let affected = result[0]
            .images
            .iter()
            .find(|image| image.image_id == 2)
            .unwrap();
        assert!(affected.flags.contains(&IssueCategory::SatelliteTrailRisk));
        assert!(affected.quality_score <= 0.75);
        assert!(affected.quality_score > 0.35);
        assert!(affected.regrade_reason.is_none());
        assert!(affected
            .details
            .as_deref()
            .is_some_and(|details| details.contains("found no matching trail")));
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
                spatial: 0.0,
                transparency: 0.0,
                pointing: 0.0,
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
            spatial: 0.0,
            transparency: 0.0,
            pointing: 0.0,
        };
        let normalized = weights.normalized();
        let sum = normalized.star_count
            + normalized.hfr
            + normalized.eccentricity
            + normalized.snr
            + normalized.background
            + normalized.spatial
            + normalized.transparency
            + normalized.pointing;
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
    fn test_weight_normalization_of_defaults_preserves_ratios() {
        // Defaults sum to 1.20 (the spatial weight is additive so that the
        // relative ratios of the original five metrics are unchanged);
        // normalized() rescales to 1.0.
        let weights = QualityWeights::default();
        let normalized = weights.normalized();
        let sum = normalized.star_count
            + normalized.hfr
            + normalized.eccentricity
            + normalized.snr
            + normalized.background
            + normalized.spatial
            + normalized.transparency
            + normalized.pointing;
        assert!((sum - 1.0).abs() < 1e-10, "sum should be 1.0, got {}", sum);
        // star_count : hfr ratio (0.30 : 0.25) preserved
        assert!((normalized.star_count / normalized.hfr - 0.30 / 0.25).abs() < 1e-10);
    }

    #[test]
    fn test_weight_normalization_all_zero_returns_defaults() {
        let weights = QualityWeights {
            star_count: 0.0,
            hfr: 0.0,
            eccentricity: 0.0,
            snr: 0.0,
            background: 0.0,
            spatial: 0.0,
            transparency: 0.0,
            pointing: 0.0,
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

    #[test]
    fn test_subtle_occlusion_flagged_despite_stable_global_metrics() {
        // Modeled on NGC 6820 2026-06-30 frames 0004-0011: global star count
        // and HFR stay within normal variation while an occluder kills 15% of
        // the frame's grid cells.
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        });

        let mut images: Vec<ImageMetrics> = (0..8)
            .map(|i| make_spatial_image(i, i as i64 * 300, 4700.0, 2.55, 0.02, 0.05))
            .collect();
        // Frame 6-7: corner occluded; star count barely moves.
        images[6] = make_spatial_image(6, 6 * 300, 4650.0, 2.60, 0.17, 0.10);
        images[7] = make_spatial_image(7, 7 * 300, 4600.0, 2.58, 0.21, 0.12);

        let results = analyzer.analyze(&images, 1, "NGC 6820", "R");
        let seq = &results[0];

        assert_eq!(
            seq.images[6].category,
            Some(IssueCategory::PossibleObstruction),
            "subtle occlusion should classify as obstruction, got {:?} ({:?})",
            seq.images[6].category,
            seq.images[6].details,
        );
        assert_eq!(
            seq.images[7].category,
            Some(IssueCategory::PossibleObstruction)
        );
        // Clean frames stay unclassified.
        assert_eq!(seq.images[3].category, None);
        // Occluded frames must score below clean frames.
        assert!(seq.images[6].quality_score < seq.images[3].quality_score);
    }

    #[test]
    fn test_progressive_occlusion_does_not_normalize_into_baseline() {
        // "Boiling frog": an occluder drifts across the field over many
        // frames (tree line as the target sets). Every frame is only a bit
        // worse than the previous one; without baseline freezing the EWMA
        // absorbs the trend and late frames stop being flagged.
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        });

        // 5 clean frames, then dead fraction grows 0.08 per frame while star
        // count decays, mirroring the validated sequences.
        let mut images: Vec<ImageMetrics> = (0..5)
            .map(|i| make_spatial_image(i, i as i64 * 300, 4700.0, 2.55, 0.02, 0.05))
            .collect();
        for j in 0..10 {
            let dead = (0.02 + 0.08 * (j + 1) as f64).min(1.0);
            let stars = 4700.0 * (1.0 - dead * 0.9);
            images.push(make_spatial_image(
                5 + j,
                (5 + j as i64) * 300,
                stars,
                2.6,
                dead,
                0.05 + dead * 0.3,
            ));
        }

        let results = analyzer.analyze(&images, 1, "NGC 6820", "R");
        let seq = &results[0];

        // Every frame from onset (dead >= 0.15) must be classified.
        for r in seq.images.iter().skip(6) {
            assert!(
                r.category.is_some(),
                "progressively occluded frame {} lost its classification (score {:.2})",
                r.image_id,
                r.quality_score
            );
        }
        // Late, heavily occluded frames must score badly even though each
        // step was small.
        let last = seq.images.last().unwrap();
        assert!(
            last.quality_score < 0.3,
            "fully occluded frame should score < 0.3, got {:.2}",
            last.quality_score
        );
    }

    #[test]
    fn test_single_frame_dead_cell_blip_is_not_obstruction() {
        // Regression (code review): a one-frame transient (wind gust, wisp)
        // that nudges the dead-cell fraction past the rise threshold must not
        // classify an otherwise excellent frame - occlusion needs neighbor
        // corroboration.
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        });

        let mut images: Vec<ImageMetrics> = (0..10)
            .map(|i| make_spatial_image(i, i as i64 * 300, 4700.0, 2.55, 0.02, 0.05))
            .collect();
        // Single-frame blip: 0.02 -> 0.12 -> 0.02.
        images[5] = make_spatial_image(5, 5 * 300, 4650.0, 2.56, 0.12, 0.06);

        let results = analyzer.analyze(&images, 1, "NGC 6820", "R");
        let seq = &results[0];
        assert_ne!(
            seq.images[5].category,
            Some(IssueCategory::PossibleObstruction),
            "single-frame blip must not be classified as obstruction: {:?}",
            seq.images[5].details
        );
    }

    #[test]
    fn test_permanent_condition_change_does_not_flag_rest_of_session() {
        // Regression (code review): a lasting shift (moonrise, light dome)
        // freezes the EWMA baselines at most baseline_freeze_max_frames;
        // after that the new regime is accepted and later frames are neither
        // temporally anomalous nor classified as clouds/obstruction.
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        });

        let mut images: Vec<ImageMetrics> = (0..5)
            .map(|i| make_image(i, i as i64 * 300, 4000.0, 2.5))
            .collect();
        // Permanent 40% star-count drop for 30 frames (no spatial data -
        // this is the DB-metadata-only path).
        for j in 0..30 {
            images.push(make_image(5 + j, (5 + j as i64) * 300, 2400.0, 2.5));
        }

        let results = analyzer.analyze(&images, 1, "test", "L");
        let seq = &results[0];

        for r in seq.images.iter().skip(25) {
            assert!(
                r.temporal_anomaly_score < 0.15,
                "frame {} after regime re-establishment still anomalous ({:.2})",
                r.image_id,
                r.temporal_anomaly_score
            );
            assert!(
                !matches!(
                    r.category,
                    Some(IssueCategory::LikelyClouds) | Some(IssueCategory::PossibleObstruction)
                ),
                "frame {} misclassified as {:?} after regime change",
                r.image_id,
                r.category
            );
        }
    }

    #[test]
    fn test_stray_light_gradient_classified_as_sky_brightening() {
        // Modeled on NGC 6820 2026-05-21 frames 0139-0141: background grid
        // spread rises sharply before any stars are lost.
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        });

        let mut images: Vec<ImageMetrics> = (0..8)
            .map(|i| make_spatial_image(i, i as i64 * 300, 3600.0, 2.55, 0.00, 0.05))
            .collect();
        images[7] = make_spatial_image(7, 7 * 300, 3550.0, 2.56, 0.00, 0.42);

        let results = analyzer.analyze(&images, 1, "NGC 6820", "G");
        let seq = &results[0];

        assert_eq!(
            seq.images[7].category,
            Some(IssueCategory::SkyBrightening),
            "stray-light gradient should classify as sky brightening, got {:?}",
            seq.images[7].category
        );
    }

    #[test]
    fn test_clean_sequence_with_spatial_metrics_has_no_false_positives() {
        // Clean-frame envelope from four validated nights: dead fraction
        // jitters 0.00-0.04, bg spread 0.02-0.09.
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        });

        let images: Vec<ImageMetrics> = (0..20)
            .map(|i| {
                let jitter = (i % 3) as f64;
                make_spatial_image(
                    i,
                    i as i64 * 300,
                    4500.0 + jitter * 150.0,
                    2.5 + jitter * 0.05,
                    jitter * 0.02,
                    0.03 + jitter * 0.02,
                )
            })
            .collect();

        let results = analyzer.analyze(&images, 1, "NGC 6820", "R");
        let seq = &results[0];

        for r in &seq.images {
            assert_ne!(
                r.category,
                Some(IssueCategory::PossibleObstruction),
                "clean frame {} misclassified as obstruction ({:?})",
                r.image_id,
                r.details
            );
        }
        let poor = seq.images.iter().filter(|r| r.quality_score < 0.3).count();
        assert_eq!(poor, 0, "clean sequence should have no poor frames");
    }

    #[test]
    fn test_uniform_cloud_event_still_classified_as_clouds() {
        // Clouds veil the frame uniformly: star count collapses everywhere,
        // dead fraction stays low, background rises. Must NOT be classified
        // as obstruction.
        let analyzer = SequenceAnalyzer::new(SequenceAnalyzerConfig {
            min_sequence_length: 3,
            ..Default::default()
        });

        let mut images: Vec<ImageMetrics> = (0..8)
            .map(|i| {
                let mut m = make_full_image(i, i as i64 * 300, 300.0, 2.5, 1200.0, 45.0, 0.35);
                m.dead_cell_fraction = Some(0.02);
                m.bg_cell_spread = Some(0.05);
                m
            })
            .collect();
        let mut cloud = make_full_image(6, 6 * 300, 100.0, 3.5, 1800.0, 15.0, 0.35);
        cloud.dead_cell_fraction = Some(0.06); // uniform loss, few dead cells
        cloud.bg_cell_spread = Some(0.08);
        images[6] = cloud;

        let results = analyzer.analyze(&images, 1, "test", "L");
        let seq = &results[0];

        assert_eq!(
            seq.images[6].category,
            Some(IssueCategory::LikelyClouds),
            "uniform veiling should classify as clouds, got {:?}",
            seq.images[6].category
        );
    }
}
