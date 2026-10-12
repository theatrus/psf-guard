//! Pictures of raw calibration frames.
//!
//! Each kind gets the stretch that shows what can go wrong with it. A flat
//! is divided by its own median, per CFA site on a colour sensor, and shown
//! as a window around 1: vignetting, dust and droplets read as shades of a
//! uniform grey. A bias, dark or dark-flat is shown as a window of a few
//! noise sigmas around its median, so banding shows as stripes and hot
//! pixels as white points.
//!
//! A flat can also be shown against its session: binned, divided by its
//! median, then by the median of every flat of the session treated the same
//! way. What all the flats share, dust and vignetting, divides out; what
//! changes between them, such as drying dew, stays.
//!
//! Frames are read through [`crate::image_io`] and reduced by block means,
//! which keeps a hot pixel visible at any size and costs one pass.

use crate::calibration::CalibrationKind;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Bump when what a preview shows changes, so cached previews are redrawn.
pub const RENDER_VERSION: u32 = 1;
/// The largest side of a session comparison before any further shrinking.
const SESSION_SIDE: usize = 1024;
/// At most this many flats make a session's median, evenly spaced.
const SESSION_MAX_FRAMES: usize = 48;
/// Samples a level or spread is estimated from, at most.
const STATISTIC_SAMPLES: usize = 1 << 20;
const MEDIAN_MAGIC: &[u8; 8] = b"PSFGMED1";

/// What a preview shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameView {
    /// The frame itself.
    #[default]
    Frame,
    /// A flat divided by its session's median flat.
    Session,
}

/// How wide a window of values the grey scale spans.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameStretch {
    Wide,
    #[default]
    Normal,
    Tight,
}

/// How large a preview is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameSize {
    /// For a strip of frames.
    Thumb,
    #[default]
    Screen,
    Original,
}

impl FrameSize {
    pub fn max_side(self) -> Option<usize> {
        match self {
            Self::Thumb => Some(256),
            Self::Screen => Some(1200),
            Self::Original => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Thumb => "thumb",
            Self::Screen => "screen",
            Self::Original => "original",
        }
    }
}

impl FrameView {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Frame => "frame",
            Self::Session => "session",
        }
    }
}

impl FrameStretch {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wide => "wide",
            Self::Normal => "normal",
            Self::Tight => "tight",
        }
    }

    /// Half the window, as a share of the level for a flat or its session
    /// ratio, or in noise sigmas for the rest.
    fn half_window(self, kind: CalibrationKind, view: FrameView) -> f64 {
        let [wide, normal, tight] = match (kind, view) {
            (CalibrationKind::Flat, FrameView::Session) => [0.10, 0.03, 0.01],
            (CalibrationKind::Flat, FrameView::Frame) => [0.50, 0.15, 0.03],
            _ => [30.0, 6.0, 2.0],
        };
        match self {
            Self::Wide => wide,
            Self::Normal => normal,
            Self::Tight => tight,
        }
    }
}

/// The flats a comparison divides by.
#[derive(Debug, Clone)]
pub struct SessionSource {
    /// Every flat of the session, the shown one among them.
    pub members: Vec<PathBuf>,
    /// Where their median is kept once worked out.
    pub median_path: PathBuf,
}

/// One more size drawn from the same read.
#[derive(Debug, Clone)]
pub struct ExtraOutput {
    pub path: PathBuf,
    pub max_side: Option<usize>,
}

/// What to draw.
#[derive(Debug, Clone)]
pub struct RenderSpec {
    pub kind: CalibrationKind,
    pub session: Option<SessionSource>,
    pub stretch: FrameStretch,
    pub max_side: Option<usize>,
    /// Other sizes of the frame view to write while the frame is in memory,
    /// best effort: reading the frame costs far more than drawing it.
    pub also: Vec<ExtraOutput>,
}

impl RenderSpec {
    fn view(&self) -> FrameView {
        if self.session.is_some() {
            FrameView::Session
        } else {
            FrameView::Frame
        }
    }
}

/// A mono plane of display values.
#[derive(Debug, Clone, PartialEq)]
struct Plane {
    width: usize,
    height: usize,
    data: Vec<f32>,
}

/// Draw `source` as `spec` asks and write a PNG to `output`.
pub fn render(source: &Path, output: &Path, spec: &RenderSpec) -> Result<()> {
    let frame = crate::image_io::open_linear_frame(source)
        .with_context(|| format!("reading {}", source.display()))?;
    let image = &frame.image;
    anyhow::ensure!(
        image.width > 0 && image.height > 0 && image.channels > 0,
        "the frame has no pixels"
    );
    let cfa = frame.bayer.is_some() && image.channels == 1;
    let view = spec.view();
    let plane = match &spec.session {
        None => {
            let gains = level_gains(image, cfa, spec.kind == CalibrationKind::Flat);
            let factor = bin_factor(image.width, image.height, spec.max_side, cfa, false);
            for extra in &spec.also {
                if extra.path.exists() {
                    continue;
                }
                let factor = bin_factor(image.width, image.height, extra.max_side, cfa, false);
                let plane = bin(image, factor, &gains);
                let written = write_png(
                    &stretched(&plane, spec.kind, view, spec.stretch),
                    &extra.path,
                );
                if let Err(error) = written {
                    tracing::debug!("could not write {}: {error:#}", extra.path.display());
                }
            }
            bin(image, factor, &gains)
        }
        Some(session) => {
            let factor = session_factor(image.width, image.height, cfa);
            let median = session_median(session, factor, cfa, (image.width, image.height))?;
            let own = normalized(bin(image, factor, &[1.0]));
            anyhow::ensure!(
                own.width == median.width && own.height == median.height,
                "the frame is not the size of its session"
            );
            let ratio = Plane {
                width: own.width,
                height: own.height,
                data: own
                    .data
                    .iter()
                    .zip(&median.data)
                    .map(|(value, typical)| {
                        if *typical > 0.0 {
                            value / typical
                        } else {
                            f32::NAN
                        }
                    })
                    .collect(),
            };
            shrink(ratio, spec.max_side)
        }
    };
    let image = stretched(&plane, spec.kind, view, spec.stretch);
    image
        .save_with_format(output, image::ImageFormat::Png)
        .with_context(|| format!("writing {}", output.display()))
}

/// What each sample is multiplied by before binning: one over its CFA
/// site's or channel's median for a flat, so the picture is the response
/// alone; one for anything else.
fn level_gains(image: &seiza_stacking::LinearImage, cfa: bool, flat: bool) -> Vec<f32> {
    let sites = if cfa { 4 } else { image.channels };
    if !flat {
        return vec![1.0; sites];
    }
    (0..sites)
        .map(|site| {
            let level = median(site_samples(image, cfa, site));
            if level.is_finite() && level > 0.0 {
                1.0 / level
            } else {
                1.0
            }
        })
        .collect()
}

/// Up to [`STATISTIC_SAMPLES`] finite samples of one CFA site or channel.
fn site_samples(image: &seiza_stacking::LinearImage, cfa: bool, site: usize) -> Vec<f32> {
    let (width, height, channels) = (image.width, image.height, image.channels);
    let count = if cfa {
        width * height / 4
    } else {
        width * height
    };
    let step = (count / STATISTIC_SAMPLES).max(1);
    let mut samples = Vec::with_capacity(count / step + 1);
    if cfa {
        let (column, row) = (site & 1, site >> 1);
        let mut index = 0usize;
        for y in (row..height).step_by(2) {
            for x in (column..width).step_by(2) {
                if index.is_multiple_of(step) {
                    samples.push(image.data[y * width + x]);
                }
                index += 1;
            }
        }
    } else {
        samples.extend(
            image
                .data
                .iter()
                .skip(site)
                .step_by(channels * step)
                .copied(),
        );
    }
    samples.retain(|value| value.is_finite());
    samples
}

/// The block size that brings the larger side within `max_side`. A colour
/// sensor's blocks cover whole 2x2 cells once they bin at all, and a
/// session comparison always bins them so.
fn bin_factor(
    width: usize,
    height: usize,
    max_side: Option<usize>,
    cfa: bool,
    whole_cells: bool,
) -> usize {
    let side = width.max(height);
    let mut factor = max_side.map_or(1, |max| side.div_ceil(max.max(1)).max(1));
    if cfa && (factor > 1 || whole_cells) && factor % 2 == 1 {
        factor += 1;
    }
    factor
}

fn session_factor(width: usize, height: usize, cfa: bool) -> usize {
    bin_factor(width, height, Some(SESSION_SIDE), cfa, true)
}

/// Block means of `image`, each sample times its site's or channel's gain.
/// A block with no finite sample is NaN.
fn bin(image: &seiza_stacking::LinearImage, factor: usize, gains: &[f32]) -> Plane {
    let (width, height, channels) = (image.width, image.height, image.channels);
    let factor = factor.max(1);
    let out_width = width.div_ceil(factor);
    let out_height = height.div_ceil(factor);
    let cfa = gains.len() == 4 && channels == 1;
    let gain = |x: usize, y: usize, channel: usize| -> f32 {
        if cfa {
            gains[(y & 1) * 2 + (x & 1)]
        } else {
            *gains.get(channel).or(gains.first()).unwrap_or(&1.0)
        }
    };
    let mut sums = vec![0f64; out_width];
    let mut counts = vec![0u32; out_width];
    let mut data = Vec::with_capacity(out_width * out_height);
    for out_y in 0..out_height {
        sums.iter_mut().for_each(|sum| *sum = 0.0);
        counts.iter_mut().for_each(|count| *count = 0);
        for y in out_y * factor..((out_y + 1) * factor).min(height) {
            let row = &image.data[y * width * channels..(y + 1) * width * channels];
            for x in 0..width {
                let out_x = x / factor;
                for channel in 0..channels {
                    let value = row[x * channels + channel];
                    if value.is_finite() {
                        sums[out_x] += f64::from(value * gain(x, y, channel));
                        counts[out_x] += 1;
                    }
                }
            }
        }
        data.extend(sums.iter().zip(&counts).map(|(sum, count)| {
            if *count > 0 {
                (*sum / f64::from(*count)) as f32
            } else {
                f32::NAN
            }
        }));
    }
    Plane {
        width: out_width,
        height: out_height,
        data,
    }
}

/// Block means of a plane, to bring it within `max_side`.
fn shrink(plane: Plane, max_side: Option<usize>) -> Plane {
    let factor = bin_factor(plane.width, plane.height, max_side, false, false);
    if factor == 1 {
        return plane;
    }
    let image = seiza_stacking::LinearImage {
        width: plane.width,
        height: plane.height,
        channels: 1,
        data: plane.data,
    };
    bin(&image, factor, &[1.0])
}

/// A plane divided by its own median.
fn normalized(mut plane: Plane) -> Plane {
    let level = median(sampled(&plane.data));
    if level.is_finite() && level != 0.0 {
        plane.data.iter_mut().for_each(|value| *value /= level);
    }
    plane
}

fn sampled(data: &[f32]) -> Vec<f32> {
    let step = (data.len() / STATISTIC_SAMPLES).max(1);
    data.iter()
        .step_by(step)
        .copied()
        .filter(|value| value.is_finite())
        .collect()
}

/// The median of some samples; NaN when there are none.
fn median(mut samples: Vec<f32>) -> f32 {
    if samples.is_empty() {
        return f32::NAN;
    }
    let middle = samples.len() / 2;
    let (_, value, _) = samples.select_nth_unstable_by(middle, f32::total_cmp);
    *value
}

/// The plane as grey levels: a window around its median, as wide as the
/// kind, view and stretch call for.
fn stretched(
    plane: &Plane,
    kind: CalibrationKind,
    view: FrameView,
    stretch: FrameStretch,
) -> image::GrayImage {
    let samples = sampled(&plane.data);
    let center = f64::from(median(samples.clone()));
    let half = if kind == CalibrationKind::Flat {
        stretch.half_window(kind, view) * center.abs().max(f64::MIN_POSITIVE)
    } else {
        // Normal-equivalent median absolute deviation.
        let spread = f64::from(median(
            samples
                .iter()
                .map(|value| (f64::from(*value) - center).abs() as f32)
                .collect(),
        )) * 1.4826;
        let sigma = if spread.is_finite() && spread > 0.0 {
            spread
        } else {
            (center.abs() * 0.01).max(1.0)
        };
        stretch.half_window(kind, view) * sigma
    };
    let (low, span) = if center.is_finite() && half.is_finite() && half > 0.0 {
        (center - half, 2.0 * half)
    } else {
        (0.0, 1.0)
    };
    let pixels = plane
        .data
        .iter()
        .map(|value| {
            if value.is_finite() {
                (((f64::from(*value) - low) / span).clamp(0.0, 1.0) * 255.0).round() as u8
            } else {
                0
            }
        })
        .collect();
    image::GrayImage::from_raw(plane.width as u32, plane.height as u32, pixels)
        .expect("one grey level per plane sample")
}

fn write_png(image: &image::GrayImage, path: &Path) -> Result<()> {
    let temporary = crate::server::preview_queue::temp_path(path);
    let written = image
        .save_with_format(&temporary, image::ImageFormat::Png)
        .map_err(anyhow::Error::from)
        .and_then(|()| std::fs::rename(&temporary, path).map_err(anyhow::Error::from));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// One build of a session's median at a time per file, so frames of one
/// session asked for together share the work.
fn median_lock(path: &Path) -> Arc<Mutex<()>> {
    static LOCKS: Mutex<Option<HashMap<PathBuf, Arc<Mutex<()>>>>> = Mutex::new(None);
    let mut locks = LOCKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let locks = locks.get_or_insert_with(HashMap::new);
    // Forget locks no build holds, so the map stays small.
    locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    Arc::clone(locks.entry(path.to_path_buf()).or_default())
}

/// The session's median flat, binned by `factor` and each flat divided by
/// its own median first; read back when kept, worked out and kept when not.
fn session_median(
    session: &SessionSource,
    factor: usize,
    cfa: bool,
    size: (usize, usize),
) -> Result<Plane> {
    let lock = median_lock(&session.median_path);
    let _held = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Ok(plane) = read_median(&session.median_path) {
        return Ok(plane);
    }
    let members = spaced(&session.members, SESSION_MAX_FRAMES);
    let mut planes = Vec::with_capacity(members.len());
    for member in members {
        let frame = match crate::image_io::open_linear_frame(member) {
            Ok(frame) => frame,
            Err(error) => {
                tracing::debug!("session median: skipping {}: {error}", member.display());
                continue;
            }
        };
        let member_cfa = frame.bayer.is_some() && frame.image.channels == 1;
        if (frame.image.width, frame.image.height) != size || member_cfa != cfa {
            continue;
        }
        planes.push(normalized(bin(&frame.image, factor, &[1.0])));
    }
    anyhow::ensure!(
        planes.len() >= crate::calibration::browse::MIN_COMPARE_FLATS,
        "only {} flats of the session could be read",
        planes.len()
    );
    let median = median_of(&planes);
    write_median(&session.median_path, &median)?;
    Ok(median)
}

/// At most `limit` of `items`, evenly spaced, the first and last among them.
fn spaced<T>(items: &[T], limit: usize) -> Vec<&T> {
    if items.len() <= limit || limit < 2 {
        return items.iter().take(limit).collect();
    }
    (0..limit)
        .map(|index| &items[index * (items.len() - 1) / (limit - 1)])
        .collect()
}

/// The per-pixel median of planes of one size.
fn median_of(planes: &[Plane]) -> Plane {
    let (width, height) = (planes[0].width, planes[0].height);
    let mut values = Vec::with_capacity(planes.len());
    let data = (0..width * height)
        .map(|index| {
            values.clear();
            values.extend(
                planes
                    .iter()
                    .map(|plane| plane.data[index])
                    .filter(|value| value.is_finite()),
            );
            median(std::mem::take(&mut values))
        })
        .collect();
    Plane {
        width,
        height,
        data,
    }
}

fn write_median(path: &Path, plane: &Plane) -> Result<()> {
    let temporary = crate::server::preview_queue::temp_path(path);
    let written = (|| -> Result<()> {
        let mut file = std::io::BufWriter::new(std::fs::File::create(&temporary)?);
        file.write_all(MEDIAN_MAGIC)?;
        file.write_all(&(plane.width as u32).to_le_bytes())?;
        file.write_all(&(plane.height as u32).to_le_bytes())?;
        for value in &plane.data {
            file.write_all(&value.to_le_bytes())?;
        }
        file.into_inner().map_err(|error| error.into_error())?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

fn read_median(path: &Path) -> Result<Plane> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() >= 16 && &bytes[..8] == MEDIAN_MAGIC,
        "not a session median"
    );
    let width = u32::from_le_bytes(bytes[8..12].try_into()?) as usize;
    let height = u32::from_le_bytes(bytes[12..16].try_into()?) as usize;
    anyhow::ensure!(
        bytes.len() == 16 + width * height * 4,
        "a session median of the wrong length"
    );
    let data = bytes[16..]
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("four bytes")))
        .collect();
    Ok(Plane {
        width,
        height,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(
        width: usize,
        height: usize,
        value: impl Fn(usize, usize) -> f32,
    ) -> seiza_stacking::LinearImage {
        seiza_stacking::LinearImage {
            width,
            height,
            channels: 1,
            data: (0..width * height)
                .map(|index| value(index % width, index / width))
                .collect(),
        }
    }

    #[test]
    fn block_means_skip_missing_samples_and_keep_partial_edges() {
        let source = image(5, 3, |x, y| {
            if (x, y) == (0, 0) {
                f32::NAN
            } else {
                (x + y) as f32
            }
        });
        let plane = bin(&source, 2, &[1.0]);
        assert_eq!((plane.width, plane.height), (3, 2));
        // The first block has three finite samples: 1, 1 and 2.
        assert!((plane.data[0] - 4.0 / 3.0).abs() < 1e-6);
        // The last column of blocks holds one column of samples.
        assert!((plane.data[2] - 4.5).abs() < 1e-6);
        assert!((plane.data[5] - 6.0).abs() < 1e-6);
    }

    #[test]
    fn a_colour_flat_loses_its_bayer_pattern() {
        // Red sites read half the green ones; the response is flat.
        let source = image(8, 8, |x, y| {
            if x % 2 == 0 && y % 2 == 0 {
                1000.0
            } else {
                2000.0
            }
        });
        let gains = level_gains(&source, true, true);
        let plane = bin(&source, 1, &gains);
        assert!(plane.data.iter().all(|value| (value - 1.0).abs() < 1e-6));
        // Binning keeps whole cells, so a dark's pattern averages out too.
        assert_eq!(bin_factor(9, 4, Some(4), true, false), 4);
        assert_eq!(bin_factor(9, 4, Some(9), true, false), 1);
        assert_eq!(bin_factor(9, 4, Some(9), true, true), 2);
    }

    #[test]
    fn a_hot_pixel_stays_white_and_the_noise_grey() {
        let mut source = image(64, 64, |x, y| {
            let noise = ((x * 7_919 + y * 104_729) as u32).wrapping_mul(2_654_435_761);
            500.0 + ((noise >> 16) % 9) as f32 - 4.0
        });
        source.data[64 * 10 + 10] = 60_000.0;
        let plane = bin(&source, 1, &[1.0]);
        let picture = stretched(
            &plane,
            CalibrationKind::Dark,
            FrameView::Frame,
            FrameStretch::Normal,
        );
        assert_eq!(picture.get_pixel(10, 10)[0], 255);
        let middle = picture
            .as_raw()
            .iter()
            .filter(|level| (60..=200).contains(*level))
            .count();
        assert!(middle > 64 * 64 / 2, "most pixels sit mid-grey: {middle}");
    }

    #[test]
    fn a_session_ratio_shows_a_spot_one_flat_has_and_hides_the_dust_all_share() {
        let directory = tempfile::tempdir().unwrap();
        let dust = |x: usize, y: usize| (x as f32 - 16.0).hypot(y as f32 - 16.0) < 4.0;
        let droplet = |x: usize, y: usize| (x as f32 - 44.0).hypot(y as f32 - 40.0) < 4.0;
        let mut members = Vec::new();
        for index in 0..4 {
            let path = directory.path().join(format!("flat-{index}.fits"));
            let level = 20_000.0 + index as f32 * 500.0;
            let data: Vec<f32> = (0..64 * 64)
                .map(|i| {
                    let (x, y) = (i % 64, i / 64);
                    let mut value = level * (1.0 - 0.002 * x as f32);
                    if dust(x, y) {
                        value *= 0.9;
                    }
                    if index == 2 && droplet(x, y) {
                        value *= 0.8;
                    }
                    value
                })
                .collect();
            let image = seiza_stacking::LinearImage::new(64, 64, 1, data).unwrap();
            seiza_stacking::write_processed_image_fits_f32(&path, &image, &[], &[]).unwrap();
            members.push(path);
        }
        let median_path = directory.path().join("session.median");
        let spec = || RenderSpec {
            kind: CalibrationKind::Flat,
            session: Some(SessionSource {
                members: members.clone(),
                median_path: median_path.clone(),
            }),
            stretch: FrameStretch::Normal,
            max_side: None,
            also: Vec::new(),
        };
        let output = directory.path().join("ratio.png");
        render(&members[2], &output, &spec()).unwrap();
        assert!(
            median_path.is_file(),
            "the median is kept for the next frame"
        );
        let ratio = image::open(&output).unwrap().to_luma8();
        let grey = ratio.get_pixel(32, 20)[0];
        assert!((110..=145).contains(&grey), "flat field reads grey: {grey}");
        assert!(
            ratio.get_pixel(44, 40)[0] < 20,
            "the droplet stays: {}",
            ratio.get_pixel(44, 40)[0]
        );
        assert!(
            ratio.get_pixel(16, 16)[0].abs_diff(grey) < 10,
            "the dust divides out: {}",
            ratio.get_pixel(16, 16)[0]
        );

        // Shown alone, the same flat keeps its dust.
        let alone = directory.path().join("alone.png");
        let mut frame = spec();
        frame.session = None;
        frame.stretch = FrameStretch::Tight;
        render(&members[2], &alone, &frame).unwrap();
        let alone = image::open(&alone).unwrap().to_luma8();
        assert!(
            alone.get_pixel(16, 16)[0] < 20,
            "dust shows in the frame view"
        );

        // The kept median is read back, not built again.
        let built = std::fs::metadata(&median_path).unwrap().modified().unwrap();
        render(&members[0], &output, &spec()).unwrap();
        assert_eq!(
            std::fs::metadata(&median_path).unwrap().modified().unwrap(),
            built
        );
        assert_eq!(read_median(&median_path).unwrap().width, 64);
    }

    #[test]
    fn extra_sizes_come_from_the_same_read() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("dark.fits");
        let image = seiza_stacking::LinearImage::new(
            600,
            300,
            1,
            (0..600 * 300)
                .map(|index| 500.0 + (index % 7) as f32)
                .collect(),
        )
        .unwrap();
        seiza_stacking::write_processed_image_fits_f32(&path, &image, &[], &[]).unwrap();
        let thumb = directory.path().join("thumb.png");
        let spec = RenderSpec {
            kind: CalibrationKind::Dark,
            session: None,
            stretch: FrameStretch::Normal,
            max_side: Some(1200),
            also: vec![ExtraOutput {
                path: thumb.clone(),
                max_side: Some(256),
            }],
        };
        let screen = directory.path().join("screen.png");
        render(&path, &screen, &spec).unwrap();
        assert_eq!(image::image_dimensions(&screen).unwrap(), (600, 300));
        assert_eq!(image::image_dimensions(&thumb).unwrap(), (200, 100));
    }

    #[test]
    fn spaced_keeps_the_ends() {
        let items: Vec<usize> = (0..10).collect();
        assert_eq!(spaced(&items, 4), vec![&0, &3, &6, &9]);
        assert_eq!(spaced(&items, 20).len(), 10);
    }
}
