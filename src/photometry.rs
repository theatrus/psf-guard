//! Cross-frame differential photometry and per-cell temporal analysis.
//!
//! The grid metrics in `spatial_analysis` catch occlusion that kills stars,
//! but small clouds and errant light dim or brighten parts of a frame without
//! necessarily removing detections. This module adds the classical remedies:
//!
//! - **Star matching + flux ratios**: stars are matched across frames of a
//!   sequence (nearest-neighbor after estimating the global dither offset)
//!   against a presence-filtered reference catalog. The median flux ratio is
//!   a per-frame **transparency** index (thin uniform cloud dims everything
//!   ~10-40% long before stars disappear), and per-cell median ratios give a
//!   localized **extinction map** (a small cloud is a coherent flux dip in a
//!   patch of stars). This is the technique all-sky cloud monitors use.
//! - **Per-cell temporal baselines**: each grid cell is compared against its
//!   own history across the sequence. A transient localized drop in a cell's
//!   share of stars, or a transient localized background rise that the
//!   frame's own gradient (plane fit) does not explain, flags small moving
//!   clouds and errant light (car headlights, flashlights) that global
//!   metrics dilute away.
//!
//! All fluxes must be in physical ADU (`stored_flux / FitsImage::raw_scale`):
//! stored pixel units are rescaled per frame and their ratios are meaningless
//! across frames.
//!
//! Caveat: the reference catalog requires stars to be present in at least
//! `reference_min_presence` of frames, so photometry is blind to regions
//! occluded for most of a sequence — that case is the dead-cell metric's job.
//! These metrics target *transient, localized* events.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One detected star: position in pixels, background-subtracted flux in ADU.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogStar {
    pub x: f64,
    pub y: f64,
    pub flux: f64,
}

/// All usable detections of one frame.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FrameCatalog {
    pub stars: Vec<CatalogStar>,
}

impl FrameCatalog {
    /// Keep only the brightest `n` stars (for persistence; matching quality
    /// saturates well below full catalog size).
    pub fn truncated(mut self, n: usize) -> Self {
        self.stars.sort_by(|a, b| {
            b.flux
                .partial_cmp(&a.flux)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self.stars.truncate(n);
        self
    }
}

#[derive(Debug, Clone)]
pub struct PhotometryConfig {
    /// Search radius (px) for the global offset estimation; must exceed the
    /// largest dither step (default 100).
    pub offset_search_radius: f64,
    /// Star match radius (px) after the offset is applied (default 5).
    pub match_radius: f64,
    /// Brightest-N stars used for offset estimation (default 200).
    pub offset_bright_count: usize,
    /// Minimum pairs for a trustworthy offset (default 10).
    pub min_offset_matches: usize,
    /// A reference star must be matched in at least this fraction of frames
    /// (default 0.5).
    pub reference_min_presence: f64,
    /// Minimum reference stars for photometry to be meaningful (default 20).
    pub min_reference_stars: usize,
    /// Minimum matched bright stars for a transparency estimate (default 10).
    pub min_transparency_matches: usize,
    /// Minimum matched stars in a grid cell for a cell ratio (default 3).
    pub min_stars_per_cell: usize,
    /// A cell is locally extinguished when its median flux ratio, after
    /// dividing out the frame's global transparency, falls below this
    /// (default 0.75, i.e. ~0.3 mag of localized extinction).
    pub local_extinction_ratio: f64,
    /// Minimum frames in a sequence for photometry / temporal baselines
    /// (default 5).
    pub min_frames: usize,
}

impl Default for PhotometryConfig {
    fn default() -> Self {
        Self {
            offset_search_radius: 100.0,
            match_radius: 5.0,
            offset_bright_count: 200,
            min_offset_matches: 10,
            reference_min_presence: 0.5,
            min_reference_stars: 20,
            min_transparency_matches: 10,
            min_stars_per_cell: 3,
            local_extinction_ratio: 0.75,
            min_frames: 5,
        }
    }
}

/// Per-frame photometric results, relative to the sequence reference.
#[derive(Debug, Clone, Default)]
pub struct FramePhotometry {
    /// Median flux ratio vs the reference over bright matched stars.
    /// 1.0 = nominal; 0.7 = the whole frame is ~0.4 mag dimmer (thin cloud).
    pub transparency: Option<f64>,
    /// Fraction of measurable grid cells whose median flux ratio (after
    /// dividing out global transparency) is below `local_extinction_ratio`:
    /// localized extinction from a small cloud.
    pub extinction_cell_fraction: Option<f64>,
    /// Fraction of bright reference stars with no match in this frame.
    pub missing_star_fraction: Option<f64>,
    /// Matched star count (diagnostics).
    pub matched_stars: usize,
    /// Per-cell flux ratio relative to the frame's global transparency
    /// (row-major; None = too few matched stars in the cell). For
    /// diagnostics/annotation.
    pub cell_relative_ratios: Vec<Option<f64>>,
}

// ---------------------------------------------------------------------------
// Spatial hashing + matching
// ---------------------------------------------------------------------------

struct GridHash {
    cell: f64,
    buckets: HashMap<(i64, i64), Vec<usize>>,
}

impl GridHash {
    fn build(stars: &[CatalogStar], cell: f64) -> Self {
        let mut buckets: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, s) in stars.iter().enumerate() {
            buckets
                .entry(((s.x / cell).floor() as i64, (s.y / cell).floor() as i64))
                .or_default()
                .push(i);
        }
        Self { cell, buckets }
    }

    /// Nearest star to (x, y) within `radius`.
    fn nearest(&self, stars: &[CatalogStar], x: f64, y: f64, radius: f64) -> Option<usize> {
        let span = (radius / self.cell).ceil() as i64;
        let cx = (x / self.cell).floor() as i64;
        let cy = (y / self.cell).floor() as i64;
        let mut best: Option<(usize, f64)> = None;
        for by in (cy - span)..=(cy + span) {
            for bx in (cx - span)..=(cx + span) {
                if let Some(idxs) = self.buckets.get(&(bx, by)) {
                    for &i in idxs {
                        let dx = stars[i].x - x;
                        let dy = stars[i].y - y;
                        let d2 = dx * dx + dy * dy;
                        if d2 <= radius * radius && best.is_none_or(|(_, b)| d2 < b) {
                            best = Some((i, d2));
                        }
                    }
                }
            }
        }
        best.map(|(i, _)| i)
    }
}

fn brightest_indices(stars: &[CatalogStar], n: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..stars.len()).collect();
    idx.sort_by(|&a, &b| {
        stars[b]
            .flux
            .partial_cmp(&stars[a].flux)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    idx.truncate(n);
    idx
}

/// Estimate the global translation from `from` to `to` (dither/drift):
/// `from.pos + offset ~= to.pos`. Returns None when too few pairs agree.
pub fn estimate_offset(
    from: &FrameCatalog,
    to: &FrameCatalog,
    config: &PhotometryConfig,
) -> Option<(f64, f64)> {
    let from_bright = brightest_indices(&from.stars, config.offset_bright_count);
    let to_bright: Vec<CatalogStar> = brightest_indices(&to.stars, config.offset_bright_count)
        .into_iter()
        .map(|i| to.stars[i].clone())
        .collect();
    if from_bright.is_empty() || to_bright.is_empty() {
        return None;
    }
    let hash = GridHash::build(&to_bright, config.offset_search_radius.max(1.0));

    let mut dxs = Vec::new();
    let mut dys = Vec::new();
    for &i in &from_bright {
        let s = &from.stars[i];
        if let Some(j) = hash.nearest(&to_bright, s.x, s.y, config.offset_search_radius) {
            dxs.push(to_bright[j].x - s.x);
            dys.push(to_bright[j].y - s.y);
        }
    }
    if dxs.len() < config.min_offset_matches {
        return None;
    }
    Some((median(&mut dxs), median(&mut dys)))
}

// ---------------------------------------------------------------------------
// Reference catalog + per-frame photometry
// ---------------------------------------------------------------------------

/// A presence-filtered reference: seed-frame positions with median fluxes.
#[derive(Debug, Clone)]
pub struct ReferenceCatalog {
    pub stars: Vec<CatalogStar>,
}

/// Build the sequence reference from the frame with the most stars, keeping
/// stars matched in at least `reference_min_presence` of frames with their
/// median flux across the sequence.
pub fn build_reference(
    catalogs: &[FrameCatalog],
    config: &PhotometryConfig,
) -> Option<ReferenceCatalog> {
    if catalogs.len() < config.min_frames {
        return None;
    }
    let seed_idx = (0..catalogs.len()).max_by_key(|&i| catalogs[i].stars.len())?;
    let seed = &catalogs[seed_idx];
    if seed.stars.is_empty() {
        return None;
    }

    let mut fluxes: Vec<Vec<f64>> = vec![Vec::new(); seed.stars.len()];
    for catalog in catalogs {
        let Some((dx, dy)) = estimate_offset(seed, catalog, config) else {
            continue;
        };
        let hash = GridHash::build(&catalog.stars, config.match_radius.max(1.0) * 4.0);
        for (i, s) in seed.stars.iter().enumerate() {
            if let Some(j) = hash.nearest(&catalog.stars, s.x + dx, s.y + dy, config.match_radius) {
                fluxes[i].push(catalog.stars[j].flux);
            }
        }
    }

    let min_count = ((catalogs.len() as f64) * config.reference_min_presence)
        .ceil()
        .max(1.0) as usize;
    let stars: Vec<CatalogStar> = seed
        .stars
        .iter()
        .zip(fluxes.iter_mut())
        .filter(|(_, f)| f.len() >= min_count)
        .map(|(s, f)| CatalogStar {
            x: s.x,
            y: s.y,
            flux: median(f),
        })
        .filter(|s| s.flux > 0.0)
        .collect();

    if stars.len() < config.min_reference_stars {
        return None;
    }
    Some(ReferenceCatalog { stars })
}

/// Run photometry for every frame of a sequence against its own reference.
/// `grid` is (cols, rows) of the extinction map; frames whose offset cannot
/// be estimated (e.g. fully clouded) get default (all-None) results.
pub fn sequence_photometry(
    catalogs: &[FrameCatalog],
    width: usize,
    height: usize,
    grid: (usize, usize),
    config: &PhotometryConfig,
) -> Vec<FramePhotometry> {
    let n = catalogs.len();
    let mut out = vec![FramePhotometry::default(); n];
    if width == 0 || height == 0 {
        return out;
    }
    let Some(reference) = build_reference(catalogs, config) else {
        return out;
    };
    let ref_catalog = FrameCatalog {
        stars: reference.stars.clone(),
    };

    // Bright half of the reference drives transparency and missing-star
    // counts (best measured, least detection-threshold flicker).
    let mut ref_fluxes: Vec<f64> = reference.stars.iter().map(|s| s.flux).collect();
    let bright_cut = median(&mut ref_fluxes);

    let (cols, rows) = (grid.0.max(1), grid.1.max(1));
    let cell_of = |s: &CatalogStar| -> usize {
        let gx = ((s.x / width as f64) * cols as f64) as usize;
        let gy = ((s.y / height as f64) * rows as f64) as usize;
        gy.min(rows - 1) * cols + gx.min(cols - 1)
    };

    for (frame_idx, catalog) in catalogs.iter().enumerate() {
        let Some((dx, dy)) = estimate_offset(&ref_catalog, catalog, config) else {
            continue;
        };
        let hash = GridHash::build(&catalog.stars, config.match_radius.max(1.0) * 4.0);

        let mut bright_ratios: Vec<f64> = Vec::new();
        let mut bright_total = 0usize;
        let mut bright_missing = 0usize;
        let mut cell_ratios: Vec<Vec<f64>> = vec![Vec::new(); cols * rows];
        let mut matched = 0usize;

        for ref_star in &reference.stars {
            let hit = hash.nearest(
                &catalog.stars,
                ref_star.x + dx,
                ref_star.y + dy,
                config.match_radius,
            );
            let is_bright = ref_star.flux >= bright_cut;
            if is_bright {
                bright_total += 1;
            }
            match hit {
                Some(j) => {
                    matched += 1;
                    let ratio = catalog.stars[j].flux / ref_star.flux;
                    if is_bright {
                        bright_ratios.push(ratio);
                    }
                    cell_ratios[cell_of(ref_star)].push(ratio);
                }
                None => {
                    if is_bright {
                        bright_missing += 1;
                    }
                }
            }
        }

        let result = &mut out[frame_idx];
        result.matched_stars = matched;
        if bright_total > 0 {
            result.missing_star_fraction = Some(bright_missing as f64 / bright_total as f64);
        }
        if bright_ratios.len() >= config.min_transparency_matches {
            let transparency = median(&mut bright_ratios);
            result.transparency = Some(transparency);

            if transparency > 0.0 {
                let mut measurable = 0usize;
                let mut extinguished = 0usize;
                for ratios in cell_ratios.iter_mut() {
                    if ratios.len() < config.min_stars_per_cell {
                        continue;
                    }
                    measurable += 1;
                    if median(ratios) / transparency < config.local_extinction_ratio {
                        extinguished += 1;
                    }
                }
                // Require reasonable coverage before claiming a fraction.
                if measurable * 4 >= cols * rows {
                    result.extinction_cell_fraction = Some(extinguished as f64 / measurable as f64);
                }
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Combined per-sequence screening signals (shared by CLI and server)
// ---------------------------------------------------------------------------

/// Per-frame inputs for the sequence-level pass, in time order.
#[derive(Debug, Clone, Default)]
pub struct FrameInputs {
    pub catalog: FrameCatalog,
    /// Star counts per cell at the configured grid (row-major); empty when
    /// unavailable.
    pub star_cell_counts: Vec<f64>,
    /// Background medians per cell in ADU (row-major); empty when
    /// unavailable.
    pub bg_cell_medians: Vec<f64>,
}

/// Per-frame outputs of the sequence-level pass.
#[derive(Debug, Clone, Default)]
pub struct FrameSignals {
    pub transparency: Option<f64>,
    pub extinction_cell_fraction: Option<f64>,
    pub missing_star_fraction: Option<f64>,
    /// Fraction of cells with a hard transient drop in their share of the
    /// frame's stars (small opaque cloud).
    pub star_cell_drop_fraction: Option<f64>,
    /// Fraction of cells with a hard transient background rise the frame's
    /// own gradient does not explain (errant light).
    pub bg_cell_rise_fraction: Option<f64>,
    /// Fraction of cells with a hard transient background FALL: a dark
    /// occluder or cloud shadow blocking skyglow reads darker, not milky.
    pub bg_cell_fall_fraction: Option<f64>,
    /// Per-cell diagnostics for annotation (empty when unavailable).
    pub cell_relative_ratios: Vec<Option<f64>>,
    pub star_drop_cells: Vec<bool>,
    pub bg_rise_cells: Vec<bool>,
    pub bg_fall_cells: Vec<bool>,
}

/// Run photometry and the per-cell temporal baselines over one sequence
/// (same filter + exposure, time-ordered). `sigma_star`/`sigma_bg` are the
/// per-cell anomaly thresholds (defaults 4.0 / 5.0).
pub fn sequence_screening_signals(
    frames: &[FrameInputs],
    width: usize,
    height: usize,
    grid: (usize, usize),
    config: &PhotometryConfig,
) -> Vec<FrameSignals> {
    let n = frames.len();
    let mut out: Vec<FrameSignals> = vec![FrameSignals::default(); n];
    if n == 0 {
        return out;
    }

    let catalogs: Vec<FrameCatalog> = frames.iter().map(|f| f.catalog.clone()).collect();
    let phot = sequence_photometry(&catalogs, width, height, grid, config);
    for (o, p) in out.iter_mut().zip(phot) {
        o.transparency = p.transparency;
        o.extinction_cell_fraction = p.extinction_cell_fraction;
        o.missing_star_fraction = p.missing_star_fraction;
        o.cell_relative_ratios = p.cell_relative_ratios;
    }

    let cells = grid.0 * grid.1;
    let star_grids: Vec<Vec<f64>> = frames
        .iter()
        .map(|f| {
            if f.star_cell_counts.len() == cells {
                f.star_cell_counts.clone()
            } else {
                vec![0.0; cells]
            }
        })
        .collect();
    for (o, f) in out.iter_mut().zip(star_share_drop_cells(&star_grids, 4.0)) {
        if let Some(cells) = f {
            o.star_cell_drop_fraction = Some(cells.fraction);
            o.star_drop_cells = cells.flagged;
        }
    }

    // Background shift pass only when every frame has a grid: a frame with a
    // synthesized all-zero grid would corrupt the temporal baselines.
    if frames.iter().all(|f| f.bg_cell_medians.len() == cells) {
        let bg_grids: Vec<Vec<f64>> = frames.iter().map(|f| f.bg_cell_medians.clone()).collect();
        for (o, f) in out
            .iter_mut()
            .zip(bg_shift_cells(&bg_grids, grid.0, grid.1, 5.0))
        {
            if let Some(shift) = f {
                o.bg_cell_rise_fraction = Some(shift.rise.fraction);
                o.bg_rise_cells = shift.rise.flagged;
                o.bg_cell_fall_fraction = Some(shift.fall.fraction);
                o.bg_fall_cells = shift.fall.flagged;
            }
        }
    }

    out
}

/// Split time-ordered frame indices into sessions on gaps larger than
/// `gap_seconds` (mirrors the sequence analyzer's session logic so the
/// photometric reference never spans nights). Frames without timestamps stay
/// in the current session.
pub fn split_sessions(timestamps: &[Option<i64>], gap_seconds: i64) -> Vec<Vec<usize>> {
    let mut sessions: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut last_ts: Option<i64> = None;
    for (i, ts) in timestamps.iter().enumerate() {
        if let (Some(t), Some(prev)) = (ts, last_ts)
            && t - prev > gap_seconds
            && !current.is_empty()
        {
            sessions.push(std::mem::take(&mut current));
        }
        current.push(i);
        if ts.is_some() {
            last_ts = *ts;
        }
    }
    if !current.is_empty() {
        sessions.push(current);
    }
    sessions
}

// ---------------------------------------------------------------------------
// Per-cell temporal baselines (small transients: clouds, errant light)
// ---------------------------------------------------------------------------

/// Per-frame per-cell anomaly outcome: the flagged fraction plus which
/// cells fired (for annotation).
#[derive(Debug, Clone)]
pub struct CellAnomalies {
    pub fraction: f64,
    pub flagged: Vec<bool>,
}

/// For per-frame star-count grids, compute each frame's fraction of cells
/// whose *share* of the frame's stars dropped hard below that cell's own
/// temporal median (Poisson-aware floors). Localized transient star loss —
/// the small-cloud signature — flags; global changes do not (shares
/// normalize them away). Returns None per frame when the frame or the
/// sequence carries too little information.
pub fn star_share_drop_fractions(grids: &[Vec<f64>], sigma: f64) -> Vec<Option<f64>> {
    star_share_drop_cells(grids, sigma)
        .into_iter()
        .map(|o| o.map(|c| c.fraction))
        .collect()
}

/// Per-cell variant of [`star_share_drop_fractions`].
pub fn star_share_drop_cells(grids: &[Vec<f64>], sigma: f64) -> Vec<Option<CellAnomalies>> {
    let n = grids.len();
    let mut out = vec![None; n];
    if n < 5 {
        return out;
    }
    let cells = grids[0].len();
    if cells == 0 || grids.iter().any(|g| g.len() != cells) {
        return out;
    }

    let totals: Vec<f64> = grids.iter().map(|g| g.iter().sum()).collect();
    const MIN_TOTAL: f64 = 50.0;
    let valid: Vec<usize> = (0..n).filter(|&f| totals[f] >= MIN_TOTAL).collect();
    if valid.len() < 5 {
        return out;
    }
    let mut valid_totals: Vec<f64> = valid.iter().map(|&f| totals[f]).collect();
    let typical_total = median(&mut valid_totals).max(1.0);

    // Per-cell temporal median + MAD of the share.
    let share = |f: usize, c: usize| grids[f][c] / totals[f];
    let mut cell_median = vec![0.0f64; cells];
    let mut cell_mad = vec![0.0f64; cells];
    for c in 0..cells {
        let mut vals: Vec<f64> = valid.iter().map(|&f| share(f, c)).collect();
        let m = median(&mut vals);
        let mut devs: Vec<f64> = vals.iter().map(|v| (v - m).abs()).collect();
        cell_median[c] = m;
        cell_mad[c] = median(&mut devs);
    }

    for &f in &valid {
        let mut flags = vec![false; cells];
        let mut flagged = 0usize;
        for (c, flag) in flags.iter_mut().enumerate() {
            let m = cell_median[c];
            if m <= 0.0 {
                continue;
            }
            let drop = m - share(f, c);
            // Threshold: robust MAD term or Poisson counting noise on the
            // cell's typical count, whichever is larger.
            let mad_term = sigma * 1.4826 * cell_mad[c];
            let poisson_term = sigma * (m / typical_total).sqrt();
            if drop > mad_term.max(poisson_term).max(0.25 * m) {
                *flag = true;
                flagged += 1;
            }
        }
        out[f] = Some(CellAnomalies {
            fraction: flagged as f64 / cells as f64,
            flagged: flags,
        });
    }
    out
}

/// For per-frame background grids (ADU), compute each frame's fraction of
/// cells whose plane-detrended residual rose hard above that cell's own
/// temporal median. Static gradients (vignetting, sky glow) live in the
/// plane and the temporal baseline; a transient localized rise — errant
/// light — flags.
pub fn bg_rise_fractions(
    grids: &[Vec<f64>],
    cols: usize,
    rows: usize,
    sigma: f64,
) -> Vec<Option<f64>> {
    bg_rise_cells(grids, cols, rows, sigma)
        .into_iter()
        .map(|o| o.map(|c| c.fraction))
        .collect()
}

/// Rise + fall per-cell background anomalies for one frame.
#[derive(Debug, Clone)]
pub struct BgShiftCells {
    /// Transient localized rise (errant light).
    pub rise: CellAnomalies,
    /// Transient localized fall: something blocking skyglow (dark occluder,
    /// cloud shadow) - the affected region reads *darker* than its own
    /// history, not milky.
    pub fall: CellAnomalies,
}

/// Per-cell variant of [`bg_rise_fractions`].
pub fn bg_rise_cells(
    grids: &[Vec<f64>],
    cols: usize,
    rows: usize,
    sigma: f64,
) -> Vec<Option<CellAnomalies>> {
    bg_shift_cells(grids, cols, rows, sigma)
        .into_iter()
        .map(|o| o.map(|s| s.rise))
        .collect()
}

/// Two-sided variant: rise (errant light) and fall (dark patch) per cell.
pub fn bg_shift_cells(
    grids: &[Vec<f64>],
    cols: usize,
    rows: usize,
    sigma: f64,
) -> Vec<Option<BgShiftCells>> {
    let n = grids.len();
    let mut out = vec![None; n];
    let cells = cols * rows;
    if n < 5 || cells == 0 || grids.iter().any(|g| g.len() != cells) {
        return out;
    }

    // Plane-detrended relative residuals per frame. The fit is robust
    // (outlier cells excluded and refit) so a strong localized patch cannot
    // tilt the plane and manufacture spurious shifts in unaffected cells.
    let mut resid: Vec<Vec<f64>> = Vec::with_capacity(n);
    for g in grids {
        let mut level: Vec<f64> = g.clone();
        let level = median(&mut level).max(1.0);
        let plane = fit_plane_robust(g, cols, rows);
        resid.push(
            g.iter()
                .enumerate()
                .map(|(i, &v)| (v - plane[i]) / level)
                .collect(),
        );
    }

    let mut cell_median = vec![0.0f64; cells];
    let mut cell_mad = vec![0.0f64; cells];
    for c in 0..cells {
        let mut vals: Vec<f64> = resid.iter().map(|r| r[c]).collect();
        let m = median(&mut vals);
        let mut devs: Vec<f64> = vals.iter().map(|v| (v - m).abs()).collect();
        cell_median[c] = m;
        cell_mad[c] = median(&mut devs);
    }

    const REL_FLOOR: f64 = 0.02; // 2% of sky: below this is noise, not light.
    for f in 0..n {
        let mut rise_flags = vec![false; cells];
        let mut fall_flags = vec![false; cells];
        let mut risen = 0usize;
        let mut fallen = 0usize;
        for c in 0..cells {
            let shift = resid[f][c] - cell_median[c];
            let threshold = (sigma * 1.4826 * cell_mad[c]).max(REL_FLOOR);
            if shift > threshold {
                rise_flags[c] = true;
                risen += 1;
            } else if -shift > threshold {
                fall_flags[c] = true;
                fallen += 1;
            }
        }
        out[f] = Some(BgShiftCells {
            rise: CellAnomalies {
                fraction: risen as f64 / cells as f64,
                flagged: rise_flags,
            },
            fall: CellAnomalies {
                fraction: fallen as f64 / cells as f64,
                flagged: fall_flags,
            },
        });
    }
    out
}

/// Robust plane fit: least squares, then refit excluding cells whose
/// residual is a hard outlier, so a localized bright/dark patch cannot tilt
/// the plane for the rest of the frame.
pub(crate) fn fit_plane_robust(values: &[f64], cols: usize, rows: usize) -> Vec<f64> {
    let first = fit_plane_masked(values, cols, rows, None);
    let mut resid: Vec<f64> = values.iter().zip(&first).map(|(v, p)| v - p).collect();
    let mut abs: Vec<f64> = resid.iter().map(|r| r.abs()).collect();
    let mad = median(&mut abs);
    let threshold = (3.0 * 1.4826 * mad).max(1e-9);
    let mask: Vec<bool> = resid.iter().map(|r| r.abs() <= threshold).collect();
    let kept = mask.iter().filter(|&&m| m).count();
    if kept < 6 || kept == mask.len() {
        return first;
    }
    // Recompute residual basis for the final answer.
    resid.clear();
    fit_plane_masked(values, cols, rows, Some(&mask))
}

/// Least-squares plane a + b*gx + c*gy over grid cells, optionally
/// restricted to masked-in cells (the returned plane still covers all).
fn fit_plane_masked(values: &[f64], cols: usize, rows: usize, mask: Option<&[bool]>) -> Vec<f64> {
    let mut n = 0.0f64;
    let mut sx = 0.0;
    let mut sy = 0.0;
    let mut sxx = 0.0;
    let mut syy = 0.0;
    let mut sxy = 0.0;
    let mut sv = 0.0;
    let mut sxv = 0.0;
    let mut syv = 0.0;
    for gy in 0..rows {
        for gx in 0..cols {
            let c = gy * cols + gx;
            if let Some(m) = mask
                && !m[c]
            {
                continue;
            }
            let v = values[c];
            let (x, y) = (gx as f64, gy as f64);
            n += 1.0;
            sx += x;
            sy += y;
            sxx += x * x;
            syy += y * y;
            sxy += x * y;
            sv += v;
            sxv += x * v;
            syv += y * v;
        }
    }
    // Solve the 3x3 normal equations via Cramer's rule.
    let det = n * (sxx * syy - sxy * sxy) - sx * (sx * syy - sxy * sy) + sy * (sx * sxy - sxx * sy);
    let (a, b, c) = if det.abs() < 1e-9 {
        (sv / n.max(1.0), 0.0, 0.0)
    } else {
        let a = (sv * (sxx * syy - sxy * sxy) - sx * (sxv * syy - sxy * syv)
            + sy * (sxv * sxy - sxx * syv))
            / det;
        let b = (n * (sxv * syy - sxy * syv) - sv * (sx * syy - sxy * sy)
            + sy * (sx * syv - sxv * sy))
            / det;
        let c = (n * (sxx * syv - sxv * sxy) - sx * (sx * syv - sxv * sy)
            + sv * (sx * sxy - sxx * sy))
            / det;
        (a, b, c)
    };
    (0..rows)
        .flat_map(|gy| (0..cols).map(move |gx| a + b * gx as f64 + c * gy as f64))
        .collect()
}

fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 2000;
    const H: usize = 1500;
    const GRID: (usize, usize) = (8, 6);

    /// A deterministic field of stars with varied fluxes.
    fn base_field(count: usize) -> Vec<CatalogStar> {
        (0..count)
            .map(|i| {
                let x = ((i * 7919) % 1000) as f64 / 1000.0 * (W as f64 - 40.0) + 20.0;
                let y = ((i * 104729) % 1000) as f64 / 1000.0 * (H as f64 - 40.0) + 20.0;
                let flux = 500.0 + ((i * 613) % 5000) as f64;
                CatalogStar { x, y, flux }
            })
            .collect()
    }

    /// Frame = field shifted by dither, fluxes scaled by `transparency`,
    /// with `mutate` applied per star (return None to drop the star).
    fn make_frame(
        field: &[CatalogStar],
        dither: (f64, f64),
        transparency: f64,
        mutate: impl Fn(&CatalogStar) -> Option<f64>,
    ) -> FrameCatalog {
        FrameCatalog {
            stars: field
                .iter()
                .filter_map(|s| {
                    mutate(s).map(|extra| CatalogStar {
                        x: s.x + dither.0,
                        y: s.y + dither.1,
                        flux: s.flux * transparency * extra,
                    })
                })
                .collect(),
        }
    }

    #[test]
    fn offset_estimation_recovers_dither() {
        let field = base_field(400);
        let a = make_frame(&field, (0.0, 0.0), 1.0, |_| Some(1.0));
        let b = make_frame(&field, (17.3, -22.6), 1.0, |_| Some(1.0));
        let (dx, dy) = estimate_offset(&a, &b, &PhotometryConfig::default()).unwrap();
        assert!((dx - 17.3).abs() < 0.5, "dx {}", dx);
        assert!((dy + 22.6).abs() < 0.5, "dy {}", dy);
    }

    #[test]
    fn clean_sequence_reads_nominal() {
        let field = base_field(500);
        let dithers = [
            (0.0, 0.0),
            (12.0, 5.0),
            (-8.0, 14.0),
            (3.0, -11.0),
            (20.0, 2.0),
            (-15.0, -9.0),
        ];
        let catalogs: Vec<FrameCatalog> = dithers
            .iter()
            .map(|&d| make_frame(&field, d, 1.0, |_| Some(1.0)))
            .collect();

        let phot = sequence_photometry(&catalogs, W, H, GRID, &PhotometryConfig::default());
        for (i, p) in phot.iter().enumerate() {
            let t = p.transparency.expect("transparency");
            assert!((t - 1.0).abs() < 0.02, "frame {} transparency {}", i, t);
            assert_eq!(p.extinction_cell_fraction, Some(0.0), "frame {}", i);
            assert!(p.missing_star_fraction.unwrap() < 0.02, "frame {}", i);
        }
    }

    #[test]
    fn thin_cloud_dims_transparency() {
        let field = base_field(500);
        let mut catalogs: Vec<FrameCatalog> = (0..8)
            .map(|i| make_frame(&field, (i as f64 * 3.0, 0.0), 1.0, |_| Some(1.0)))
            .collect();
        // Frame 5: uniform 30% dimming - detection keeps every star, star
        // counts and HFR unchanged, only flux ratios notice.
        catalogs[5] = make_frame(&field, (15.0, 0.0), 0.7, |_| Some(1.0));

        let phot = sequence_photometry(&catalogs, W, H, GRID, &PhotometryConfig::default());
        let t5 = phot[5].transparency.unwrap();
        assert!((t5 - 0.7).abs() < 0.03, "veiled frame transparency {}", t5);
        assert_eq!(
            phot[5].extinction_cell_fraction,
            Some(0.0),
            "uniform veil is not localized extinction"
        );
        assert!(phot[4].transparency.unwrap() > 0.95);
    }

    #[test]
    fn small_cloud_shows_localized_extinction() {
        let field = base_field(600);
        let mut catalogs: Vec<FrameCatalog> = (0..8)
            .map(|i| make_frame(&field, (i as f64 * 2.0, i as f64), 1.0, |_| Some(1.0)))
            .collect();
        // Frame 4: a small cloud dims stars in the upper-left ~2x2 cells by
        // 60% without removing them.
        let patch_w = W as f64 / 4.0;
        let patch_h = H as f64 / 3.0;
        catalogs[4] = make_frame(&field, (8.0, 4.0), 1.0, |s| {
            Some(if s.x < patch_w && s.y < patch_h {
                0.4
            } else {
                1.0
            })
        });

        let phot = sequence_photometry(&catalogs, W, H, GRID, &PhotometryConfig::default());
        let ext = phot[4].extinction_cell_fraction.unwrap();
        assert!(
            ext > 0.04,
            "small cloud should extinguish some cells, got {}",
            ext
        );
        // Global transparency barely moves (patch is ~8% of the field).
        assert!(phot[4].transparency.unwrap() > 0.9);
        // Neighboring clean frame unaffected.
        assert_eq!(phot[3].extinction_cell_fraction, Some(0.0));
    }

    #[test]
    fn opaque_cloud_reads_as_missing_stars() {
        let field = base_field(500);
        let mut catalogs: Vec<FrameCatalog> = (0..8)
            .map(|i| make_frame(&field, (i as f64, 0.0), 1.0, |_| Some(1.0)))
            .collect();
        // Frame 6: an opaque blob removes stars in a corner patch entirely.
        let patch_w = W as f64 / 4.0;
        let patch_h = H as f64 / 3.0;
        catalogs[6] = make_frame(&field, (6.0, 0.0), 1.0, |s| {
            if s.x < patch_w && s.y < patch_h {
                None
            } else {
                Some(1.0)
            }
        });

        let phot = sequence_photometry(&catalogs, W, H, GRID, &PhotometryConfig::default());
        let missing = phot[6].missing_star_fraction.unwrap();
        assert!(
            missing > 0.04,
            "opaque patch should read missing, got {}",
            missing
        );
        assert!(phot[5].missing_star_fraction.unwrap() < 0.02);
    }

    #[test]
    fn short_sequences_abstain() {
        let field = base_field(300);
        let catalogs: Vec<FrameCatalog> = (0..3)
            .map(|i| make_frame(&field, (i as f64, 0.0), 1.0, |_| Some(1.0)))
            .collect();
        let phot = sequence_photometry(&catalogs, W, H, GRID, &PhotometryConfig::default());
        assert!(phot.iter().all(|p| p.transparency.is_none()));
    }

    // --- per-cell temporal baselines ---

    fn uniform_grid(cells: usize, per_cell: f64) -> Vec<f64> {
        vec![per_cell; cells]
    }

    #[test]
    fn star_share_drop_flags_transient_cell_loss() {
        let cells = 48;
        let mut grids: Vec<Vec<f64>> = (0..10).map(|_| uniform_grid(cells, 100.0)).collect();
        // Frame 6: three cells lose 80% of their stars (small cloud), rest
        // unchanged.
        for cell in grids[6].iter_mut().take(3) {
            *cell = 20.0;
        }
        let fractions = star_share_drop_fractions(&grids, 4.0);
        let f6 = fractions[6].unwrap();
        assert!(
            (f6 - 3.0 / 48.0).abs() < 0.05,
            "3 of 48 cells should flag, got {}",
            f6
        );
        assert_eq!(fractions[5], Some(0.0));
    }

    #[test]
    fn star_share_ignores_global_changes() {
        let cells = 48;
        let mut grids: Vec<Vec<f64>> = (0..10).map(|_| uniform_grid(cells, 100.0)).collect();
        // Frame 4: everything halves (global transparency change) - shares
        // are unchanged, nothing local to flag.
        grids[4] = uniform_grid(cells, 50.0);
        let fractions = star_share_drop_fractions(&grids, 4.0);
        assert_eq!(fractions[4], Some(0.0));
    }

    #[test]
    fn bg_rise_ignores_static_gradient_but_flags_transient_bump() {
        let (cols, rows) = (8usize, 6usize);
        let cells = cols * rows;
        // Every frame has the same strong left-right gradient (vignetting /
        // sky glow): 1000 + 50*gx ADU.
        let gradient: Vec<f64> = (0..cells)
            .map(|i| 1000.0 + 50.0 * (i % cols) as f64)
            .collect();
        let mut grids: Vec<Vec<f64>> = (0..10).map(|_| gradient.clone()).collect();
        // Frame 7: errant light bumps two corner cells by 15% of sky.
        grids[7][0] += 150.0;
        grids[7][1] += 150.0;

        let fractions = bg_rise_fractions(&grids, cols, rows, 5.0);
        assert_eq!(fractions[3], Some(0.0), "static gradient must not flag");
        let f7 = fractions[7].unwrap();
        assert!(
            f7 > 0.02 && f7 < 0.2,
            "two bumped cells should flag, got {}",
            f7
        );
    }

    #[test]
    fn bg_fall_flags_transient_dark_patch() {
        let (cols, rows) = (8usize, 6usize);
        let cells = cols * rows;
        let base: Vec<f64> = vec![1500.0; cells];
        let mut grids: Vec<Vec<f64>> = (0..10).map(|_| base.clone()).collect();
        // Frame 5: a dark occluder blocks skyglow in three cells (-20%).
        for cell in grids[5].iter_mut().take(3) {
            *cell = 1200.0;
        }
        let shifts = bg_shift_cells(&grids, cols, rows, 5.0);
        let f5 = shifts[5].as_ref().unwrap();
        assert!(
            f5.fall.fraction > 0.02 && f5.fall.fraction < 0.2,
            "dark patch should flag as fall, got {}",
            f5.fall.fraction
        );
        assert_eq!(f5.rise.fraction, 0.0, "no rise on a darkening frame corner");
        assert_eq!(shifts[4].as_ref().unwrap().fall.fraction, 0.0);
    }

    #[test]
    fn sequence_signals_populate_per_cell_outputs_end_to_end() {
        // Regression: the combined pass must populate the per-cell temporal
        // outputs (drop/rise/fall flags AND fractions), not just photometry.
        let field = base_field(500);
        let cells = GRID.0 * GRID.1;
        let mut frames: Vec<FrameInputs> = (0..8)
            .map(|i| FrameInputs {
                catalog: make_frame(&field, (i as f64, 0.0), 1.0, |_| Some(1.0)),
                star_cell_counts: vec![100.0; cells],
                bg_cell_medians: vec![1500.0; cells],
            })
            .collect();
        // Frame 5: dark occluder - three cells lose bg AND stars.
        for c in 0..3 {
            frames[5].bg_cell_medians[c] = 1100.0;
            frames[5].star_cell_counts[c] = 10.0;
        }

        let signals = sequence_screening_signals(&frames, W, H, GRID, &PhotometryConfig::default());
        let s5 = &signals[5];
        assert!(
            s5.bg_cell_fall_fraction.unwrap() > 0.02,
            "fall fraction should populate: {:?}",
            s5.bg_cell_fall_fraction
        );
        assert!(
            s5.bg_fall_cells.iter().take(3).all(|&f| f),
            "fall flags set"
        );
        assert!(
            s5.star_cell_drop_fraction.unwrap() > 0.02,
            "star drop fraction should populate"
        );
        assert!(s5.star_drop_cells.iter().take(3).all(|&f| f));
        assert_eq!(signals[4].bg_cell_fall_fraction, Some(0.0));
        assert!(s5.transparency.is_some());
    }

    #[test]
    fn temporal_baselines_abstain_on_short_sequences() {
        let grids: Vec<Vec<f64>> = (0..3).map(|_| uniform_grid(48, 100.0)).collect();
        assert!(star_share_drop_fractions(&grids, 4.0)
            .iter()
            .all(|f| f.is_none()));
        assert!(bg_rise_fractions(&grids, 8, 6, 5.0)
            .iter()
            .all(|f| f.is_none()));
    }
}
