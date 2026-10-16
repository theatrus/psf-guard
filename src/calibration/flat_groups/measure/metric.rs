//! Whether two flat sessions describe the same light path.
//!
//! A session is reduced to one **response**: its flats binned to at most
//! [`MAX_SIDE`] pixels, each divided by its own median, then combined by a
//! per-pixel median and a 3σ-clipped mean. Two responses are compared by
//! their ratio `Q = later / earlier`. A light shot in the later state and
//! divided by the earlier flat carries `Q − 1` of its sky background, so
//! every number here is a fraction of the background.
//!
//! - **Broad**: the largest departure of a smooth surface fitted to `Q`
//!   from its best plane. A straight gradient is what twilight flats differ
//!   by and what background extraction removes.
//! - **Dust**: the deepest compact spot left after the smooth part and a
//!   wide local median are taken out, counted only where one of the two
//!   responses holds a dip of its own. Dust and dew take light away; a star
//!   adds it.
//!
//! Stars come first. A session median keeps the stars of a sky-flat set
//! shot while the mount tracks, so compact bright sources are masked in
//! both responses before anything is measured ([`star_mask`]).
//!
//! This belongs in `seiza-calibration` beside `measure_flat`; it lives here
//! until Seiza grows it.

use rayon::prelude::*;
use seiza_imgproc::blur::{auto_ksize_f32, gaussian_blur_f32};
use seiza_imgproc::components::{connected_components, Connectivity};
use seiza_imgproc::BorderMode;
use serde::{Deserialize, Serialize};

/// Bump when equal frames may build a different response.
pub const RESPONSE_ALGORITHM_VERSION: u32 = 1;
/// Bump when equal responses may compare differently.
pub const COMPARE_ALGORITHM_VERSION: u32 = 1;
/// The longest side of a response, in response pixels.
pub const MAX_SIDE: usize = 1024;
/// The most flats one response combines, evenly spaced.
pub const MAX_FRAMES: usize = 16;
/// The fewest flats a response needs: below three a median cannot outvote
/// one odd frame.
pub const MIN_FRAMES: usize = 3;

/// A pixel is a star when it stands this many robust sigmas above the
/// median of its neighbourhood.
const STAR_SIGMA: f32 = 5.0;
/// Side of that neighbourhood, in response pixels.
const STAR_WINDOW: usize = 9;
/// How far a star's mask grows, to take in its wings and the halo the
/// smoothing leaves around it.
const STAR_GROW: usize = 3;
/// Block medians across the long side of the surface grid.
const SURFACE_BLOCKS: usize = 32;
/// Degree of the surface polynomial.
const SURFACE_DEGREE: usize = 4;
/// Rounds of clipping blocks that hold dust before the final fit.
const SURFACE_ROUNDS: usize = 3;
/// The local median that follows a sky gradient bending into a corner spans
/// this fraction of the long side: several times wider than any dust shadow.
const LOCAL_FRACTION: usize = 6;
/// A spot is this many robust sigmas of the smoothed residual.
const SPOT_SIGMA: f32 = 5.0;
/// A spot covers at least this many sensor pixels; smaller is a star, a hot
/// cluster or noise.
const MIN_SPOT_SENSOR_PIXELS: f64 = 200.0;
/// Edge band left out of both metrics, as a fraction of each side.
const BORDER_FRACTION: f64 = 0.02;
/// Most spots one comparison keeps, deepest first.
const MAX_SPOTS: usize = 40;

/// A single-channel image in row-major order.
#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Plane {
    pub fn filled(width: usize, height: usize, value: f32) -> Self {
        Self {
            width,
            height,
            data: vec![value; width * height],
        }
    }

    fn same_size(&self, other: &Plane) -> bool {
        self.width == other.width && self.height == other.height
    }
}

/// One flat session reduced to a response.
#[derive(Clone, Debug, PartialEq)]
pub struct FlatResponse {
    /// Each flat divided by its own median, combined; the pedestal is still
    /// in it (see [`FlatResponse::without_pedestal`]).
    pub plane: Plane,
    /// Sensor pixels per response pixel along each side.
    pub bin: usize,
    /// The flats' raw median level in ADU, pedestal included.
    pub level: f32,
    /// Noise of one response pixel, as a fraction of the response.
    pub noise: f32,
    pub frames: usize,
}

impl FlatResponse {
    /// The response with a pedestal taken out: `(R·m − p) / (m − p)`. A
    /// pedestal shrinks every difference by about `p / m`, and two sessions
    /// at different levels would otherwise differ by a vignette's print.
    pub fn without_pedestal(&self, pedestal: f32) -> Plane {
        let m = self.level;
        if !(pedestal > 0.0 && m > pedestal) {
            return self.plane.clone();
        }
        let scale = m / (m - pedestal);
        let shift = pedestal / (m - pedestal);
        Plane {
            width: self.plane.width,
            height: self.plane.height,
            data: self
                .plane
                .data
                .iter()
                .map(|value| value * scale - shift)
                .collect(),
        }
    }
}

/// The block factor that brings a frame's long side within [`MAX_SIDE`]. A
/// colour frame's blocks cover whole 2×2 cells, so each block sums every
/// site and the filter pattern cancels.
pub fn bin_factor(width: usize, height: usize, cfa: bool) -> usize {
    let factor = width.max(height).div_ceil(MAX_SIDE).max(1);
    if cfa && factor % 2 == 1 {
        factor + 1
    } else {
        factor
    }
}

/// Block means of a frame, its channels averaged. Partial blocks at the
/// right and bottom edges are left out, so a response pixel always covers
/// the same sensor area.
pub fn bin_frame(
    data: &[f32],
    width: usize,
    height: usize,
    channels: usize,
    factor: usize,
) -> Plane {
    let channels = channels.max(1);
    let factor = factor.max(1);
    let (out_width, out_height) = (width / factor, height / factor);
    let rows: Vec<Vec<f32>> = (0..out_height)
        .into_par_iter()
        .map(|out_y| {
            let mut sums = vec![0f64; out_width];
            let mut counts = vec![0u32; out_width];
            for y in out_y * factor..(out_y + 1) * factor {
                let row = &data[y * width * channels..(y + 1) * width * channels];
                for x in 0..out_width * factor {
                    for channel in 0..channels {
                        let value = row[x * channels + channel];
                        if value.is_finite() {
                            sums[x / factor] += f64::from(value);
                            counts[x / factor] += 1;
                        }
                    }
                }
            }
            sums.iter()
                .zip(&counts)
                .map(|(sum, count)| {
                    if *count > 0 {
                        (*sum / f64::from(*count)) as f32
                    } else {
                        f32::NAN
                    }
                })
                .collect()
        })
        .collect();
    Plane {
        width: out_width,
        height: out_height,
        data: rows.into_iter().flatten().collect(),
    }
}

/// The median of up to a million finite samples, evenly spaced; NaN when
/// there are none.
pub fn sampled_median(data: &[f32]) -> f32 {
    let step = (data.len() / (1 << 20)).max(1);
    let mut values: Vec<f32> = data
        .iter()
        .step_by(step)
        .copied()
        .filter(|value| value.is_finite())
        .collect();
    median_in_place(&mut values)
}

fn median_in_place(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return f32::NAN;
    }
    let middle = values.len() / 2;
    let (_, value, _) = values.select_nth_unstable_by(middle, f32::total_cmp);
    *value
}

/// The normal-equivalent median absolute deviation of finite samples.
fn robust_sigma(values: &[f32]) -> f32 {
    let mut finite: Vec<f32> = values.iter().copied().filter(|v| v.is_finite()).collect();
    let middle = median_in_place(&mut finite);
    let mut deviations: Vec<f32> = finite.iter().map(|v| (v - middle).abs()).collect();
    1.4826 * median_in_place(&mut deviations)
}

/// Combine binned flats of one session, raw ADU with their pedestal, into a
/// response. `None` with fewer than [`MIN_FRAMES`] or planes of different
/// sizes.
pub fn build_response(binned: &[Plane], bin: usize) -> Option<FlatResponse> {
    let first = binned.first()?;
    if binned.len() < MIN_FRAMES || binned.iter().any(|plane| !plane.same_size(first)) {
        return None;
    }
    let levels: Vec<f32> = binned
        .iter()
        .map(|plane| sampled_median(&plane.data))
        .collect();
    if levels
        .iter()
        .any(|level| !(level.is_finite() && *level > 0.0))
    {
        return None;
    }
    let count = binned.len();
    let pixels = first.width * first.height;
    let combined: Vec<(f32, f32)> = (0..pixels)
        .into_par_iter()
        .map_init(
            || Vec::with_capacity(count),
            |samples, index| {
                samples.clear();
                samples.extend(
                    binned
                        .iter()
                        .zip(&levels)
                        .map(|(plane, level)| plane.data[index] / level)
                        .filter(|value| value.is_finite()),
                );
                if samples.is_empty() {
                    return (f32::NAN, f32::NAN);
                }
                let middle = median_in_place(samples);
                let mut deviations: Vec<f32> =
                    samples.iter().map(|value| (value - middle).abs()).collect();
                let mad = 1.4826 * median_in_place(&mut deviations);
                // Drop what sits more than 3σ off the median: a star that
                // moved, a satellite, a cosmic ray.
                let limit = 3.0 * mad.max(1e-6);
                let (sum, kept) = samples
                    .iter()
                    .filter(|value| (*value - middle).abs() <= limit)
                    .fold((0f64, 0u32), |(sum, kept), value| {
                        (sum + f64::from(*value), kept + 1)
                    });
                let value = if kept > 0 {
                    (sum / f64::from(kept)) as f32
                } else {
                    middle
                };
                (value, mad)
            },
        )
        .collect();
    let mads: Vec<f32> = combined.iter().map(|(_, mad)| *mad).collect();
    let mut mads = mads;
    // The median of n samples is noisier than their mean by √(π/2).
    let noise = median_in_place(&mut mads) * 1.2533 / (count as f32).sqrt();
    let mut levels = levels;
    Some(FlatResponse {
        plane: Plane {
            width: first.width,
            height: first.height,
            data: combined.into_iter().map(|(value, _)| value).collect(),
        },
        bin,
        level: median_in_place(&mut levels),
        noise,
        frames: count,
    })
}

/// A square max filter of radius `radius` on a mask.
pub(super) fn grow(mask: &[bool], width: usize, height: usize, radius: usize) -> Vec<bool> {
    if radius == 0 {
        return mask.to_vec();
    }
    let mut rows = vec![false; mask.len()];
    for y in 0..height {
        for x in 0..width {
            let lo = x.saturating_sub(radius);
            let hi = (x + radius).min(width - 1);
            rows[y * width + x] = (lo..=hi).any(|i| mask[y * width + i]);
        }
    }
    let mut out = vec![false; mask.len()];
    for y in 0..height {
        let lo = y.saturating_sub(radius);
        let hi = (y + radius).min(height - 1);
        for x in 0..width {
            out[y * width + x] = (lo..=hi).any(|j| rows[j * width + x]);
        }
    }
    out
}

/// The median of each pixel's `size × size` neighbourhood, NaNs left out.
fn median_filter(plane: &Plane, size: usize) -> Plane {
    quantile_filter(plane, size, 0.5)
}

/// The `quantile` of each pixel's `size × size` neighbourhood, NaNs left
/// out.
fn quantile_filter(plane: &Plane, size: usize, quantile: f32) -> Plane {
    let half = size / 2;
    let (width, height) = (plane.width, plane.height);
    let data: Vec<f32> = (0..width * height)
        .into_par_iter()
        .map_init(
            || Vec::with_capacity(size * size),
            |values, index| {
                let (x, y) = (index % width, index / width);
                values.clear();
                for j in y.saturating_sub(half)..=(y + half).min(height - 1) {
                    let row = &plane.data[j * width..(j + 1) * width];
                    values.extend(
                        row[x.saturating_sub(half)..=(x + half).min(width - 1)]
                            .iter()
                            .copied()
                            .filter(|value| value.is_finite()),
                    );
                }
                if values.is_empty() {
                    return f32::NAN;
                }
                let at = ((values.len() - 1) as f32 * quantile).round() as usize;
                let (_, value, _) = values.select_nth_unstable_by(at, f32::total_cmp);
                *value
            },
        )
        .collect();
    Plane {
        width,
        height,
        data,
    }
}

/// Compact bright sources: stars a session median kept. A pixel is one
/// when it stands more than [`STAR_SIGMA`] robust sigmas above the upper
/// quartile of its [`STAR_WINDOW`]-pixel neighbourhood; the mask then grows
/// by [`STAR_GROW`]. The upper quartile rather than the median, so the
/// bright side of a dark shadow's rim, half of whose neighbourhood lies in
/// the shadow, is not taken for a star. Dust is a dip, so it is never
/// masked. Shared by the comparison and the calibration browser's session
/// picture.
pub fn star_mask(plane: &Plane) -> Vec<bool> {
    star_mask_grown(plane, STAR_GROW)
}

/// [`star_mask`] grown by `by` pixels rather than the default.
pub fn star_mask_grown(plane: &Plane, by: usize) -> Vec<bool> {
    let local = quantile_filter(plane, STAR_WINDOW, 0.75);
    let excess: Vec<f32> = plane
        .data
        .iter()
        .zip(&local.data)
        .map(|(value, around)| {
            if value.is_finite() && around.is_finite() && *around != 0.0 {
                value / around - 1.0
            } else {
                f32::NAN
            }
        })
        .collect();
    let finite: Vec<f32> = excess.iter().copied().filter(|v| v.is_finite()).collect();
    let mut sorted = finite.clone();
    let middle = median_in_place(&mut sorted);
    let sigma = robust_sigma(&finite);
    let threshold = middle + STAR_SIGMA * sigma.max(1e-6);
    let bright: Vec<bool> = excess
        .iter()
        .map(|v| v.is_finite() && *v > threshold)
        .collect();
    grow(&bright, plane.width, plane.height, by)
}

fn gaussian(plane: &[f32], width: usize, height: usize, sigma: f64) -> Vec<f32> {
    gaussian_blur_f32(
        plane,
        width,
        height,
        auto_ksize_f32(sigma),
        sigma,
        BorderMode::Reflect,
    )
}

/// `x^i y^j` for `i + j ≤ degree`.
fn terms(x: f64, y: f64, degree: usize) -> Vec<f64> {
    let mut out = Vec::new();
    for i in 0..=degree {
        for j in 0..=(degree - i) {
            out.push(x.powi(i as i32) * y.powi(j as i32));
        }
    }
    out
}

fn least_squares(rows: &[Vec<f64>], values: &[f64]) -> Option<Vec<f64>> {
    let columns = rows.first()?.len();
    if rows.len() < columns {
        return None;
    }
    let a = nalgebra::DMatrix::from_fn(rows.len(), columns, |r, c| rows[r][c]);
    let b = nalgebra::DVector::from_column_slice(values);
    let solution = a.svd(true, true).solve(&b, 1e-12).ok()?;
    Some(solution.iter().copied().collect())
}

/// A robust smooth surface over a plane, and its best plane: a degree-4
/// polynomial fitted to block medians, clipping blocks that hold dust. NaN
/// pixels take no part. `None` when too few blocks hold data.
fn smooth_surface(plane: &Plane) -> Option<(Vec<f32>, Vec<f32>)> {
    let (width, height) = (plane.width, plane.height);
    let block = (width.max(height) / SURFACE_BLOCKS).max(1);
    let (grid_w, grid_h) = (width / block, height / block);
    if grid_w < 4 || grid_h < 4 {
        return None;
    }
    let mut points = Vec::new();
    let mut values = Vec::new();
    for gy in 0..grid_h {
        for gx in 0..grid_w {
            let mut samples = Vec::with_capacity(block * block);
            for y in gy * block..(gy + 1) * block {
                samples.extend(
                    plane.data[y * width + gx * block..y * width + (gx + 1) * block]
                        .iter()
                        .copied()
                        .filter(|value| value.is_finite()),
                );
            }
            if samples.len() * 2 < block * block {
                continue;
            }
            let value = median_in_place(&mut samples);
            let x = ((gx as f64 + 0.5) * block as f64) / width as f64 * 2.0 - 1.0;
            let y = ((gy as f64 + 0.5) * block as f64) / height as f64 * 2.0 - 1.0;
            points.push((x, y));
            values.push(f64::from(value));
        }
    }
    let mut keep = vec![true; points.len()];
    let mut coefficients = Vec::new();
    for _ in 0..=SURFACE_ROUNDS {
        let rows: Vec<Vec<f64>> = points
            .iter()
            .zip(&keep)
            .filter(|(_, keep)| **keep)
            .map(|((x, y), _)| terms(*x, *y, SURFACE_DEGREE))
            .collect();
        let kept: Vec<f64> = values
            .iter()
            .zip(&keep)
            .filter(|(_, keep)| **keep)
            .map(|(value, _)| *value)
            .collect();
        coefficients = least_squares(&rows, &kept)?;
        let residuals: Vec<f32> = points
            .iter()
            .zip(&values)
            .map(|((x, y), value)| {
                let fit: f64 = terms(*x, *y, SURFACE_DEGREE)
                    .iter()
                    .zip(&coefficients)
                    .map(|(t, c)| t * c)
                    .sum();
                (value - fit) as f32
            })
            .collect();
        let kept_residuals: Vec<f32> = residuals
            .iter()
            .zip(&keep)
            .filter(|(_, keep)| **keep)
            .map(|(r, _)| *r)
            .collect();
        let sigma = robust_sigma(&kept_residuals).max(1e-7);
        keep = residuals.iter().map(|r| r.abs() <= 3.0 * sigma).collect();
    }
    // The best plane through the fitted surface at the kept blocks.
    let fitted: Vec<f64> = points
        .iter()
        .map(|(x, y)| {
            terms(*x, *y, SURFACE_DEGREE)
                .iter()
                .zip(&coefficients)
                .map(|(t, c)| t * c)
                .sum()
        })
        .collect();
    let plane_rows: Vec<Vec<f64>> = points
        .iter()
        .zip(&keep)
        .filter(|(_, keep)| **keep)
        .map(|((x, y), _)| vec![1.0, *x, *y])
        .collect();
    let plane_values: Vec<f64> = fitted
        .iter()
        .zip(&keep)
        .filter(|(_, keep)| **keep)
        .map(|(value, _)| *value)
        .collect();
    let tilt = least_squares(&plane_rows, &plane_values)?;
    let mut surface = vec![0f32; width * height];
    let mut flat = vec![0f32; width * height];
    surface
        .par_chunks_mut(width)
        .zip(flat.par_chunks_mut(width))
        .enumerate()
        .for_each(|(y, (surface_row, flat_row))| {
            let sy = (y as f64 + 0.5) / height as f64 * 2.0 - 1.0;
            for x in 0..width {
                let sx = (x as f64 + 0.5) / width as f64 * 2.0 - 1.0;
                surface_row[x] = terms(sx, sy, SURFACE_DEGREE)
                    .iter()
                    .zip(&coefficients)
                    .map(|(t, c)| t * c)
                    .sum::<f64>() as f32;
                flat_row[x] = (tilt[0] + tilt[1] * sx + tilt[2] * sy) as f32;
            }
        });
    Some((surface, flat))
}

/// A plane's own compact residual against its own smooth surface, smoothed
/// by one pixel: negative where it holds a dip.
fn own_residual(plane: &Plane) -> Option<Vec<f32>> {
    let (surface, _) = smooth_surface(plane)?;
    let residual: Vec<f32> = plane
        .data
        .iter()
        .zip(&surface)
        .map(|(value, smooth)| {
            if value.is_finite() && *smooth != 0.0 {
                value / smooth - 1.0
            } else {
                0.0
            }
        })
        .collect();
    Some(gaussian(&residual, plane.width, plane.height, 1.0))
}

/// Subtract a wide local median: what the polynomial cannot follow, such as
/// a sky gradient bending into a corner. Worked out on every fourth pixel
/// and spread back bilinearly.
fn subtract_local_median(map: &mut [f32], width: usize, height: usize) {
    const STEP: usize = 4;
    let window = (width.max(height) / LOCAL_FRACTION) | 1;
    let size = ((window / STEP).max(3)) | 1;
    let (small_w, small_h) = (width.div_ceil(STEP), height.div_ceil(STEP));
    let small = Plane {
        width: small_w,
        height: small_h,
        data: (0..small_w * small_h)
            .map(|index| map[(index / small_w) * STEP * width + (index % small_w) * STEP])
            .collect(),
    };
    let coarse = median_filter(&small, size);
    map.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        let fy = (y as f32 / STEP as f32).min((small_h - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy - fy.floor());
        let y1 = (y0 + 1).min(small_h - 1);
        for (x, value) in row.iter_mut().enumerate() {
            let fx = (x as f32 / STEP as f32).min((small_w - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx - fx.floor());
            let x1 = (x0 + 1).min(small_w - 1);
            let at = |xx: usize, yy: usize| {
                let v = coarse.data[yy * small_w + xx];
                if v.is_finite() {
                    v
                } else {
                    0.0
                }
            };
            let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
            let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
            *value -= top * (1.0 - ty) + bottom * ty;
        }
    });
}

/// What a spot is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpotKind {
    /// The later session is darker here: dust that arrived.
    New,
    /// The later session is brighter here: dust that left.
    Gone,
    /// One of a new and a gone spot of like size and depth close together.
    Moved,
}

/// One compact difference between two sessions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spot {
    pub kind: SpotKind,
    /// Whether the later session is darker here. A moved shadow has one
    /// spot of each.
    pub darker: bool,
    /// Centre, in sensor pixels.
    pub x: f32,
    pub y: f32,
    /// Radius of a disc of the same area, in sensor pixels.
    pub radius: f32,
    /// Area in sensor pixels.
    pub area: f32,
    /// Deepest departure, as a fraction of the background.
    pub peak: f32,
}

/// How two sessions differ, in fractions of the sky background.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub algorithm_version: u32,
    pub broad: f32,
    /// Where the broad maximum lies, as fractions of the width and height.
    pub broad_x: f32,
    pub broad_y: f32,
    pub dust: f32,
    /// Robust spread of the smoothed residual.
    pub sigma: f32,
    /// Deepest first, at most [`MAX_SPOTS`].
    pub spots: Vec<Spot>,
    /// Share of pixels masked as stars.
    pub star_fraction: f32,
}

/// The ratio of two responses, and the masks and maps the metrics share.
struct Ratio {
    width: usize,
    height: usize,
    valid: Vec<bool>,
    stars: Vec<bool>,
    q: Vec<f32>,
    surface: Vec<f32>,
    tilt: Vec<f32>,
}

impl Ratio {
    /// `Q` against its surface, less a wide local median, smoothed by one
    /// pixel: the compact part of the difference. Stars and invalid pixels
    /// read as 0.
    fn compact_residual(&self) -> Vec<f32> {
        let (width, height) = (self.width, self.height);
        let mut residual: Vec<f32> = (0..width * height)
            .map(|index| {
                if self.valid[index] && !self.stars[index] && self.surface[index] != 0.0 {
                    self.q[index] / self.surface[index] - 1.0
                } else {
                    0.0
                }
            })
            .collect();
        subtract_local_median(&mut residual, width, height);
        gaussian(&residual, width, height, 1.0)
    }
}

/// The compact part of `numerator / denominator`, as [`compare`] measures
/// dust, with the pixels that took no part: stars in either plane and
/// pixels without data. For fitting a light's own background against a
/// change ([`super::evidence`]).
pub(crate) fn compact_ratio(
    denominator: &Plane,
    numerator: &Plane,
    star_grow: usize,
) -> Option<(Vec<f32>, Vec<bool>)> {
    let ratio = ratio(denominator, numerator, star_grow)?;
    let residual = ratio.compact_residual();
    let excluded = ratio
        .valid
        .iter()
        .zip(&ratio.stars)
        .map(|(valid, star)| !valid || *star)
        .collect();
    Some((residual, excluded))
}

fn ratio(earlier: &Plane, later: &Plane, star_grow: usize) -> Option<Ratio> {
    if !earlier.same_size(later) || earlier.width < 16 || earlier.height < 16 {
        return None;
    }
    let (width, height) = (earlier.width, earlier.height);
    let valid: Vec<bool> = earlier
        .data
        .iter()
        .zip(&later.data)
        .map(|(a, b)| a.is_finite() && b.is_finite() && *a > 0.05 && *b > 0.05)
        .collect();
    let stars: Vec<bool> = star_mask_grown(earlier, star_grow)
        .into_iter()
        .zip(star_mask_grown(later, star_grow))
        .map(|(a, b)| a || b)
        .collect();
    let raw: Vec<f32> = earlier
        .data
        .iter()
        .zip(&later.data)
        .zip(&valid)
        .map(|((a, b), ok)| if *ok { b / a } else { f32::NAN })
        .collect();
    let middle = sampled_median(&raw);
    if !(middle.is_finite() && middle > 0.0) {
        return None;
    }
    let q: Vec<f32> = raw.iter().map(|v| v / middle).collect();
    let masked = Plane {
        width,
        height,
        data: q
            .iter()
            .zip(&stars)
            .map(|(v, star)| if *star { f32::NAN } else { *v })
            .collect(),
    };
    let (surface, tilt) = smooth_surface(&masked)?;
    Some(Ratio {
        width,
        height,
        valid,
        stars,
        q,
        surface,
        tilt,
    })
}

fn inner_band(width: usize, height: usize) -> impl Fn(usize) -> bool {
    let bx = (width as f64 * BORDER_FRACTION) as usize;
    let by = (height as f64 * BORDER_FRACTION) as usize;
    move |index| {
        let (x, y) = (index % width, index / width);
        x >= bx && x < width - bx && y >= by && y < height - by
    }
}

/// Compare two responses of one family, their pedestals already removed.
/// `bin` converts response pixels to sensor pixels. `None` when the
/// responses differ in size or hold too little to fit.
pub fn compare(earlier: &Plane, later: &Plane, bin: usize) -> Option<Comparison> {
    let ratio = ratio(earlier, later, STAR_GROW)?;
    let (width, height) = (ratio.width, ratio.height);
    let inner = inner_band(width, height);

    // Broad: the smooth surface against its best plane.
    let mut broad = 0f32;
    let mut broad_at = 0usize;
    for index in 0..width * height {
        if !inner(index) || !ratio.valid[index] || ratio.tilt[index] == 0.0 {
            continue;
        }
        let departure = (ratio.surface[index] / ratio.tilt[index] - 1.0).abs();
        if departure > broad {
            broad = departure;
            broad_at = index;
        }
    }

    // Dust: what is left after the surface and a wide local median.
    let smoothed = ratio.compact_residual();
    let core: Vec<f32> = (0..width * height)
        .filter(|index| inner(*index))
        .map(|index| smoothed[index])
        .collect();
    let sigma = robust_sigma(&core).max(1e-6);
    let own_earlier = own_residual(earlier)?;
    let own_later = own_residual(later)?;

    let pixel_area = (bin * bin) as f64;
    let mut found: Vec<(Spot, f32)> = Vec::new();
    for sign in [-1f32, 1f32] {
        let mask: Vec<u8> = (0..width * height)
            .map(|index| u8::from(inner(index) && sign * smoothed[index] > SPOT_SIGMA * sigma))
            .collect();
        for component in connected_components(&mask, width, height, Connectivity::Eight) {
            let area = component.pixels.len() as f64 * pixel_area;
            if area < MIN_SPOT_SENSOR_PIXELS {
                continue;
            }
            // Dust is dark: one response must hold a dip here deeper than
            // any bump the other holds. A star left in either is a bump.
            let (dipping, bumping) = if sign < 0.0 {
                (&own_later, &own_earlier)
            } else {
                (&own_earlier, &own_later)
            };
            let dip = component
                .pixels
                .iter()
                .map(|index| dipping[*index])
                .fold(f32::INFINITY, f32::min);
            let bump = component
                .pixels
                .iter()
                .map(|index| bumping[*index])
                .fold(f32::NEG_INFINITY, f32::max);
            if bump > -dip {
                continue;
            }
            let peak = component
                .pixels
                .iter()
                .map(|index| sign * smoothed[*index])
                .fold(0f32, f32::max);
            found.push((
                Spot {
                    kind: if sign < 0.0 {
                        SpotKind::New
                    } else {
                        SpotKind::Gone
                    },
                    darker: sign < 0.0,
                    x: (component.centroid_x + 0.5) * bin as f32,
                    y: (component.centroid_y + 0.5) * bin as f32,
                    radius: (area / std::f64::consts::PI).sqrt() as f32,
                    area: area as f32,
                    peak,
                },
                sign,
            ));
        }
    }
    pair_moved(&mut found);
    let mut spots: Vec<Spot> = found.into_iter().map(|(spot, _)| spot).collect();
    spots.sort_by(|a, b| b.peak.total_cmp(&a.peak).then(a.x.total_cmp(&b.x)));
    let dust = spots.first().map_or(0.0, |spot| spot.peak);
    spots.truncate(MAX_SPOTS);
    let star_fraction =
        ratio.stars.iter().filter(|star| **star).count() as f32 / (width * height) as f32;
    Some(Comparison {
        algorithm_version: COMPARE_ALGORITHM_VERSION,
        broad,
        broad_x: (broad_at % width) as f32 / width as f32,
        broad_y: (broad_at / width) as f32 / height as f32,
        dust,
        sigma,
        spots,
        star_fraction,
    })
}

/// A new spot and a gone spot of like size and depth within three radii are
/// one shadow that moved.
fn pair_moved(found: &mut [(Spot, f32)]) {
    let mut taken = vec![false; found.len()];
    for i in 0..found.len() {
        if taken[i] || found[i].0.kind != SpotKind::New {
            continue;
        }
        let best = (0..found.len())
            .filter(|j| !taken[*j] && found[*j].0.kind == SpotKind::Gone)
            .filter(|j| {
                let (a, b) = (&found[i].0, &found[*j].0);
                let distance = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
                let ratio = |p: f32, q: f32| p.max(q) / p.min(q).max(1e-9);
                distance <= 3.0 * a.radius.max(b.radius)
                    && ratio(a.peak, b.peak) <= 2.0
                    && ratio(a.area, b.area) <= 3.0
            })
            .min_by(|a, b| {
                let d = |j: usize| {
                    (found[i].0.x - found[j].0.x).powi(2) + (found[i].0.y - found[j].0.y).powi(2)
                };
                d(*a).total_cmp(&d(*b))
            });
        if let Some(j) = best {
            taken[i] = true;
            taken[j] = true;
            found[i].0.kind = SpotKind::Moved;
            found[j].0.kind = SpotKind::Moved;
        }
    }
}

/// The compact change between a session before an event and one after it,
/// in the responses' own (sensor-aligned) pixels: `after / before` with the
/// smooth part and stars taken out, and exactly 1 away from the change. Dividing an after-flat by it gives a before-flat.
pub fn change_map(before: &Plane, after: &Plane) -> Option<Plane> {
    let ratio = ratio(before, after, STAR_GROW)?;
    let (width, height) = (ratio.width, ratio.height);
    let shape: Vec<f32> = (0..width * height)
        .map(|index| {
            if ratio.valid[index] && !ratio.stars[index] && ratio.surface[index] != 0.0 {
                ratio.q[index] / ratio.surface[index]
            } else {
                1.0
            }
        })
        .collect();
    let deviation: Vec<f32> = shape.iter().map(|v| v - 1.0).collect();
    let smoothed = gaussian(&deviation, width, height, 1.0);
    let sigma = robust_sigma(&smoothed).max(1e-6);
    let support: Vec<bool> = smoothed.iter().map(|v| v.abs() > 3.0 * sigma).collect();
    let support = grow(&support, width, height, 3);
    // Unsmoothed inside the support: a shadow's rim is where a blurred map
    // would leave its own print.
    Some(Plane {
        width,
        height,
        data: shape
            .iter()
            .zip(&support)
            .map(|(value, keep)| {
                if *keep && value.is_finite() {
                    *value
                } else {
                    1.0
                }
            })
            .collect(),
    })
}

/// Carry a flat across a change in place: divide by the change's map for a
/// light shot before it (`divide`), multiply for one shot after. `data` is
/// row-major, `channels` interleaved, at sensor resolution; the map is on
/// the response grid, `bin` sensor pixels to one.
pub fn apply_change_map(
    data: &mut [f32],
    width: usize,
    channels: usize,
    map: &Plane,
    bin: usize,
    divide: bool,
) {
    let channels = channels.max(1);
    data.par_chunks_mut(width * channels)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..width {
                let factor = map_at(map, bin, x as f32, y as f32);
                if !(factor.is_finite() && factor > 0.0) {
                    continue;
                }
                for value in &mut row[x * channels..(x + 1) * channels] {
                    *value = if divide {
                        *value / factor
                    } else {
                        *value * factor
                    };
                }
            }
        });
}

/// The value of a map at a sensor pixel, bilinear between map pixels.
pub fn map_at(map: &Plane, bin: usize, x: f32, y: f32) -> f32 {
    let fx = ((x + 0.5) / bin as f32 - 0.5).clamp(0.0, (map.width - 1) as f32);
    let fy = ((y + 0.5) / bin as f32 - 0.5).clamp(0.0, (map.height - 1) as f32);
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(map.width - 1), (y0 + 1).min(map.height - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let at = |xx: usize, yy: usize| map.data[yy * map.width + xx];
    let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
    let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
    top * (1.0 - ty) + bottom * ty
}

/// Encode a plane for the cache: 16-bit fixed point, `value × 32768`,
/// covering 0 to 2. Not-a-number reads back as 0.
pub fn encode_plane(plane: &Plane) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + plane.data.len() * 2);
    out.extend_from_slice(&(plane.width as u32).to_le_bytes());
    out.extend_from_slice(&(plane.height as u32).to_le_bytes());
    for value in &plane.data {
        let fixed = if value.is_finite() {
            (value * 32768.0).round().clamp(0.0, 65535.0) as u16
        } else {
            0
        };
        out.extend_from_slice(&fixed.to_le_bytes());
    }
    out
}

pub fn decode_plane(bytes: &[u8]) -> Option<Plane> {
    let width = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?) as usize;
    let height = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
    let body = bytes.get(8..)?;
    if body.len() != width * height * 2 {
        return None;
    }
    Some(Plane {
        width,
        height,
        data: body
            .chunks_exact(2)
            .map(|pair| {
                let fixed = u16::from_le_bytes([pair[0], pair[1]]);
                if fixed == 0 {
                    f32::NAN
                } else {
                    f32::from(fixed) / 32768.0
                }
            })
            .collect(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A deterministic, roughly normal number with unit spread.
    pub(crate) fn noise(x: usize, y: usize, seed: u64) -> f32 {
        let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
            ^ seed.wrapping_mul(0x1656_67B1_9E37_79F9);
        let mut sum = 0f32;
        for _ in 0..4 {
            h ^= h >> 33;
            h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
            h ^= h >> 33;
            sum += (h >> 40) as f32 / (1u64 << 24) as f32;
        }
        (sum - 2.0) * 1.732
    }

    /// A synthetic flat response: radial vignetting, optional dust shadows
    /// `(x, y, radius, depth)` in plane pixels, a linear gradient, a few
    /// stars `(x, y, height)`, and noise of `noise_level`.
    pub(crate) fn synthetic(
        width: usize,
        height: usize,
        dust: &[(f32, f32, f32, f32)],
        gradient: f32,
        stars: &[(f32, f32, f32)],
        noise_level: f32,
        seed: u64,
    ) -> Plane {
        let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
        let r_max = (cx * cx + cy * cy).sqrt();
        let mut data = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let r = (dx * dx + dy * dy).sqrt() / r_max;
                let mut value = 1.0 - 0.3 * r * r;
                value *= 1.0 + gradient * (x as f32 / width as f32 - 0.5);
                for (sx, sy, radius, depth) in dust {
                    let d = ((x as f32 - sx).powi(2) + (y as f32 - sy).powi(2)).sqrt();
                    // A soft-edged out-of-focus disc.
                    let edge = ((radius - d) / 2.0).clamp(0.0, 1.0);
                    value *= 1.0 - depth * edge;
                }
                for (sx, sy, peak) in stars {
                    let d2 = (x as f32 - sx).powi(2) + (y as f32 - sy).powi(2);
                    value += peak * (-d2 / 2.0).exp();
                }
                value *= 1.0 + noise_level * noise(x, y, seed);
                data.push(value);
            }
        }
        Plane {
            width,
            height,
            data,
        }
    }

    fn compare_plain(a: &Plane, b: &Plane) -> Comparison {
        compare(a, b, 10).expect("comparable")
    }

    #[test]
    fn the_same_dust_with_fresh_noise_reads_as_nothing() {
        let dust = [(80.0, 60.0, 15.0, 0.02)];
        let a = synthetic(240, 160, &dust, 0.0, &[], 0.0005, 1);
        let b = synthetic(240, 160, &dust, 0.0, &[], 0.0005, 2);
        let c = compare_plain(&a, &b);
        assert!(c.dust < 0.002, "dust {}", c.dust);
        assert!(c.broad < 0.002, "broad {}", c.broad);
    }

    #[test]
    fn new_dust_reads_at_its_depth() {
        let a = synthetic(240, 160, &[], 0.0, &[], 0.0005, 1);
        let b = synthetic(240, 160, &[(150.0, 80.0, 18.0, 0.016)], 0.0, &[], 0.0005, 2);
        let c = compare_plain(&a, &b);
        assert!((c.dust - 0.016).abs() < 0.004, "dust {}", c.dust);
        assert_eq!(c.spots[0].kind, SpotKind::New);
        assert!((c.spots[0].x - 1505.0).abs() < 60.0, "x {}", c.spots[0].x);
        let gone = compare_plain(&b, &a);
        assert_eq!(gone.spots[0].kind, SpotKind::Gone);
    }

    #[test]
    fn a_moved_shadow_pairs_its_two_halves() {
        let a = synthetic(240, 160, &[(90.0, 80.0, 14.0, 0.02)], 0.0, &[], 0.0005, 1);
        let b = synthetic(240, 160, &[(125.0, 80.0, 14.0, 0.02)], 0.0, &[], 0.0005, 2);
        let c = compare_plain(&a, &b);
        assert!(c.dust > 0.012, "dust {}", c.dust);
        assert!(c
            .spots
            .iter()
            .take(2)
            .all(|spot| spot.kind == SpotKind::Moved));
    }

    #[test]
    fn a_straight_gradient_is_not_a_difference() {
        let a = synthetic(240, 160, &[], 0.0, &[], 0.0005, 1);
        let b = synthetic(240, 160, &[], 0.03, &[], 0.0005, 2);
        let c = compare_plain(&a, &b);
        assert!(c.broad < 0.003, "broad {}", c.broad);
        assert!(c.dust < 0.002, "dust {}", c.dust);
    }

    #[test]
    fn a_changed_vignette_reads_as_broad() {
        let a = synthetic(240, 160, &[], 0.0, &[], 0.0005, 1);
        let mut b = synthetic(240, 160, &[], 0.0, &[], 0.0005, 2);
        let (cx, cy) = (120.0f32, 80.0f32);
        for y in 0..160 {
            for x in 0..240 {
                let r2 = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)) / (cx * cx + cy * cy);
                b.data[y * 240 + x] *= 1.0 - 0.03 * r2;
            }
        }
        let c = compare_plain(&a, &b);
        assert!(c.broad > 0.015, "broad {}", c.broad);
        assert!(c.dust < 0.003, "dust {}", c.dust);
    }

    #[test]
    fn stars_that_sat_still_are_not_dust() {
        let stars_a: Vec<(f32, f32, f32)> = (0..30)
            .map(|i| {
                (
                    10.0 + (i * 37 % 220) as f32,
                    8.0 + (i * 53 % 145) as f32,
                    0.3 + (i % 5) as f32 * 0.1,
                )
            })
            .collect();
        let stars_b: Vec<(f32, f32, f32)> = (0..30)
            .map(|i| {
                (
                    12.0 + (i * 41 % 216) as f32,
                    9.0 + (i * 29 % 142) as f32,
                    0.2 + (i % 4) as f32 * 0.15,
                )
            })
            .collect();
        let a = synthetic(240, 160, &[], 0.0, &stars_a, 0.0005, 1);
        let b = synthetic(240, 160, &[], 0.0, &stars_b, 0.0005, 2);
        let c = compare_plain(&a, &b);
        assert!(c.dust < 0.004, "dust {}", c.dust);
        assert!(c.star_fraction > 0.0);
    }

    #[test]
    fn a_response_takes_out_moving_stars_and_its_level() {
        let frames: Vec<Plane> = (0..5)
            .map(|i| {
                let mut plane = synthetic(
                    64,
                    48,
                    &[],
                    0.0,
                    &[(10.0 + 8.0 * i as f32, 20.0, 0.5)],
                    0.001,
                    i,
                );
                let level = 20_000.0 + 1000.0 * i as f32;
                plane.data.iter_mut().for_each(|v| *v *= level);
                plane
            })
            .collect();
        let response = build_response(&frames, 4).expect("response");
        assert_eq!(response.frames, 5);
        let clean = synthetic(64, 48, &[], 0.0, &[], 0.0, 0);
        let level = sampled_median(&clean.data);
        let worst = response
            .plane
            .data
            .iter()
            .zip(&clean.data)
            .map(|(r, c)| (r / (c / level) - 1.0).abs())
            .fold(0f32, f32::max);
        assert!(worst < 0.02, "a star survived: {worst}");
        assert!(
            response.level > 17_000.0 && response.level < 23_000.0,
            "level {}",
            response.level
        );
    }

    #[test]
    fn a_change_map_turns_an_after_flat_into_a_before_flat() {
        let before = synthetic(240, 160, &[], 0.0, &[], 0.0005, 1);
        let after = synthetic(240, 160, &[(120.0, 80.0, 16.0, 0.02)], 0.0, &[], 0.0005, 2);
        let map = change_map(&before, &after).expect("map");
        let corrected = Plane {
            width: 240,
            height: 160,
            data: after
                .data
                .iter()
                .zip(&map.data)
                .map(|(a, m)| a / m)
                .collect(),
        };
        let c = compare_plain(&before, &corrected);
        assert!(c.dust < 0.004, "left {}", c.dust);
    }

    #[test]
    fn planes_round_trip_through_the_cache_encoding() {
        let plane = synthetic(32, 16, &[], 0.0, &[], 0.0, 1);
        let back = decode_plane(&encode_plane(&plane)).expect("decodes");
        assert!(plane
            .data
            .iter()
            .zip(&back.data)
            .all(|(a, b)| (a - b).abs() < 1e-4));
    }
}
