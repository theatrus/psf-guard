# Cloud Detection and Image Quality Scoring -- Design Document

## 1. Overview

This document describes a multi-metric quality scoring system for PSF Guard that
detects cloud-affected frames, classifies quality issues, and produces a relative
quality score for every image in an acquisition sequence. The system builds on
metrics already computed by the codebase (HFR, FWHM, star count, background,
SNR, eccentricity, MAD) and requires no new image processing for v1.

### Goals

- Score each image 0.0--1.0 relative to the best frame in its sequence
  (same target + filter within a contiguous session).
- Detect cloud passages from sudden temporal changes in multiple metrics.
- Classify issues: clouds, obstruction, focus drift, tracking error.
- Expose analysis through a REST API for the web UI.
- Provide sensible defaults that work without per-user tuning.

### Non-Goals (v1)

- Per-pixel spatial analysis (background gradient maps, quadrant comparison).
- Machine-learning classifiers.
- Absolute quality thresholds (everything is relative to the session).

---

## 2. Available Metrics

These metrics are already computed per image and stored in the `acquiredimage`
metadata JSON or obtainable from FITS headers at analysis time.

| Metric | Source | Unit | Notes |
|--------|--------|------|-------|
| `DetectedStars` | HocusFocus / N.I.N.A. star detection | count | Primary cloud indicator |
| `HFR` | Star detection | pixels | Increases with clouds/defocus |
| `FWHM` | PSF fitting or HFR approximation | pixels | Correlated with HFR |
| `Eccentricity` | PSF fitting (sigma_x / sigma_y) | 0.0--1.0 | Tracking/guiding quality |
| `Mean` | Image statistics | ADU | Overall brightness |
| `Median` | Image statistics | ADU | Background level (robust) |
| `StdDev` | Image statistics | ADU | Contrast/noise |
| `MAD` | Image statistics | ADU | Robust noise estimate |
| `SNR` | Star detection (signal/noise) | ratio | Per-star average SNR |
| `Temperature` | FITS header CCD-TEMP | celsius | Thermal noise baseline |
| `Background` | Star detection (background_mean) | ADU | Sky background level |

### Metadata JSON Fields (from N.I.N.A. Target Scheduler)

The `acquiredimage.metadata` JSON already contains:

```json
{
  "FileName": "...",
  "FilterName": "Ha",
  "HFR": 2.5,
  "DetectedStars": 342,
  "ExposureStartTime": "2023-08-27T10:00:00Z"
}
```

For v1, the system uses `HFR`, `DetectedStars`, and `ExposureStartTime` from
metadata. Additional metrics (Median, MAD, SNR, eccentricity) can be computed
on demand from the FITS file when the file cache has resolved a path for the
image, or pre-computed and stored during a cache refresh.

---

## 3. Sequence Definition

A **sequence** is a contiguous group of images sharing the same:

- `target_id` (same astronomical target)
- `filter_name` (same narrowband/broadband filter)
- Session continuity: no gap > 60 minutes between consecutive exposures

Images are ordered by `ExposureStartTime` (or `acquireddate` from the database).
A gap exceeding 60 minutes splits the group into separate sequences. This
prevents comparing images from different nights or sessions where conditions may
be entirely different.

### Why Relative Scoring

Absolute thresholds fail because conditions vary enormously between setups:

- A Bortle 2 site has background 500 ADU; Bortle 7 has 8000 ADU.
- Fast optics (f/2) produce HFR 1.5; slow optics (f/10) produce HFR 4.0.
- Narrowband filters detect fewer stars than broadband.

By scoring each image relative to the best in its sequence, the system
automatically adapts to any equipment, site, and filter combination.

---

## 4. Temporal Analysis

### 4.1 Cloud Passage Detection

Clouds produce a characteristic multi-metric signature when they pass through
the field of view:

| Metric | Cloud Effect | Typical Magnitude |
|--------|-------------|-------------------|
| Star count | Drops sharply | 30--90% reduction |
| Background (Median) | Rises (light scatter) | 10--50% increase |
| HFR | Increases (scattering) | 15--40% increase |
| SNR | Drops | 20--60% reduction |
| Eccentricity | Unchanged | No effect |

The key insight is that clouds affect **star count, background, HFR, and SNR
simultaneously**, while other issues only affect a subset:

- Focus drift: HFR increases gradually, star count drops slowly, background
  unchanged.
- Tracking error: eccentricity increases, star count unchanged, HFR may
  increase slightly.
- Dew/frost: similar to clouds but onset is gradual rather than sudden.

### 4.2 Rolling Baseline

Instead of the current fixed-window baseline in `grading.rs`, use an
**exponentially weighted moving average (EWMA)** for each metric:

```
baseline[i] = alpha * value[i-1] + (1 - alpha) * baseline[i-1]
```

Where `alpha = 0.3` provides a ~5-frame effective window. EWMA is superior to
the current fixed-window approach because:

1. It adapts smoothly to gradual changes (temperature drift, altitude change).
2. It does not require a fixed `cloud_baseline_count` parameter.
3. It is cheap to compute (O(1) per frame vs O(n) for rolling median).

For the very first frame, initialize baseline to the frame's own value.

### 4.3 Temporal Deviation Score

For each metric at each time step, compute the deviation from the EWMA
baseline:

```
deviation[metric][i] = (value[i] - baseline[i]) / baseline[i]
```

For star count, invert the sign (a drop is bad):

```
star_deviation[i] = (baseline[i] - star_count[i]) / baseline[i]
```

A **temporal anomaly** is flagged when multiple metrics deviate simultaneously:

```
temporal_score[i] = w_stars * max(0, star_deviation[i])
                  + w_bg    * max(0, bg_deviation[i])
                  + w_hfr   * max(0, hfr_deviation[i])
                  + w_snr   * max(0, -snr_deviation[i])
```

Default weights: `w_stars = 0.40, w_bg = 0.25, w_hfr = 0.20, w_snr = 0.15`

Star count is weighted highest because it is the most reliable cloud indicator:
thin clouds reduce star count before they noticeably affect background or HFR.

---

## 5. Quality Scoring Formula

### 5.1 Per-Metric Normalization

For each metric in a sequence, normalize to [0, 1] relative to the best and
worst values observed:

```
normalized[metric][i] = (value[i] - worst) / (best - worst)
```

Where "best" and "worst" are defined per metric:

| Metric | Best = | Worst = |
|--------|--------|---------|
| Star count | max in sequence | min in sequence |
| HFR | min in sequence | max in sequence |
| FWHM | min in sequence | max in sequence |
| Eccentricity | min in sequence | max in sequence |
| SNR | max in sequence | min in sequence |
| Background | min in sequence | max in sequence |

If best == worst (all values identical), normalized = 1.0 for all frames.

### 5.2 Robust Normalization

To prevent a single extreme outlier from compressing the entire scale, use
the 5th and 95th percentiles as the normalization bounds instead of raw
min/max:

```
best_robust  = percentile(values, 5)   // for "lower is better" metrics
worst_robust = percentile(values, 95)  // for "lower is better" metrics
```

Values beyond these bounds are clamped to 0.0 or 1.0.

### 5.3 Composite Quality Score

Combine normalized metrics using a weighted sum, inspired by the PixInsight
SubframeSelector approach:

```
quality_score[i] = w_stars * norm_stars[i]
                 + w_hfr   * norm_hfr[i]
                 + w_ecc   * norm_eccentricity[i]
                 + w_snr   * norm_snr[i]
                 + w_bg    * norm_background[i]
```

**Default weights:**

| Weight | Value | Rationale |
|--------|-------|-----------|
| `w_stars` | 0.30 | Most reliable cloud/obstruction indicator |
| `w_hfr` | 0.25 | Focus and atmospheric seeing quality |
| `w_ecc` | 0.10 | Tracking/guiding quality |
| `w_snr` | 0.25 | Overall signal quality |
| `w_bg` | 0.10 | Sky conditions (light pollution, clouds) |

These weights are designed for general deep-sky imaging. The API will accept
optional weight overrides for specific target types (similar to PixInsight's
per-target-type formulas).

### 5.4 Penalty for Temporal Anomalies

Apply a multiplicative penalty when temporal analysis flags an anomaly:

```
final_score[i] = quality_score[i] * (1.0 - clamp(temporal_score[i], 0, 0.5))
```

This ensures that even if a cloud-affected frame happens to have acceptable
absolute metrics (thin clouds), the sudden change from baseline still reduces
its score.

### 5.5 Score Interpretation

| Score Range | Interpretation |
|-------------|----------------|
| 0.90 -- 1.00 | Excellent -- among the best in the sequence |
| 0.70 -- 0.89 | Good -- minor quality reduction |
| 0.50 -- 0.69 | Fair -- noticeable degradation |
| 0.30 -- 0.49 | Poor -- significant issues detected |
| 0.00 -- 0.29 | Bad -- likely unusable |

---

## 6. Issue Classification

After scoring, classify the likely cause based on which metrics deviated and
how the deviation evolved over time.

### 6.1 Classification Rules

```
IF star_count drops > 25% AND background rises > 10%:
    category = "likely_clouds"

ELSE IF star_count drops > 25% AND background stable:
    category = "possible_obstruction"
    (tree branch, dome slit, dew cap)

ELSE IF hfr increases gradually over 3+ frames AND eccentricity stable:
    category = "focus_drift"

ELSE IF eccentricity increases > 0.15 AND star_count stable:
    category = "tracking_error"

ELSE IF hfr increases AND star_count drops AND eccentricity increases:
    category = "wind_shake"
    (guiding + seeing both degraded)

ELSE IF background rises gradually AND star_count stable:
    category = "sky_brightening"
    (dawn, moon rise, light pollution event)

ELSE IF all metrics degraded simultaneously and suddenly:
    category = "likely_clouds"
    (thick cloud event)

ELSE IF score < 0.5 but no clear pattern:
    category = "unknown_degradation"
```

### 6.2 Gradual vs Sudden Detection

To distinguish "focus_drift" from "clouds", measure the rate of change:

```
rate_of_change[metric][i] = abs(value[i] - value[i-1]) / value[i-1]
```

- **Sudden**: rate > 15% in a single frame (clouds, obstruction)
- **Gradual**: rate < 5% per frame over 3+ consecutive frames (drift, thermal)

### 6.3 Classification Data Structure

```rust
pub enum IssueCategory {
    LikelyClouds,
    PossibleObstruction,
    FocusDrift,
    TrackingError,
    WindShake,
    SkyBrightening,
    UnknownDegradation,
}

pub struct ImageQualityResult {
    pub image_id: i32,
    pub quality_score: f64,          // 0.0 to 1.0
    pub temporal_anomaly_score: f64, // 0.0 to 1.0
    pub category: Option<IssueCategory>,
    pub metrics: NormalizedMetrics,
    pub details: String,             // Human-readable explanation
}

pub struct NormalizedMetrics {
    pub star_count: Option<f64>,     // normalized 0-1
    pub hfr: Option<f64>,
    pub eccentricity: Option<f64>,
    pub snr: Option<f64>,
    pub background: Option<f64>,
}
```

---

## 7. API Design

### 7.1 Sequence Analysis Endpoint

```
POST /api/analysis/sequence
```

**Request Body:**

```json
{
  "target_id": 5,
  "filter_name": "Ha",
  "session_gap_minutes": 60,
  "weights": {
    "star_count": 0.30,
    "hfr": 0.25,
    "eccentricity": 0.10,
    "snr": 0.25,
    "background": 0.10
  }
}
```

All fields except `target_id` are optional. If `filter_name` is omitted,
analyze all filters for the target (returning separate sequences per filter).
If `weights` is omitted, use defaults.

**Response:**

```json
{
  "success": true,
  "data": {
    "sequences": [
      {
        "target_id": 5,
        "target_name": "M42",
        "filter_name": "Ha",
        "session_start": "2024-01-15T22:00:00Z",
        "session_end": "2024-01-16T03:30:00Z",
        "image_count": 45,
        "reference_values": {
          "best_star_count": 342,
          "best_hfr": 2.1,
          "best_eccentricity": 0.35,
          "best_snr": 48.2,
          "best_background": 1250.0
        },
        "images": [
          {
            "image_id": 1001,
            "quality_score": 0.95,
            "temporal_anomaly_score": 0.0,
            "category": null,
            "normalized_metrics": {
              "star_count": 0.98,
              "hfr": 0.92,
              "eccentricity": 0.90,
              "snr": 0.96,
              "background": 0.97
            },
            "details": null
          },
          {
            "image_id": 1015,
            "quality_score": 0.32,
            "temporal_anomaly_score": 0.45,
            "category": "likely_clouds",
            "normalized_metrics": {
              "star_count": 0.15,
              "hfr": 0.40,
              "eccentricity": 0.88,
              "snr": 0.30,
              "background": 0.25
            },
            "details": "Star count dropped 72% from baseline while background increased 35%. Pattern consistent with cloud passage."
          }
        ],
        "summary": {
          "excellent_count": 30,
          "good_count": 8,
          "fair_count": 3,
          "poor_count": 2,
          "bad_count": 2,
          "cloud_events_detected": 1,
          "focus_drift_detected": false,
          "tracking_issues_detected": false
        }
      }
    ]
  }
}
```

### 7.2 Batch Scoring Endpoint

For scoring all images in a project at once:

```
POST /api/analysis/project/{project_id}
```

**Request Body:**

```json
{
  "session_gap_minutes": 60,
  "weights": null,
  "auto_reject_threshold": 0.3,
  "dry_run": true
}
```

When `dry_run` is false and `auto_reject_threshold` is set, images scoring
below the threshold are automatically marked as rejected with the reason
set to the detected `category`.

**Response:**

```json
{
  "success": true,
  "data": {
    "sequences_analyzed": 12,
    "total_images": 450,
    "images_below_threshold": 23,
    "rejections_applied": 0,
    "dry_run": true,
    "breakdown_by_category": {
      "likely_clouds": 15,
      "focus_drift": 4,
      "tracking_error": 2,
      "unknown_degradation": 2
    }
  }
}
```

### 7.3 Single Image Score Endpoint

For getting the quality context of a specific image:

```
GET /api/analysis/image/{image_id}
```

Returns the image's score along with its sequence context (what sequence it
belongs to, reference values, surrounding frame scores).

### 7.4 Score Distribution Endpoint

For the UI to render histograms:

```
GET /api/analysis/distribution?target_id=5&filter_name=Ha
```

Returns score histogram buckets and metric time series for charting.

---

## 8. Threshold Recommendations

### 8.1 Auto-Reject Thresholds

| Threshold | Value | Use Case |
|-----------|-------|----------|
| Conservative | 0.20 | Only reject clearly ruined frames |
| Moderate | 0.35 | Good default for most users |
| Aggressive | 0.50 | Maximize quality, discard more |

### 8.2 Cloud Detection Thresholds

These apply to the temporal anomaly detection (Section 4):

| Parameter | Default | Range | Description |
|-----------|---------|-------|-------------|
| `star_drop_threshold` | 0.25 | 0.10 -- 0.50 | Minimum fractional star count drop to flag |
| `bg_rise_threshold` | 0.10 | 0.05 -- 0.30 | Minimum fractional background increase |
| `hfr_rise_threshold` | 0.15 | 0.10 -- 0.40 | Minimum fractional HFR increase |
| `ewma_alpha` | 0.30 | 0.10 -- 0.50 | Smoothing factor (higher = more responsive) |
| `sudden_change_rate` | 0.15 | 0.05 -- 0.30 | Rate threshold for sudden vs gradual |
| `session_gap_minutes` | 60 | 15 -- 180 | Gap to split sequences |
| `min_sequence_length` | 5 | 3 -- 10 | Minimum frames for analysis |

### 8.3 Per-Metric Rejection Thresholds

For users who want simple per-metric rejection (like the existing `grading.rs`
approach), express thresholds in terms of sigma from the sequence median:

| Metric | Default Sigma | Description |
|--------|--------------|-------------|
| HFR | 2.5 sigma | Reject if HFR > median + 2.5 * MAD |
| Star count | 2.5 sigma | Reject if stars < median - 2.5 * MAD |
| Eccentricity | 3.0 sigma | Reject if ecc > median + 3.0 * MAD |
| Background | 2.0 sigma | Reject if bg > median + 2.0 * MAD |

Use MAD (Median Absolute Deviation) * 1.4826 as the robust sigma estimator,
which is already computed in the codebase.

---

## 9. Implementation Plan

### Phase 1: Core Scoring (Backend)

1. Define the `SequenceAnalyzer` struct in a new `src/sequence_analysis.rs`.
2. Implement sequence grouping (target + filter + session gap).
3. Implement per-metric normalization with robust percentile bounds.
4. Implement EWMA temporal baseline and deviation scoring.
5. Implement composite quality score with configurable weights.
6. Implement issue classification rules.
7. Add the `POST /api/analysis/sequence` endpoint.

### Phase 2: Integration with Existing Grading

8. Wire `SequenceAnalyzer` into the existing `StatisticalGrader` as an
   optional enhancement (the current grader continues to work standalone).
9. Add the project-level batch scoring endpoint.
10. Add the single-image and distribution endpoints.

### Phase 3: Frontend (Web UI)

11. Sequence selector in the UI (pick target + filter to analyze).
12. Time-series chart showing metrics + score over the sequence.
13. Color-coded quality indicators on image thumbnails.
14. Batch reject/accept controls based on score threshold slider.

---

## 10. Relationship to Existing Code

### Current `grading.rs`

The existing `StatisticalGrader` uses:

- HFR z-score outlier detection (symmetric, flags both high and low)
- Star count z-score outlier detection
- MAD-based distribution analysis for skewed data
- Rolling median cloud detection on HFR and star count (sequential, either/or)

**Limitations addressed by this design:**

1. **Single-metric cloud detection**: Current code checks HFR first, then
   star count only if no HFR rejections found. The new system combines
   all metrics simultaneously.
2. **Fixed baseline window**: Current code requires `cloud_baseline_count`
   images before establishing a baseline. EWMA starts from frame 1.
3. **No scoring**: Current code is binary (reject/accept). The new system
   produces a continuous score enabling threshold-based decisions.
4. **No classification**: Current code labels everything as "Cloud Detection"
   or "Statistical HFR". The new system classifies the likely cause.
5. **No temporal analysis**: Current code treats HFR outliers the same
   whether they occur gradually or suddenly.

### Compatibility

The new `SequenceAnalyzer` does not replace `StatisticalGrader`. The
existing grader continues to provide its current functionality. The sequence
analyzer is a separate, more sophisticated analysis available via the API
that produces scores and classifications rather than binary rejections.

In a future version, the grader could optionally delegate to the sequence
analyzer, but for v1 they coexist independently.

---

## 11. Data Flow

```
Database (acquiredimage table)
    |
    v
Query images by target_id + filter
    |
    v
Sort by ExposureStartTime
    |
    v
Split into sequences (60-min gap)
    |
    v
For each sequence:
    |
    +-- Extract metrics from metadata JSON
    |     (HFR, DetectedStars, background)
    |
    +-- Optionally compute additional metrics from FITS
    |     (eccentricity, SNR, MAD -- if file available)
    |
    +-- Normalize each metric (robust percentile)
    |
    +-- Compute EWMA baseline + temporal deviation
    |
    +-- Compute composite quality score
    |
    +-- Classify issues
    |
    v
Return scored + classified sequence
```

---

## 12. References

- PixInsight SubframeSelector: weighted quality formula using FWHM,
  eccentricity, and SNR with per-target-type weights.
  Source: https://chaoticnebula.com/pixinsight-subframe-selector/

- PixInsight PSFSignalWeight: combined quality metric synthesizing
  FWHM, noise, eccentricity, and SNR into a single score.
  Source: https://stirlingastrophoto.com/posts/subframeselector-quality-metrics/

- Cloud detection via star photometry and extinction measurement
  in all-sky camera data.
  Source: https://ui.adsabs.harvard.edu/abs/2024PASP..136c5002Z/abstract

- Machine learning cloud identification achieving 95% accuracy with
  gradient-boosted trees on all-sky camera features.
  Source: https://arxiv.org/abs/2003.11109

- N.I.N.A. HFR and star count monitoring for real-time session quality.
  Source: https://nighttime-imaging.eu/docs/master/site/tabs/imaging/
