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
    let (black, white) = key.range;
    let span = (white - black).max(f32::MIN_POSITIVE);
    let unit = |value: f32| (value - black) / span;
    let mut values = vec![f32::NAN; width * height];
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
