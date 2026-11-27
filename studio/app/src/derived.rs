//! Planes Studio works out from the whole image while a person edits, for
//! what the canvas cannot draw from one pixel and the maps a stretch reads:
//! the luminance of red, green and blue stretched together, and the fine
//! blur that star-core compression measures detail against.
//!
//! A plane is worked out by a task on a thread of its own, from the store
//! at full resolution, with its progress shown in the status line. When it
//! is done the canvas draws from a source that adds it to the store's
//! channels, and its tiles load again. Until then the canvas draws without
//! it and says what it is waiting for.

use crate::image_steps::Region;
use crate::store_tiles::{BandState, StoreTiles};
use crate::studio_model::RegionReader;
use psf_studio_render::synth::{TileKey, TILE};
use psf_studio_render::{Pyramid, TileSource};
use seiza_compose::ReferenceRegion;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// What a plane holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DerivedKind {
    /// The luminance of three bands stretched together, red, green and
    /// blue in that order.
    Luminance,
    /// The fine blur of one band, or of the luminance of three, for
    /// star-core compression.
    Fine,
    /// The stars of one of the bands, `bands[n]`, made smaller: scaled by
    /// how much a one-pixel cross erosion lowers the brightest of the
    /// bands at each pixel, as Seiza's RGB star reduction does, so the
    /// bands' colors keep. A layer blends toward it by its reduction.
    ReducedStars(usize),
    /// The stars of red, green or blue (`pick`), stretched as Seiza's RGB
    /// star stretch does at `amount_milli` thousandths: each star sample
    /// over the range of the three channels with their stars, then a
    /// midtones curve. `bands` are red, green and blue as their star
    /// removal left them, then their stars. With `reduced`, the stretched
    /// stars are also made smaller in full, which a layer blends toward by
    /// its reduction.
    StretchedStars {
        pick: usize,
        amount_milli: u32,
        reduced: bool,
    },
    /// One channel of what a processing tool step in the Combine or Finish
    /// stage made, read from the file it wrote: its result, or the stars a
    /// star removal took out.
    Tool {
        step: uuid::Uuid,
        channel: u8,
        stars: bool,
    },
}

/// Which plane: what it holds, of which bands in which states, as the
/// stretch reads them (0 to 1 over the stretch's range).
#[derive(Clone, Debug, PartialEq)]
pub struct DerivedKey {
    pub kind: DerivedKind,
    /// The bands it is made from: one, or red, green and blue.
    pub bands: Vec<usize>,
    /// Each band's state.
    pub states: Vec<BandState>,
    /// The stretch's range, as the physical values that become 0 and 1.
    pub range: (f32, f32),
}

/// Where one band's samples come from in a state: its tile channel, and the
/// corrected channel blended in by an amount.
#[derive(Clone, Copy, Debug)]
pub struct BandRead {
    pub plane: usize,
    pub corrected: Option<(usize, f32)>,
}

/// A plane worked out from the whole image, with its pyramid of means.
pub struct DerivedPlane {
    pub key: DerivedKey,
    /// Each level's width, height and samples, full resolution first.
    levels: Vec<(usize, usize, Vec<f32>)>,
    /// The largest sample: what a canvas sample of 1 stands for.
    pub scale: f32,
}

impl DerivedPlane {
    /// The plane's samples over `rect` at full resolution, a missing one as
    /// 0, kept within 0 and the plane's scale as the canvas's tiles are.
    pub fn read(&self, rect: ReferenceRegion) -> Vec<f32> {
        let (width, height, samples) = &self.levels[0];
        let mut out = Vec::with_capacity(rect.width * rect.height);
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                let value = if x < *width && y < *height {
                    samples[y * width + x]
                } else {
                    f32::NAN
                };
                out.push(if value.is_finite() {
                    value.clamp(0.0, self.scale)
                } else {
                    0.0
                });
            }
        }
        out
    }

    /// One canvas tile of the plane, as half floats over its scale.
    fn tile(&self, key: TileKey, out: &mut [u16]) {
        let Some((width, height, samples)) = self.levels.get(key.level as usize) else {
            return;
        };
        let tile = TILE as usize;
        let (x0, y0) = (key.x as usize * tile, key.y as usize * tile);
        for y in 0..tile {
            for x in 0..tile {
                let (px, py) = (x0 + x, y0 + y);
                if px >= *width || py >= *height {
                    continue;
                }
                let value = samples[py * width + px];
                let value = if value.is_finite() {
                    (value / self.scale).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                out[y * tile + x] = half::f16::from_f32(value).to_bits();
            }
        }
    }
}

/// A store's channels, then planes worked out from it: what the canvas
/// draws once a plane is done.
pub struct DerivedSource {
    pub store: Arc<StoreTiles>,
    pub planes: Vec<Arc<DerivedPlane>>,
}

impl TileSource for DerivedSource {
    fn pyramid(&self) -> Pyramid {
        self.store.pyramid()
    }

    fn bands(&self) -> usize {
        self.store.bands() + self.planes.len()
    }

    fn tile(&self, key: TileKey) -> Vec<u16> {
        let plane = TILE as usize * TILE as usize;
        let mut out = self.store.tile(key);
        out.resize(plane * self.bands(), 0);
        let first = self.store.bands();
        for (index, derived) in self.planes.iter().enumerate() {
            let at = (first + index) * plane;
            derived.tile(key, &mut out[at..at + plane]);
        }
        out
    }
}

/// A plane being worked out.
pub struct DeriveTask {
    pub key: DerivedKey,
    /// The share done, in thousandths.
    pub progress: Arc<AtomicU32>,
    pub stop: Arc<AtomicBool>,
    pub done: Arc<Mutex<Option<Result<DerivedPlane, String>>>>,
    pub started: std::time::Instant,
}

impl DeriveTask {
    /// Works out the plane `key` names from `store`, each band read as
    /// `reads` gives it, on a thread of its own.
    pub fn start(store: Arc<StoreTiles>, key: DerivedKey, reads: Vec<BandRead>) -> Self {
        let progress = Arc::new(AtomicU32::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(Mutex::new(None));
        let task = Self {
            key: key.clone(),
            progress: progress.clone(),
            stop: stop.clone(),
            done: done.clone(),
            started: std::time::Instant::now(),
        };
        std::thread::Builder::new()
            .name("derive plane".into())
            .spawn(move || {
                let result = work_out(&store, &key, &reads, &progress, &stop);
                *done.lock().expect("derived plane") = Some(result);
            })
            .expect("spawning a task");
        task
    }

    /// What the task does, for the status line.
    pub fn describe(&self) -> &'static str {
        match self.key.kind {
            DerivedKind::Luminance => "Measuring the luminance of red, green and blue",
            DerivedKind::Fine => "Measuring star cores",
            DerivedKind::ReducedStars(_) => "Reducing stars",
            DerivedKind::StretchedStars { .. } => "Stretching stars",
            DerivedKind::Tool { .. } => "Reading a tool's result",
        }
    }

    /// The share done, from 0 to 1.
    pub fn share(&self) -> f32 {
        self.progress.load(Ordering::Relaxed) as f32 / 1000.0
    }
}

/// The plane `key` names, from the store at full resolution.
fn work_out(
    store: &StoreTiles,
    key: &DerivedKey,
    reads: &[BandRead],
    progress: &AtomicU32,
    stop: &AtomicBool,
) -> Result<DerivedPlane, String> {
    let level = store
        .full_level()
        .ok_or_else(|| "the store has no levels".to_string())?;
    let shape = level.shape();
    let (width, height) = (shape.width, shape.height);
    let channels = store.bands();
    if let DerivedKind::StretchedStars {
        pick,
        amount_milli,
        reduced,
    } = key.kind
    {
        return stretched_stars(
            store,
            key,
            reads,
            (pick, amount_milli, reduced),
            progress,
            stop,
        );
    }
    let (black, white) = key.range;
    let span = (white - black).max(f32::MIN_POSITIVE);
    let unit = |value: f32| (value - black) / span;
    let mut values = vec![f32::NAN; width * height];
    // For reduced stars: the brightest of the bands at each pixel, which
    // the erosion is measured on, while `values` holds the one reduced.
    let reduced = match key.kind {
        DerivedKind::ReducedStars(pick) if pick < reads.len() => Some(pick),
        DerivedKind::ReducedStars(_) => return Err("no such band to reduce".to_string()),
        DerivedKind::Tool { .. } => return Err("a tool's result is read from its file".to_string()),
        DerivedKind::StretchedStars { .. } => unreachable!("worked out above"),
        _ => None,
    };
    let mut brightest = if reduced.is_some() {
        vec![f32::NAN; width * height]
    } else {
        Vec::new()
    };
    let strip = 256.min(height.max(1));
    let mut y = 0;
    while y < height {
        if stop.load(Ordering::Relaxed) {
            return Err("it was stopped".to_string());
        }
        let rows = strip.min(height - y);
        let samples = level
            .read(Region::new(0, y, width, rows))
            .map_err(|error| error.to_string())?;
        let band = |read: &BandRead, pixel: usize| -> f32 {
            let at = pixel * channels;
            let mut value = samples[at + read.plane];
            if let Some((corrected, amount)) = read.corrected {
                value += (samples[at + corrected] - value) * amount;
            }
            unit(value)
        };
        for pixel in 0..width * rows {
            if let Some(pick) = reduced {
                // Stars are bright on black: a value below black is none.
                values[y * width + pixel] = band(&reads[pick], pixel).max(0.0);
                brightest[y * width + pixel] = reads
                    .iter()
                    .map(|read| band(read, pixel).max(0.0))
                    .fold(f32::NEG_INFINITY, f32::max);
                continue;
            }
            values[y * width + pixel] = match reads {
                [red, green, blue] => seiza_stretch::rgb_luminance(
                    band(red, pixel),
                    band(green, pixel),
                    band(blue, pixel),
                ),
                [one] => band(one, pixel),
                _ => f32::NAN,
            };
        }
        y += rows;
        // Reading is most of the work; the blur and the pyramid the rest.
        progress.store((800 * y / height) as u32, Ordering::Relaxed);
    }
    if key.kind == DerivedKind::Fine {
        values = seiza_stretch::fine_luminance(&values, width, height);
    }
    if reduced.is_some() {
        reduce_stars(&mut values, &brightest, width, height);
    }
    progress.store(900, Ordering::Relaxed);
    let scale = values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .fold(f32::MIN_POSITIVE, f32::max);
    let mut levels = vec![(width, height, values)];
    while levels.len() < store.pyramid().levels as usize {
        let (w, h, last) = levels.last().expect("a level");
        levels.push(halve(last, *w, *h));
    }
    progress.store(1000, Ordering::Relaxed);
    Ok(DerivedPlane {
        key: key.clone(),
        levels,
        scale,
    })
}

impl DerivedPlane {
    /// A plane of `samples`, `width` by `height`, with `levels` levels of
    /// means.
    pub fn of_samples(
        key: DerivedKey,
        width: usize,
        height: usize,
        samples: Vec<f32>,
        levels: usize,
    ) -> Self {
        let scale = samples
            .iter()
            .copied()
            .filter(|value| value.is_finite())
            .fold(f32::MIN_POSITIVE, f32::max);
        let mut pyramid = vec![(width, height, samples)];
        while pyramid.len() < levels.max(1) {
            let (w, h, last) = pyramid.last().expect("a level");
            pyramid.push(halve(last, *w, *h));
        }
        Self {
            key,
            levels: pyramid,
            scale,
        }
    }
}

/// One band's stars, `stars`, made smaller: each sample scaled by how much
/// Seiza's one-pixel cross erosion lowers `brightest`, the brightest of the
/// bands reduced together, at that pixel.
/// The stretched stars `key` names, from the store at full resolution: two
/// passes, the first for the range of red, green and blue with their stars,
/// the second for the stars stretched over it, as Seiza's RGB star stretch.
fn stretched_stars(
    store: &StoreTiles,
    key: &DerivedKey,
    reads: &[BandRead],
    (pick, amount_milli, reduced): (usize, u32, bool),
    progress: &AtomicU32,
    stop: &AtomicBool,
) -> Result<DerivedPlane, String> {
    let [red, green, blue, red_stars, green_stars, blue_stars] = reads else {
        return Err("stretched stars read red, green and blue and their stars".to_string());
    };
    let (starless, stars) = ([red, green, blue], [red_stars, green_stars, blue_stars]);
    if pick > 2 {
        return Err("no such channel of stars".to_string());
    }
    let level = store
        .full_level()
        .ok_or_else(|| "the store has no levels".to_string())?;
    let shape = level.shape();
    let (width, height) = (shape.width, shape.height);
    let channels = store.bands();
    let strip = 256.min(height.max(1));
    // Each pass reads the store in strips and hands each pixel's three
    // channels with their stars, and their stars, to `each`.
    let pass = |each: &mut dyn FnMut(usize, [f32; 3], [f32; 3]), part: u32| -> Result<(), String> {
        let mut y = 0;
        while y < height {
            if stop.load(Ordering::Relaxed) {
                return Err("it was stopped".to_string());
            }
            let rows = strip.min(height - y);
            let samples = level
                .read(Region::new(0, y, width, rows))
                .map_err(|error| error.to_string())?;
            let value = |read: &BandRead, pixel: usize| -> f32 {
                let at = pixel * channels;
                let mut value = samples[at + read.plane];
                if let Some((corrected, amount)) = read.corrected {
                    value += (samples[at + corrected] - value) * amount;
                }
                value
            };
            for pixel in 0..width * rows {
                let star = stars.map(|read| value(read, pixel));
                let with =
                    std::array::from_fn(|channel| value(starless[channel], pixel) + star[channel]);
                each(y * width + pixel, with, star);
            }
            y += rows;
            progress.store(part * 400 + (400 * y / height) as u32, Ordering::Relaxed);
        }
        Ok(())
    };
    // The range of the three channels with their stars.
    let (mut floor, mut peak) = (f32::INFINITY, f32::NEG_INFINITY);
    pass(
        &mut |_, with, _| {
            if with.iter().all(|value| value.is_finite()) {
                for value in with {
                    floor = floor.min(value);
                    peak = peak.max(value);
                }
            }
        },
        0,
    )?;
    if !peak.is_finite() {
        return Err("red, green and blue have no samples".to_string());
    }
    // As Seiza's RGB star stretch: a range of at least 1.
    let scale = (f64::from(peak) - f64::from(floor.min(0.0))).max(1.0);
    let gain = 3.0_f64.powf(f64::from(amount_milli) / 1000.0);
    let stretch = |star: f32| {
        let x = (f64::from(star) / scale).clamp(0.0, 1.0);
        (gain * x / (1.0 + (gain - 1.0) * x)) as f32
    };
    let mut values = vec![f32::NAN; width * height];
    let mut brightest = if reduced {
        vec![f32::NAN; width * height]
    } else {
        Vec::new()
    };
    pass(
        &mut |at, with, star| {
            if with.iter().any(|value| !value.is_finite()) {
                return;
            }
            let stretched = star.map(stretch);
            values[at] = stretched[pick];
            if reduced {
                brightest[at] = stretched.into_iter().fold(0.0, f32::max);
            }
        },
        1,
    )?;
    if reduced {
        reduce_stars(&mut values, &brightest, width, height);
    }
    progress.store(900, Ordering::Relaxed);
    let mut levels = vec![(width, height, values)];
    while levels.len() < store.pyramid().levels as usize {
        let (w, h, last) = levels.last().expect("a level");
        levels.push(halve(last, *w, *h));
    }
    progress.store(1000, Ordering::Relaxed);
    Ok(DerivedPlane {
        key: key.clone(),
        levels,
        // Stretched stars run from 0 to 1.
        scale: 1.0,
    })
}

fn reduce_stars(stars: &mut [f32], brightest: &[f32], width: usize, height: usize) {
    let eroded = seiza_compose::cross_erosion(brightest, width, height);
    for ((value, before), after) in stars.iter_mut().zip(brightest).zip(&eroded) {
        *value = if *before > 0.0 {
            *value * after / before
        } else {
            0.0
        };
    }
}

/// A level of half the width and height: each sample the mean of the
/// finite ones of the two by two it covers, as the store's pyramid is.
fn halve(samples: &[f32], width: usize, height: usize) -> (usize, usize, Vec<f32>) {
    let (half_width, half_height) = (width.div_ceil(2), height.div_ceil(2));
    let mut out = vec![f32::NAN; half_width * half_height];
    for y in 0..half_height {
        for x in 0..half_width {
            let (mut sum, mut count) = (0.0_f32, 0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = (2 * x + dx, 2 * y + dy);
                if sx < width && sy < height {
                    let value = samples[sy * width + sx];
                    if value.is_finite() {
                        sum += value;
                        count += 1;
                    }
                }
            }
            if count > 0 {
                out[y * half_width + x] = sum / count as f32;
            }
        }
    }
    (half_width, half_height, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduced_stars_shrink_and_keep_their_colors() {
        // A star three pixels wide in red, half as bright in green.
        let red = [
            0.0, 0.0, 0.0, 0.0, 0.0, //
            0.0, 0.4, 0.8, 0.4, 0.0, //
            0.0, 0.0, 0.0, 0.0, 0.0,
        ];
        let green: Vec<f32> = red.iter().map(|value| value / 2.0).collect();
        let brightest = red;
        let mut reduced_red = red;
        let mut reduced_green = green.clone();
        reduce_stars(&mut reduced_red, &brightest, 5, 3);
        reduce_stars(&mut reduced_green, &brightest, 5, 3);
        // Inside, every sample takes its darkest neighbour's level: the
        // black above and below.
        assert_eq!(reduced_red[6], 0.0);
        assert_eq!(reduced_red[7], 0.0);
        // The colours keep: green stays half of red wherever red is lit.
        for at in 0..red.len() {
            assert!((reduced_green[at] - reduced_red[at] / 2.0).abs() < 1e-6);
        }
        // A wide star: a three by three core of 1 in a ring of 0.5. Its
        // centre keeps its level, and the core's edge falls to the ring's.
        let mut wide = vec![0.0_f32; 49];
        for y in 1..6 {
            for x in 1..6 {
                wide[y * 7 + x] = if (2..5).contains(&x) && (2..5).contains(&y) {
                    1.0
                } else {
                    0.5
                };
            }
        }
        let mut stars = wide.clone();
        reduce_stars(&mut stars, &wide, 7, 7);
        assert_eq!(stars[3 * 7 + 3], 1.0);
        assert_eq!(stars[3 * 7 + 2], 0.5);
        assert_eq!(stars[7 + 3], 0.0);
    }

    #[test]
    fn a_level_is_the_mean_of_the_two_by_two_beneath() {
        let samples = [1.0, 3.0, 5.0, 2.0, 4.0, 6.0, f32::NAN, f32::NAN, 9.0];
        let (width, height, half) = halve(&samples, 3, 3);
        assert_eq!((width, height), (2, 2));
        assert_eq!(half[0], 2.5);
        assert_eq!(half[1], 5.5);
        assert!(half[2].is_nan());
        assert_eq!(half[3], 9.0);
    }
}
