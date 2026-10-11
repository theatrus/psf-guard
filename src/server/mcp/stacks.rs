//! What the stack tools hand an agent: a project's stacks in brief, one
//! stack frame by frame with how each night drifted, and the stack itself as
//! an image it can look at.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::server::stack_preview::StackGroupStatus;

/// One channel's latest stack, in the words an agent needs to pick one.
#[derive(Debug, Serialize)]
pub(super) struct StackSummary {
    pub job_id: String,
    pub group_index: usize,
    pub target_id: i32,
    pub target_name: String,
    pub filter_name: String,
    pub exposure: Option<String>,
    pub state: String,
    pub created_unix_seconds: i64,
    pub frames: FrameCounts,
    pub total_exposure_hours: f64,
    pub calibration: CalibrationBrief,
    /// Whether the stack has an image `get_stack_image` can show.
    pub has_image: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct FrameCounts {
    pub candidates: usize,
    pub integrated: usize,
    pub stack_rejects: usize,
    pub quality_excluded: usize,
    pub missing_files: usize,
}

#[derive(Debug, Serialize)]
pub(super) struct CalibrationBrief {
    pub mode: String,
    pub state: String,
    /// One entry per set of masters, with the lights it calibrated.
    pub sessions: Vec<SessionMasters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// The masters one calibration session applied; `None` where it had none.
#[derive(Debug, Serialize)]
pub(super) struct SessionMasters {
    pub lights: usize,
    pub bias: Option<String>,
    pub dark: Option<String>,
    pub dark_flat: Option<String>,
    pub flat: Option<String>,
}

pub(super) fn summarize(
    job_id: &str,
    created_unix_seconds: i64,
    group: &StackGroupStatus,
) -> StackSummary {
    StackSummary {
        job_id: job_id.to_string(),
        group_index: group.index,
        target_id: group.target_id,
        target_name: group.target_name.clone(),
        filter_name: group.filter_name.clone(),
        exposure: group
            .exposure_group
            .as_ref()
            .map(|exposure| exposure.label.clone()),
        state: serde_json::to_value(group.state)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default(),
        created_unix_seconds,
        frames: FrameCounts {
            candidates: group.total_candidates,
            integrated: group.accepted_frames,
            stack_rejects: group.rejected_frames,
            quality_excluded: group.quality_excluded,
            missing_files: group.missing_files,
        },
        total_exposure_hours: group.total_exposure_seconds / 3600.0,
        calibration: calibration_brief(group),
        has_image: group.preview_url.is_some(),
        snr: group
            .snr
            .as_ref()
            .and_then(|snr| snr.analysis.as_ref())
            .map(|analysis| analysis.summary.clone()),
        note: group.resume_note.clone(),
        error: group.error.clone(),
    }
}

fn calibration_brief(group: &StackGroupStatus) -> CalibrationBrief {
    let applied = &group.calibration;
    CalibrationBrief {
        mode: applied.mode.as_str().to_string(),
        state: applied.state.clone(),
        sessions: applied
            .session_details
            .iter()
            .map(|session| session_masters(&session.masters_signature, session.lights))
            .collect(),
        warning: applied.warning.clone(),
    }
}

/// Read `bias=<file>;dark=<file>;dark_flat=none;flat=<file>`.
pub(super) fn session_masters(signature: &str, lights: usize) -> SessionMasters {
    let mut masters = SessionMasters {
        lights,
        bias: None,
        dark: None,
        dark_flat: None,
        flat: None,
    };
    for part in signature.split(';') {
        let Some((kind, file)) = part.split_once('=') else {
            continue;
        };
        let file = (file != "none" && !file.is_empty()).then(|| file.to_string());
        match kind {
            "bias" => masters.bias = file,
            "dark" => masters.dark = file,
            "dark_flat" => masters.dark_flat = file,
            "flat" => masters.flat = file,
            _ => {}
        }
    }
    masters
}

/// One frame of a stack: what happened to it and where it sat.
#[derive(Debug, Serialize)]
pub(super) struct FrameRow {
    pub image_id: i32,
    pub night: Option<String>,
    pub acquired_unix_seconds: Option<i64>,
    pub disposition: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality_score: Option<f64>,
    /// Where the frame lands on the reference, in reference pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shift_x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shift_y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation_deg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_rms_pixels: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_stars: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub noise_sigma: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalization_gain: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalization_offset: Option<f32>,
}

pub(super) fn frame_rows(
    group: &StackGroupStatus,
    acquired: &HashMap<i32, i64>,
    night_of: impl Fn(i64) -> String,
) -> Vec<FrameRow> {
    let mut rows: Vec<FrameRow> = group
        .frames
        .iter()
        .map(|frame| {
            let transform = frame.registered_mapping.as_ref().map(|map| map.transform());
            let at = acquired.get(&frame.image_id).copied();
            FrameRow {
                image_id: frame.image_id,
                night: at.map(&night_of),
                acquired_unix_seconds: at,
                disposition: frame.disposition.clone(),
                reason: frame.reason.clone(),
                quality_score: frame.quality_score,
                shift_x: transform.map(|t| t.translation_x),
                shift_y: transform.map(|t| t.translation_y),
                rotation_deg: transform.map(|t| t.rotation_radians.to_degrees()),
                registration_rms_pixels: frame.registration_rms_pixels,
                matched_stars: frame.matched_stars,
                weight: frame
                    .integration_weight
                    .as_ref()
                    .and_then(|weights| weights.first().copied()),
                noise_sigma: frame
                    .noise_sigma
                    .as_ref()
                    .and_then(|noise| noise.first().copied()),
                normalization_gain: frame.normalization_mean_gain,
                normalization_offset: frame.normalization_mean_offset,
            }
        })
        .collect();
    rows.sort_by_key(|row| (row.acquired_unix_seconds.unwrap_or(i64::MAX), row.image_id));
    rows
}

/// The imaging night a capture belongs to, for a catalog whose nights split
/// `boundary` seconds after midnight UTC.
pub(super) fn night_of(timestamp: i64, boundary: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp - boundary, 0)
        .map(|when| when.date_naive().to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// How one night's frames moved on the sky between exposures.
#[derive(Debug, Serialize, PartialEq)]
pub(super) struct NightDrift {
    pub night: String,
    pub frames: usize,
    /// The middle step between consecutive frames, in pixels.
    pub median_step_px: f64,
    /// From the first frame to the last.
    pub net_drift_px: f64,
    /// Net drift over the summed steps: near 1 when every step goes the
    /// same way, near 0 when steps go every which way, as dithers do.
    pub straightness: f64,
    /// Steps over 50 px (re-centring, a meridian flip), left out of the rest.
    pub jumps: usize,
    pub verdict: String,
}

/// A step larger than this is a re-centre or a flip, not drift or a dither.
const JUMP_PX: f64 = 50.0;

/// Per night, whether the frames were dithered or walked in one direction.
/// Frames a fixed sensor pattern walks along stack into streaks that
/// rejection cannot remove ("walking noise"); dithered frames spread it.
pub(super) fn drift_by_night(rows: &[FrameRow]) -> Vec<NightDrift> {
    let mut nights: Vec<(String, Vec<(f64, f64)>)> = Vec::new();
    for row in rows {
        let (Some(night), Some(x), Some(y)) = (&row.night, row.shift_x, row.shift_y) else {
            continue;
        };
        match nights.last_mut() {
            Some((current, points)) if current == night => points.push((x, y)),
            _ => nights.push((night.clone(), vec![(x, y)])),
        }
    }
    nights
        .into_iter()
        .filter(|(_, points)| points.len() >= 3)
        .map(|(night, points)| {
            let steps: Vec<(f64, f64)> = points
                .windows(2)
                .map(|pair| (pair[1].0 - pair[0].0, pair[1].1 - pair[0].1))
                .collect();
            let jumps = steps
                .iter()
                .filter(|(dx, dy)| dx.hypot(*dy) > JUMP_PX)
                .count();
            let kept: Vec<(f64, f64)> = steps
                .into_iter()
                .filter(|(dx, dy)| dx.hypot(*dy) <= JUMP_PX)
                .collect();
            let mut lengths: Vec<f64> = kept.iter().map(|(dx, dy)| dx.hypot(*dy)).collect();
            lengths.sort_by(f64::total_cmp);
            let median_step_px = lengths.get(lengths.len() / 2).copied().unwrap_or(0.0);
            let path: f64 = lengths.iter().sum();
            let (net_x, net_y) = kept
                .iter()
                .fold((0.0, 0.0), |(x, y), (dx, dy)| (x + dx, y + dy));
            let net_drift_px = net_x.hypot(net_y);
            let straightness = if path > 0.0 { net_drift_px / path } else { 0.0 };
            let verdict = if kept.len() >= 3 && straightness >= 0.6 && median_step_px < 3.0 {
                format!(
                    "Not dithered: the frames walk {net_drift_px:.0} px in one direction, so \
                     anything fixed to the sensor stacks into streaks along it"
                )
            } else if median_step_px >= 3.0 && straightness < 0.5 {
                "Dithered: steps of a few pixels in changing directions".to_string()
            } else {
                "Unclear: small steps without one direction".to_string()
            };
            NightDrift {
                night,
                frames: points.len(),
                median_step_px,
                net_drift_px,
                straightness,
                jumps,
                verdict,
            }
        })
        .collect()
}

/// A part of the image, as fractions of its width and height from the top
/// left, so it means the same whatever size the image is served at.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
pub struct CropArgs {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// How to show the stack.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImageStretch {
    /// As the app shows it.
    #[default]
    Display,
    /// The background stretched hard: the darkest 2% to black, the 70th
    /// percentile to white. Stars burn out; gradients, vignetting, streaks
    /// and calibration patterns show.
    Background,
}

/// What the picture is, for the text that goes with it.
#[derive(Debug, Serialize)]
pub(super) struct ImageNote {
    pub source_width: u32,
    pub source_height: u32,
    pub crop: [u32; 4],
    pub shown_width: u32,
    pub shown_height: u32,
    pub stretch: &'static str,
}

/// Crop, shrink and (for `Background`) stretch a preview PNG, and encode
/// the result as PNG.
pub(super) fn render_png(
    bytes: &[u8],
    crop: Option<CropArgs>,
    max_size: u32,
    stretch: ImageStretch,
) -> Result<(Vec<u8>, ImageNote), String> {
    let image = image::load_from_memory(bytes)
        .map_err(|error| format!("reading the stack image: {error}"))?;
    let (width, height) = (image.width(), image.height());
    let [x, y, w, h] = match crop {
        None => [0, 0, width, height],
        Some(crop) => {
            let fraction = |value: f64| value.clamp(0.0, 1.0);
            let x = (fraction(crop.x) * width as f64) as u32;
            let y = (fraction(crop.y) * height as f64) as u32;
            let w =
                ((fraction(crop.width) * width as f64) as u32).clamp(1, width - x.min(width - 1));
            let h = ((fraction(crop.height) * height as f64) as u32)
                .clamp(1, height - y.min(height - 1));
            [x.min(width - 1), y.min(height - 1), w, h]
        }
    };
    let mut image = image.crop_imm(x, y, w, h);
    if w.max(h) > max_size {
        image = image.resize(max_size, max_size, image::imageops::FilterType::Triangle);
    }
    if stretch == ImageStretch::Background {
        image = stretch_background(image);
    }
    let note = ImageNote {
        source_width: width,
        source_height: height,
        crop: [x, y, w, h],
        shown_width: image.width(),
        shown_height: image.height(),
        stretch: match stretch {
            ImageStretch::Display => "display",
            ImageStretch::Background => "background",
        },
    };
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|error| format!("encoding the stack image: {error}"))?;
    Ok((png, note))
}

/// Map the 2nd percentile of brightness to black and the 70th to white, the
/// same for every channel so colour keeps its balance.
fn stretch_background(image: image::DynamicImage) -> image::DynamicImage {
    let mut rgb = image.to_rgb8();
    let mut levels: Vec<u8> = rgb
        .pixels()
        .map(|pixel| pixel.0.into_iter().max().unwrap_or(0))
        .collect();
    levels.sort_unstable();
    let at = |fraction: f64| levels[((levels.len() - 1) as f64 * fraction) as usize] as f64;
    let (low, high) = (at(0.02), at(0.70).max(at(0.02) + 1.0));
    for pixel in rgb.pixels_mut() {
        for channel in pixel.0.iter_mut() {
            *channel = (((*channel as f64 - low) / (high - low)) * 255.0).clamp(0.0, 255.0) as u8;
        }
    }
    let shown = image::DynamicImage::ImageRgb8(rgb);
    if image.color().has_color() {
        shown
    } else {
        image::DynamicImage::ImageLuma8(shown.to_luma8())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(night: &str, at: i64, x: f64, y: f64) -> FrameRow {
        FrameRow {
            image_id: at as i32,
            night: Some(night.into()),
            acquired_unix_seconds: Some(at),
            disposition: "accepted".into(),
            reason: None,
            quality_score: None,
            shift_x: Some(x),
            shift_y: Some(y),
            rotation_deg: Some(0.0),
            registration_rms_pixels: None,
            matched_stars: None,
            weight: None,
            noise_sigma: None,
            normalization_gain: None,
            normalization_offset: None,
        }
    }

    #[test]
    fn a_steady_walk_reads_as_undithered_and_a_scatter_as_dithered() {
        // The July Bubble nights: about a pixel a frame, one way, and one
        // re-centre in the middle.
        let mut rows: Vec<FrameRow> = (0..20)
            .map(|i| {
                row(
                    "2026-07-01",
                    i,
                    100.0 - i as f64 * 0.8,
                    -60.0 + i as f64 * 0.5,
                )
            })
            .collect();
        rows.push(row("2026-07-01", 20, 250.0, -60.0));
        rows.push(row("2026-07-01", 21, 249.2, -59.5));
        // A dithered night: five-pixel hops every which way.
        let hops = [
            (5.0, 0.0),
            (0.0, 5.0),
            (-5.0, 0.0),
            (0.0, -5.0),
            (4.0, 3.0),
            (-4.0, -3.0),
        ];
        let (mut x, mut y) = (0.0, 0.0);
        for (i, (dx, dy)) in hops.iter().cycle().take(12).enumerate() {
            x += dx;
            y += dy;
            rows.push(row("2026-07-02", 100 + i as i64, x, y));
        }
        let drift = drift_by_night(&rows);
        assert_eq!(drift.len(), 2);
        assert_eq!(drift[0].frames, 22);
        assert_eq!(drift[0].jumps, 1);
        assert!(drift[0].straightness > 0.95, "{:?}", drift[0]);
        assert!(
            drift[0].verdict.starts_with("Not dithered"),
            "{}",
            drift[0].verdict
        );
        assert!(drift[1].verdict.starts_with("Dithered"), "{:?}", drift[1]);
    }

    #[test]
    fn a_session_signature_names_its_masters() {
        let masters = session_masters(
            "bias=bias-4c.fits;dark=dark-bd.fits;dark_flat=none;flat=none",
            106,
        );
        assert_eq!(masters.lights, 106);
        assert_eq!(masters.bias.as_deref(), Some("bias-4c.fits"));
        assert_eq!(masters.dark.as_deref(), Some("dark-bd.fits"));
        assert_eq!(masters.flat, None);
        assert_eq!(masters.dark_flat, None);
    }

    #[test]
    fn a_crop_shrinks_and_the_background_stretch_spreads_the_dark_end() {
        let mut gray = image::GrayImage::new(400, 200);
        for (x, _, pixel) in gray.enumerate_pixels_mut() {
            pixel.0 = [20 + (x / 40) as u8];
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageLuma8(gray)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let crop = CropArgs {
            x: 0.5,
            y: 0.0,
            width: 0.5,
            height: 1.0,
        };
        let (shown, note) = render_png(&png, Some(crop), 100, ImageStretch::Background).unwrap();
        assert_eq!(note.crop, [200, 0, 200, 200]);
        assert_eq!((note.shown_width, note.shown_height), (100, 100));
        let shown = image::load_from_memory(&shown).unwrap().to_luma8();
        let (darkest, brightest) = shown.pixels().fold((255u8, 0u8), |(low, high), pixel| {
            (low.min(pixel.0[0]), high.max(pixel.0[0]))
        });
        assert_eq!(darkest, 0);
        assert_eq!(brightest, 255);
    }
}
