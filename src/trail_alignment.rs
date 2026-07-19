//! Pixel evidence for orbital satellite-track predictions.
//!
//! The orbital path remains the provenance-bearing prediction. This module
//! only searches a narrow corridor around that path for a linear brightness
//! feature and reports a separate, aligned sensor-space segment when one is
//! present. It deliberately does not identify arbitrary trails without an
//! orbital candidate.

use serde::{Deserialize, Serialize};

use crate::FitsImage;

pub const PIXEL_ALIGNMENT_VERSION: u32 = 1;

const MAX_WORKING_DIMENSION: usize = 2048;
const SEARCH_RADIUS_WORKING_PX: f64 = 32.0;
const COARSE_STEP_PX: f64 = 2.0;
const REFINE_STEP_PX: f64 = 0.5;
const MIN_WORKING_LENGTH_PX: f64 = 30.0;
const MIN_SAMPLES: usize = 80;
const MAX_SAMPLES: usize = 1_200;
const MIN_CONTRAST_SIGMA: f64 = 2.0;
const MIN_CONTINUITY: f64 = 0.65;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelTrailAlignmentStatus {
    Detected,
    NotDetected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PixelTrailAlignment {
    pub status: PixelTrailAlignmentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aligned_segment: Option<[[f64; 2]; 2]>,
    pub start_normal_offset_px: f64,
    pub end_normal_offset_px: f64,
    pub mean_normal_offset_px: f64,
    pub angle_delta_deg: f64,
    pub contrast_adu: f64,
    pub contrast_sigma: f64,
    pub continuity: f64,
    pub search_radius_px: f64,
}

impl PixelTrailAlignment {
    pub fn detected(&self) -> bool {
        self.status == PixelTrailAlignmentStatus::Detected
    }
}

#[derive(Debug, Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
}

impl Point {
    fn add_scaled(self, direction: Point, amount: f64) -> Self {
        Self {
            x: self.x + direction.x * amount,
            y: self.y + direction.y * amount,
        }
    }
}

struct WorkingImage {
    width: usize,
    height: usize,
    data: Vec<f64>,
    scale_to_sensor: f64,
    raw_scale: f64,
}

/// Reusable per-frame working image. Construct once, then align every orbital
/// candidate without repeating the full-frame downsample.
pub struct PixelTrailAligner {
    working: WorkingImage,
}

#[derive(Debug, Clone, Copy)]
struct LineScore {
    contrast: f64,
    contrast_sigma: f64,
    continuity: f64,
    objective: f64,
}

/// Search the FITS pixels in a bounded corridor around a predicted path.
pub fn align_track(image: &FitsImage, clipped_segments: &[[[f64; 2]; 2]]) -> PixelTrailAlignment {
    PixelTrailAligner::new(image).align_track(clipped_segments)
}

impl PixelTrailAligner {
    pub fn new(image: &FitsImage) -> Self {
        Self {
            working: WorkingImage::from_fits(image),
        }
    }

    pub fn align_track(&self, clipped_segments: &[[[f64; 2]; 2]]) -> PixelTrailAlignment {
        let working = &self.working;
        let search_radius_px = SEARCH_RADIUS_WORKING_PX * working.scale_to_sensor;
        let Some((sensor_start, sensor_end)) = predicted_endpoints(clipped_segments) else {
            return not_detected(search_radius_px);
        };
        let predicted_start = Point {
            x: sensor_start.x / working.scale_to_sensor,
            y: sensor_start.y / working.scale_to_sensor,
        };
        let predicted_end = Point {
            x: sensor_end.x / working.scale_to_sensor,
            y: sensor_end.y / working.scale_to_sensor,
        };
        let dx = predicted_end.x - predicted_start.x;
        let dy = predicted_end.y - predicted_start.y;
        let length = dx.hypot(dy);
        if !length.is_finite() || length < MIN_WORKING_LENGTH_PX {
            return not_detected(search_radius_px);
        }
        let normal = Point {
            x: -dy / length,
            y: dx / length,
        };
        let noise = working.local_noise_sigma().max(1e-6);

        let mut best: Option<(f64, f64, LineScore)> = None;
        search_offsets(
            -SEARCH_RADIUS_WORKING_PX,
            SEARCH_RADIUS_WORKING_PX,
            COARSE_STEP_PX,
            |start_offset, end_offset| {
                let start = predicted_start.add_scaled(normal, start_offset);
                let end = predicted_end.add_scaled(normal, end_offset);
                let Some((start, end)) = clip_line(
                    start,
                    end,
                    working.width as f64 - 1.001,
                    working.height as f64 - 1.001,
                ) else {
                    return;
                };
                let score = working.line_score(start, end, noise);
                if best.is_none_or(|(_, _, current)| score.objective > current.objective) {
                    best = Some((start_offset, end_offset, score));
                }
            },
        );

        let Some((coarse_start, coarse_end, _)) = best else {
            return not_detected(search_radius_px);
        };
        search_offsets_2d(
            coarse_start - COARSE_STEP_PX,
            coarse_start + COARSE_STEP_PX,
            coarse_end - COARSE_STEP_PX,
            coarse_end + COARSE_STEP_PX,
            REFINE_STEP_PX,
            |start_offset, end_offset| {
                if start_offset.abs() > SEARCH_RADIUS_WORKING_PX
                    || end_offset.abs() > SEARCH_RADIUS_WORKING_PX
                {
                    return;
                }
                let start = predicted_start.add_scaled(normal, start_offset);
                let end = predicted_end.add_scaled(normal, end_offset);
                let Some((start, end)) = clip_line(
                    start,
                    end,
                    working.width as f64 - 1.001,
                    working.height as f64 - 1.001,
                ) else {
                    return;
                };
                let score = working.line_score(start, end, noise);
                if best.is_none_or(|(_, _, current)| score.objective > current.objective) {
                    best = Some((start_offset, end_offset, score));
                }
            },
        );

        let (start_offset, end_offset, score) = best.expect("coarse search produced a candidate");
        let candidate_start = predicted_start.add_scaled(normal, start_offset);
        let candidate_end = predicted_end.add_scaled(normal, end_offset);
        let clipped = clip_line(
            candidate_start,
            candidate_end,
            working.width as f64 - 1.001,
            working.height as f64 - 1.001,
        );
        let detected = score.contrast_sigma >= MIN_CONTRAST_SIGMA
            && score.continuity >= MIN_CONTINUITY
            && clipped.is_some();
        let aligned_segment = detected.then(|| {
            let (start, end) = clipped.expect("detected candidates are clipped");
            [
                [
                    start.x * working.scale_to_sensor,
                    start.y * working.scale_to_sensor,
                ],
                [
                    end.x * working.scale_to_sensor,
                    end.y * working.scale_to_sensor,
                ],
            ]
        });
        let angle_delta_deg = angle_delta_degrees(
            predicted_start,
            predicted_end,
            candidate_start,
            candidate_end,
        );

        PixelTrailAlignment {
            status: if detected {
                PixelTrailAlignmentStatus::Detected
            } else {
                PixelTrailAlignmentStatus::NotDetected
            },
            aligned_segment,
            start_normal_offset_px: start_offset * working.scale_to_sensor,
            end_normal_offset_px: end_offset * working.scale_to_sensor,
            mean_normal_offset_px: (start_offset + end_offset) * 0.5 * working.scale_to_sensor,
            angle_delta_deg,
            contrast_adu: score.contrast / working.raw_scale,
            contrast_sigma: score.contrast_sigma,
            continuity: score.continuity,
            search_radius_px,
        }
    }
}

fn not_detected(search_radius_px: f64) -> PixelTrailAlignment {
    PixelTrailAlignment {
        status: PixelTrailAlignmentStatus::NotDetected,
        aligned_segment: None,
        start_normal_offset_px: 0.0,
        end_normal_offset_px: 0.0,
        mean_normal_offset_px: 0.0,
        angle_delta_deg: 0.0,
        contrast_adu: 0.0,
        contrast_sigma: 0.0,
        continuity: 0.0,
        search_radius_px,
    }
}

fn predicted_endpoints(segments: &[[[f64; 2]; 2]]) -> Option<(Point, Point)> {
    let first = segments.first()?;
    let last = segments.last()?;
    Some((
        Point {
            x: first[0][0],
            y: first[0][1],
        },
        Point {
            x: last[1][0],
            y: last[1][1],
        },
    ))
}

impl WorkingImage {
    fn from_fits(image: &FitsImage) -> Self {
        let longest = image.width.max(image.height);
        let factor = longest.div_ceil(MAX_WORKING_DIMENSION).max(1);
        let width = image.width.div_ceil(factor);
        let height = image.height.div_ceil(factor);
        let mut data = vec![0.0; width * height];
        for working_y in 0..height {
            let y0 = working_y * factor;
            let y1 = (y0 + factor).min(image.height);
            for working_x in 0..width {
                let x0 = working_x * factor;
                let x1 = (x0 + factor).min(image.width);
                let mut sum = 0_u64;
                let mut count = 0_u64;
                for y in y0..y1 {
                    let row = &image.data[y * image.width..(y + 1) * image.width];
                    for value in &row[x0..x1] {
                        sum += u64::from(*value);
                        count += 1;
                    }
                }
                data[working_y * width + working_x] = sum as f64 / count.max(1) as f64;
            }
        }
        Self {
            width,
            height,
            data,
            scale_to_sensor: factor as f64,
            raw_scale: image.raw_scale.max(f64::MIN_POSITIVE),
        }
    }

    fn sample(&self, x: f64, y: f64) -> Option<f64> {
        if x < 0.0
            || y < 0.0
            || x >= self.width.saturating_sub(1) as f64
            || y >= self.height.saturating_sub(1) as f64
        {
            return None;
        }
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let fx = x - x0 as f64;
        let fy = y - y0 as f64;
        let i00 = self.data[y0 * self.width + x0];
        let i10 = self.data[y0 * self.width + x0 + 1];
        let i01 = self.data[(y0 + 1) * self.width + x0];
        let i11 = self.data[(y0 + 1) * self.width + x0 + 1];
        Some(
            i00 * (1.0 - fx) * (1.0 - fy)
                + i10 * fx * (1.0 - fy)
                + i01 * (1.0 - fx) * fy
                + i11 * fx * fy,
        )
    }

    fn local_noise_sigma(&self) -> f64 {
        let mut differences = Vec::with_capacity((self.width * self.height) / 32);
        for y in (1..self.height.saturating_sub(2)).step_by(6) {
            for x in (1..self.width.saturating_sub(2)).step_by(6) {
                let value = self.data[y * self.width + x];
                differences.push((value - self.data[y * self.width + x + 2]).abs());
                differences.push((value - self.data[(y + 2) * self.width + x]).abs());
            }
        }
        quantile(&mut differences, 0.5) / 0.953_872_552_4
    }

    fn line_score(&self, start: Point, end: Point, noise: f64) -> LineScore {
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        let length = dx.hypot(dy);
        if length < MIN_WORKING_LENGTH_PX {
            return LineScore {
                contrast: 0.0,
                contrast_sigma: 0.0,
                continuity: 0.0,
                objective: f64::NEG_INFINITY,
            };
        }
        let normal = Point {
            x: -dy / length,
            y: dx / length,
        };
        let samples = (length.ceil() as usize).clamp(MIN_SAMPLES, MAX_SAMPLES);
        let mut contrasts = Vec::with_capacity(samples);
        for index in 0..samples {
            let t = (index as f64 + 0.5) / samples as f64;
            let point = Point {
                x: start.x + dx * t,
                y: start.y + dy * t,
            };
            let center = [-0.45, 0.0, 0.45]
                .into_iter()
                .filter_map(|offset| self.sample_offset(point, normal, offset))
                .sum::<f64>()
                / 3.0;
            let mut sides = [-5.0, -3.0, 3.0, 5.0]
                .into_iter()
                .filter_map(|offset| self.sample_offset(point, normal, offset))
                .collect::<Vec<_>>();
            if sides.len() == 4 && center.is_finite() {
                contrasts.push(center - quantile(&mut sides, 0.5));
            }
        }
        if contrasts.len() < MIN_SAMPLES / 2 {
            return LineScore {
                contrast: 0.0,
                contrast_sigma: 0.0,
                continuity: 0.0,
                objective: f64::NEG_INFINITY,
            };
        }
        let continuity = contrasts
            .iter()
            .filter(|contrast| **contrast > noise * 0.75)
            .count() as f64
            / contrasts.len() as f64;
        let contrast = quantile(&mut contrasts, 0.60).max(0.0);
        let contrast_sigma = contrast / noise;
        LineScore {
            contrast,
            contrast_sigma,
            continuity,
            objective: contrast_sigma * (0.5 + 0.5 * continuity),
        }
    }

    fn sample_offset(&self, point: Point, normal: Point, offset: f64) -> Option<f64> {
        self.sample(point.x + normal.x * offset, point.y + normal.y * offset)
    }
}

fn quantile(values: &mut [f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_unstable_by(f64::total_cmp);
    let index = ((values.len() - 1) as f64 * fraction).round() as usize;
    values[index]
}

fn search_offsets(minimum: f64, maximum: f64, step: f64, mut visit: impl FnMut(f64, f64)) {
    search_offsets_2d(minimum, maximum, minimum, maximum, step, &mut visit);
}

fn search_offsets_2d(
    start_minimum: f64,
    start_maximum: f64,
    end_minimum: f64,
    end_maximum: f64,
    step: f64,
    mut visit: impl FnMut(f64, f64),
) {
    let mut start = start_minimum;
    while start <= start_maximum + step * 0.25 {
        let mut end = end_minimum;
        while end <= end_maximum + step * 0.25 {
            visit(start, end);
            end += step;
        }
        start += step;
    }
}

/// Clip a line segment to a rectangle using Liang-Barsky parameters.
fn clip_line(start: Point, end: Point, max_x: f64, max_y: f64) -> Option<(Point, Point)> {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let mut t0: f64 = 0.0;
    let mut t1: f64 = 1.0;
    for (p, q) in [
        (-dx, start.x),
        (dx, max_x - start.x),
        (-dy, start.y),
        (dy, max_y - start.y),
    ] {
        if p.abs() < f64::EPSILON {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let ratio = q / p;
        if p < 0.0 {
            t0 = t0.max(ratio);
        } else {
            t1 = t1.min(ratio);
        }
        if t0 > t1 {
            return None;
        }
    }
    Some((
        Point {
            x: start.x + t0 * dx,
            y: start.y + t0 * dy,
        },
        Point {
            x: start.x + t1 * dx,
            y: start.y + t1 * dy,
        },
    ))
}

fn angle_delta_degrees(
    predicted_start: Point,
    predicted_end: Point,
    aligned_start: Point,
    aligned_end: Point,
) -> f64 {
    let predicted = (predicted_end.y - predicted_start.y)
        .atan2(predicted_end.x - predicted_start.x)
        .to_degrees();
    let aligned = (aligned_end.y - aligned_start.y)
        .atan2(aligned_end.x - aligned_start.x)
        .to_degrees();
    let mut delta = aligned - predicted;
    while delta > 180.0 {
        delta -= 360.0;
    }
    while delta < -180.0 {
        delta += 360.0;
    }
    delta
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_image(width: usize, height: usize, trail: Option<(Point, Point)>) -> FitsImage {
        let mut data = vec![0_u16; width * height];
        for y in 0..height {
            for x in 0..width {
                let noise = ((x * 17 + y * 31 + x * y * 3) % 23) as u16;
                data[y * width + x] = 1_000 + noise;
            }
        }
        if let Some((start, end)) = trail {
            let steps = ((end.x - start.x).hypot(end.y - start.y).ceil() as usize) * 3;
            for index in 0..=steps {
                let t = index as f64 / steps.max(1) as f64;
                let x = start.x + (end.x - start.x) * t;
                let y = start.y + (end.y - start.y) * t;
                for oy in -1..=1 {
                    for ox in -1..=1 {
                        let px = (x.round() as isize + ox) as usize;
                        let py = (y.round() as isize + oy) as usize;
                        if px < width && py < height {
                            data[py * width + px] = data[py * width + px].saturating_add(45);
                        }
                    }
                }
            }
        }
        FitsImage {
            width,
            height,
            data,
            raw_min: 0.0,
            raw_scale: 1.0,
            bzero: 0.0,
        }
    }

    #[test]
    fn aligns_a_shifted_faint_trail_without_changing_the_prediction() {
        let predicted = [[[20.0, 110.0], [610.0, 245.0]]];
        let actual = (Point { x: 20.0, y: 116.0 }, Point { x: 610.0, y: 253.0 });
        let image = test_image(640, 360, Some(actual));

        let alignment = align_track(&image, &predicted);

        assert!(alignment.detected(), "{alignment:?}");
        assert!(alignment.aligned_segment.is_some());
        assert!(alignment.mean_normal_offset_px.abs() > 4.0);
        assert_eq!(predicted, [[[20.0, 110.0], [610.0, 245.0]]]);
    }

    #[test]
    fn does_not_invent_a_match_in_noise() {
        let predicted = [[[20.0, 110.0], [610.0, 245.0]]];
        let image = test_image(640, 360, None);

        let alignment = align_track(&image, &predicted);

        assert!(!alignment.detected(), "{alignment:?}");
        assert!(alignment.aligned_segment.is_none());
    }
}
