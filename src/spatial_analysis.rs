//! Spatial (grid-based) frame analysis for occlusion and stray-light screening.
//!
//! Global metrics (star count, HFR) miss partial occlusions: a tree branch or
//! stray-light gradient covering 20% of the frame barely moves the global star
//! count, while ruining that region. Dividing the frame into a coarse grid and
//! comparing per-cell star density and per-cell background exposes these
//! localized defects:
//!
//! - Occluders (trees, dome, dew shield) produce grid cells with near-zero
//!   star density while the rest of the frame stays normal.
//! - Stray light and lit foreground raise (or lower) the local background far
//!   beyond normal vignetting, producing a large spread of per-cell medians.
//!
//! Validated against a progressive tree/stray-light occlusion sequence
//! (NGC 6820 2026-06-30): clean frames show dead_cell_fraction <= 0.04 and
//! bg_cell_spread ~0.02-0.07; subtly occluded frames (global star count only
//! ~5% down) show dead_cell_fraction 0.08-0.21; heavily occluded frames
//! saturate both metrics.

use serde::{Deserialize, Serialize};

/// Configuration for grid-based spatial analysis.
#[derive(Debug, Clone)]
pub struct SpatialAnalysisConfig {
    /// Grid columns at full resolution (default 8).
    pub grid_cols: usize,
    /// Grid rows at full resolution (default 6).
    pub grid_rows: usize,
    /// A cell is "dead" when its star count falls below this fraction of the
    /// median cell star count (default 0.25).
    pub dead_cell_ratio: f64,
    /// Minimum median stars per cell for the star-grid metrics to be
    /// meaningful; below this the grid is coarsened, and if still too sparse
    /// the star metrics are reported as None (default 3.0).
    pub min_median_stars_per_cell: f64,
    /// Pixel subsampling stride used for per-cell background medians
    /// (default 4; medians are insensitive to this).
    pub background_subsample: usize,
}

impl Default for SpatialAnalysisConfig {
    fn default() -> Self {
        Self {
            grid_cols: 8,
            grid_rows: 6,
            dead_cell_ratio: 0.25,
            min_median_stars_per_cell: 3.0,
            background_subsample: 4,
        }
    }
}

/// Linear mapping from stored pixel units to physical ADU:
/// `adu = stored * adu_per_stored + adu_offset`.
///
/// `FitsImage` rescales each frame by its own min/max, so stored units are
/// not comparable across frames and, worse, ratios of stored values are
/// distorted by the per-frame minimum subtraction. Background metrics are
/// therefore computed in ADU. Use `PixelCalibration::default()` (identity)
/// when the data is already in physical units.
#[derive(Debug, Clone, Copy)]
pub struct PixelCalibration {
    pub adu_offset: f64,
    pub adu_per_stored: f64,
}

impl Default for PixelCalibration {
    fn default() -> Self {
        Self {
            adu_offset: 0.0,
            adu_per_stored: 1.0,
        }
    }
}

/// Grid-based spatial uniformity metrics for a single frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpatialMetrics {
    /// Grid dimensions actually used (may be coarser than configured when the
    /// frame has few stars).
    pub grid_cols: usize,
    pub grid_rows: usize,
    /// Fraction of grid cells whose star count is below `dead_cell_ratio` of
    /// the median cell count (0.0 = uniform coverage, 1.0 = no coverage).
    /// None when too few stars for a meaningful grid.
    pub star_dead_cell_fraction: Option<f64>,
    /// Median cell star count divided by the 95th-percentile cell count.
    /// ~0.6-0.8 on clean frames (Poisson + vignetting); collapses toward 0
    /// when stars survive in only part of the frame.
    pub star_uniformity: Option<f64>,
    /// (p95 - p5) of per-cell background medians, relative to the overall
    /// median. Clean frames: <= ~0.1 even with vignetting; occlusion or stray
    /// light: 0.15 and far beyond.
    pub bg_cell_spread: f64,
    /// Maximum absolute deviation of any cell median from the overall median,
    /// relative to the overall median.
    pub bg_cell_max_dev: f64,
}

impl SpatialMetrics {
    /// Star coverage fraction: 1.0 - dead cell fraction, when available.
    pub fn coverage(&self) -> Option<f64> {
        self.star_dead_cell_fraction.map(|d| 1.0 - d)
    }
}

/// Compute spatial metrics from detected star positions and raw pixel data.
///
/// `star_positions` are (x, y) centroids in pixel coordinates from any
/// detector. `data` is the frame in row-major order.
pub fn compute_spatial_metrics(
    star_positions: &[(f64, f64)],
    data: &[u16],
    width: usize,
    height: usize,
    calibration: &PixelCalibration,
    config: &SpatialAnalysisConfig,
) -> SpatialMetrics {
    let (grid_cols, grid_rows, star_grid) = star_grid_counts(star_positions, width, height, config);

    let (star_dead_cell_fraction, star_uniformity) = star_grid
        .as_ref()
        .map(|grid| star_grid_metrics(grid, config.dead_cell_ratio))
        .map_or((None, None), |(dead, unif)| (Some(dead), Some(unif)));

    // Background grid always uses the configured (full) resolution: it does
    // not depend on star statistics.
    let (bg_cell_spread, bg_cell_max_dev) = background_grid_metrics(
        data,
        width,
        height,
        calibration,
        config.grid_cols,
        config.grid_rows,
        config.background_subsample.max(1),
    );

    SpatialMetrics {
        grid_cols,
        grid_rows,
        star_dead_cell_fraction,
        star_uniformity,
        bg_cell_spread,
        bg_cell_max_dev,
    }
}

/// Bin star positions into a grid, coarsening once if the frame is too sparse
/// for the configured grid. Returns (cols, rows, Some(counts)) or
/// (cols, rows, None) when even the coarse grid is too sparse.
fn star_grid_counts(
    star_positions: &[(f64, f64)],
    width: usize,
    height: usize,
    config: &SpatialAnalysisConfig,
) -> (usize, usize, Option<Vec<f64>>) {
    if width == 0 || height == 0 {
        return (config.grid_cols, config.grid_rows, None);
    }

    // A frame with zero stars is fully dead, not "unknown": synthesize an
    // all-zero grid so the dead fraction reports 1.0.
    if star_positions.is_empty() {
        let cells = config.grid_cols * config.grid_rows;
        return (config.grid_cols, config.grid_rows, Some(vec![0.0; cells]));
    }

    for (cols, rows) in [
        (config.grid_cols, config.grid_rows),
        (config.grid_cols.div_ceil(2), config.grid_rows.div_ceil(2)),
    ] {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let mut counts = vec![0.0f64; cols * rows];
        for &(x, y) in star_positions {
            let gx = ((x / width as f64) * cols as f64) as usize;
            let gy = ((y / height as f64) * rows as f64) as usize;
            let gx = gx.min(cols - 1);
            let gy = gy.min(rows - 1);
            counts[gy * cols + gx] += 1.0;
        }
        if median(&counts) >= config.min_median_stars_per_cell {
            return (cols, rows, Some(counts));
        }
        // Distinguish "clustered" from "uniformly sparse". Stars packed into
        // a few well-populated cells while most cells are empty is occlusion
        // evidence and the grid is reported. A frame whose cells are ALL
        // near-empty (narrowband / short exposures on a slow rig) carries no
        // spatial information: without the max-cell requirement such frames
        // would report a large dead fraction on every frame and mass-reject
        // a perfectly healthy dataset. Abstain instead.
        let empty = counts.iter().filter(|&&c| c == 0.0).count();
        let max_cell = counts.iter().cloned().fold(0.0f64, f64::max);
        if empty * 2 > counts.len() && max_cell >= config.min_median_stars_per_cell * 3.0 {
            return (cols, rows, Some(counts));
        }
    }

    let cols = config.grid_cols.div_ceil(2).max(1);
    let rows = config.grid_rows.div_ceil(2).max(1);
    (cols, rows, None)
}

/// Compute (dead_cell_fraction, uniformity) from per-cell star counts.
fn star_grid_metrics(cell_counts: &[f64], dead_cell_ratio: f64) -> (f64, f64) {
    let med = median(cell_counts);
    let p95 = percentile(cell_counts, 0.95);

    let dead = if p95 <= 0.0 {
        // No cell has any stars: the whole frame is dead.
        1.0
    } else if med <= 0.0 {
        // Most of the frame is empty while some cells have stars: count the
        // empty cells directly (a threshold of ratio*median would be 0 and
        // report nothing dead).
        cell_counts.iter().filter(|&&c| c == 0.0).count() as f64 / cell_counts.len() as f64
    } else {
        let threshold = dead_cell_ratio * med;
        cell_counts.iter().filter(|&&c| c < threshold).count() as f64 / cell_counts.len() as f64
    };

    let uniformity = if p95 > 0.0 { (med / p95).min(1.0) } else { 0.0 };

    (dead, uniformity)
}

/// Compute (bg_cell_spread, bg_cell_max_dev) from per-cell background medians
/// in physical ADU.
fn background_grid_metrics(
    data: &[u16],
    width: usize,
    height: usize,
    calibration: &PixelCalibration,
    cols: usize,
    rows: usize,
    subsample: usize,
) -> (f64, f64) {
    if data.is_empty() || width == 0 || height == 0 || data.len() < width * height {
        return (0.0, 0.0);
    }
    let cols = cols.max(1);
    let rows = rows.max(1);

    let mut cell_medians = Vec::with_capacity(cols * rows);
    let mut samples: Vec<u16> = Vec::new();
    for gy in 0..rows {
        let y0 = gy * height / rows;
        let y1 = ((gy + 1) * height / rows).max(y0 + 1).min(height);
        for gx in 0..cols {
            let x0 = gx * width / cols;
            let x1 = ((gx + 1) * width / cols).max(x0 + 1).min(width);

            samples.clear();
            let mut y = y0;
            while y < y1 {
                let row = &data[y * width + x0..y * width + x1];
                let mut x = 0;
                while x < row.len() {
                    samples.push(row[x]);
                    x += subsample;
                }
                y += subsample;
            }
            if samples.is_empty() {
                continue;
            }
            let mid = samples.len() / 2;
            let (_, m, _) = samples.select_nth_unstable(mid);
            cell_medians.push(*m as f64 * calibration.adu_per_stored + calibration.adu_offset);
        }
    }

    if cell_medians.is_empty() {
        return (0.0, 0.0);
    }

    let overall = median(&cell_medians).max(1.0);
    let p5 = percentile(&cell_medians, 0.05);
    let p95 = percentile(&cell_medians, 0.95);
    let spread = (p95 - p5) / overall;
    let max_dev = cell_medians
        .iter()
        .map(|&m| (m - overall).abs())
        .fold(0.0f64, f64::max)
        / overall;

    (spread, max_dev)
}

fn median(values: &[f64]) -> f64 {
    percentile(values, 0.5)
}

/// Linear-interpolated percentile; `q` in [0, 1]. Returns 0.0 for empty input.
fn percentile(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<f64> = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pos = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        let frac = pos - lo as f64;
        sorted[lo] * (1.0 - frac) + sorted[hi] * frac
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 800;
    const H: usize = 600;

    fn flat_frame(level: u16) -> Vec<u16> {
        vec![level; W * H]
    }

    /// Evenly spread stars over the given fraction of the frame width.
    fn stars_covering(fraction: f64, per_cell: usize) -> Vec<(f64, f64)> {
        let mut stars = Vec::new();
        let max_x = W as f64 * fraction;
        let nx = (8.0 * fraction).round() as usize * per_cell;
        let ny = 6 * per_cell;
        for ix in 0..nx.max(1) {
            for iy in 0..ny {
                let x = (ix as f64 + 0.5) / nx.max(1) as f64 * max_x;
                let y = (iy as f64 + 0.5) / ny as f64 * H as f64;
                stars.push((x, y));
            }
        }
        stars
    }

    #[test]
    fn uniform_frame_is_clean() {
        let stars = stars_covering(1.0, 5);
        let data = flat_frame(500);
        let m = compute_spatial_metrics(
            &stars,
            &data,
            W,
            H,
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(m.star_dead_cell_fraction, Some(0.0));
        assert!(m.star_uniformity.unwrap() > 0.9);
        assert!(m.bg_cell_spread < 0.01);
        assert!(m.bg_cell_max_dev < 0.01);
    }

    #[test]
    fn half_occluded_frame_has_dead_cells() {
        // Stars only in the left half of the frame.
        let stars = stars_covering(0.5, 5);
        let data = flat_frame(500);
        let m = compute_spatial_metrics(
            &stars,
            &data,
            W,
            H,
            &Default::default(),
            &Default::default(),
        );
        let dead = m.star_dead_cell_fraction.unwrap();
        assert!(
            (dead - 0.5).abs() < 0.15,
            "expected ~half the cells dead, got {}",
            dead
        );
    }

    #[test]
    fn zero_stars_is_fully_dead() {
        let data = flat_frame(500);
        let m = compute_spatial_metrics(&[], &data, W, H, &Default::default(), &Default::default());
        assert_eq!(m.star_dead_cell_fraction, Some(1.0));
        assert_eq!(m.star_uniformity, Some(0.0));
    }

    #[test]
    fn uniformly_sparse_frame_abstains_with_default_config() {
        // Regression (code review): a healthy narrowband/short-exposure frame
        // with ~30 stars spread evenly leaves most grid cells empty. That is
        // sparseness, not occlusion - the metrics must abstain rather than
        // report a huge dead fraction that would mass-reject the dataset.
        let mut stars = Vec::new();
        for i in 0..20 {
            let x = ((i * 7919) % 100) as f64 / 100.0 * W as f64;
            let y = ((i * 104729) % 100) as f64 / 100.0 * H as f64;
            stars.push((x, y));
        }
        let data = flat_frame(500);
        let m = compute_spatial_metrics(
            &stars,
            &data,
            W,
            H,
            &Default::default(),
            &Default::default(),
        );
        assert_eq!(
            m.star_dead_cell_fraction, None,
            "uniformly sparse frame must abstain, got {:?}",
            m.star_dead_cell_fraction
        );
    }

    #[test]
    fn few_stars_clustered_in_corner_reports_dead_cells() {
        // 20 stars all in one corner cell: median cell count is 0, so the
        // sparse-frame path should report most cells dead rather than None.
        let stars: Vec<(f64, f64)> = (0..20).map(|i| (10.0 + i as f64, 12.0)).collect();
        let data = flat_frame(500);
        let m = compute_spatial_metrics(
            &stars,
            &data,
            W,
            H,
            &Default::default(),
            &Default::default(),
        );
        let dead = m
            .star_dead_cell_fraction
            .expect("clustered stars should still yield a grid");
        assert!(dead > 0.8, "expected most cells dead, got {}", dead);
    }

    #[test]
    fn sparse_but_uniform_frame_reports_none() {
        // 24 stars spread evenly: ~1 per coarse cell, legitimately sparse
        // (e.g. narrowband) - star grid metrics should abstain.
        let stars = stars_covering(1.0, 1);
        // Thin out to make the full grid sparse: keep every other star.
        let stars: Vec<_> = stars.into_iter().step_by(2).collect();
        let data = flat_frame(500);
        let config = SpatialAnalysisConfig {
            min_median_stars_per_cell: 30.0, // force both grids to be "sparse"
            ..Default::default()
        };
        let m = compute_spatial_metrics(&stars, &data, W, H, &Default::default(), &config);
        assert_eq!(m.star_dead_cell_fraction, None);
        assert_eq!(m.star_uniformity, None);
    }

    #[test]
    fn bright_gradient_raises_bg_spread() {
        // Left third of the frame lit by stray light at 3x the sky level.
        let mut data = flat_frame(500);
        for y in 0..H {
            for x in 0..W / 3 {
                data[y * W + x] = 1500;
            }
        }
        let stars = stars_covering(1.0, 5);
        let m = compute_spatial_metrics(
            &stars,
            &data,
            W,
            H,
            &Default::default(),
            &Default::default(),
        );
        assert!(
            m.bg_cell_spread > 0.5,
            "expected large bg spread, got {}",
            m.bg_cell_spread
        );
        assert!(m.bg_cell_max_dev > 0.5);
    }

    #[test]
    fn dark_occluder_raises_bg_spread() {
        // Bottom half blocked by a dark foreground (below sky level).
        let mut data = flat_frame(1000);
        for y in H / 2..H {
            for x in 0..W {
                data[y * W + x] = 300;
            }
        }
        let stars = stars_covering(1.0, 5);
        let m = compute_spatial_metrics(
            &stars,
            &data,
            W,
            H,
            &Default::default(),
            &Default::default(),
        );
        assert!(
            m.bg_cell_spread > 0.5,
            "expected large bg spread, got {}",
            m.bg_cell_spread
        );
    }

    #[test]
    fn empty_data_is_safe() {
        let m = compute_spatial_metrics(&[], &[], 0, 0, &Default::default(), &Default::default());
        assert_eq!(m.bg_cell_spread, 0.0);
        assert!(m.star_dead_cell_fraction.is_none());
    }
}
