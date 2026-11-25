//! A tile store on disk as the canvas's source of tiles, and the synthetic
//! document written out as a store to try it with.

use crate::image_steps::{Domain, ImageShape, Region};
use crate::studio_model::{
    MemoryBudget, RegionReader, SampleFormat, StoreLevel, StoreOptions, StudioError, TileStore,
};
use seiza_stretch::{
    ResolvedSampleDomain, SampleDomain, SampleNormalization, StretchAnalysis, StretchConfig,
    StretchParams,
};
use std::path::Path;
use std::sync::Arc;
use studio_render_spike::synth::{generate_tile, TileKey, BANDS, DOC_HEIGHT, DOC_WIDTH, TILE};
use studio_render_spike::{Pyramid, TileSource, STRETCH_TABLE_WIDTH};

/// A document read from a tile store.
pub struct StoreTiles {
    levels: Vec<StoreLevel>,
    pyramid: Pyramid,
    bands: usize,
    /// What each band's samples are divided by on the way to the canvas.
    scales: Vec<f32>,
}

/// What a band's stretch is resolved from: Seiza's analysis of a sample of
/// the band at full resolution.
///
/// The canvas holds each band's samples divided by `scale`, so they run from
/// 0 to 1 and fit a half float. Seiza works on the samples as they are.
pub struct BandStretch {
    /// The value that a band sample of 1 on the canvas stands for.
    pub scale: f32,
    domain: ResolvedSampleDomain,
    analysis: StretchAnalysis,
}

/// The samples Seiza analyses per band, at most.
const ANALYSIS_SAMPLES: usize = 300_000;

impl BandStretch {
    /// Analyses one band's samples, as they are in the image.
    fn analyse(samples: &[f32]) -> Result<Self, String> {
        let finite: Vec<f32> = samples
            .iter()
            .copied()
            .filter(|sample| sample.is_finite())
            .collect();
        if finite.is_empty() {
            return Err("a band has no finite samples".to_string());
        }
        let scale = finite.iter().copied().fold(f32::MIN_POSITIVE, f32::max);
        let domain = SampleDomain::PhysicalLinear {
            normalization: SampleNormalization::default(),
        }
        .resolve(&finite, 1)
        .map_err(|error| error.to_string())?;
        let unit = domain
            .apply(&finite, 1)
            .map_err(|error| error.to_string())?;
        let analysis = StretchAnalysis::analyze(&unit, 1, ANALYSIS_SAMPLES)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            scale,
            domain,
            analysis,
        })
    }

    /// The band's stretch as the canvas's table: Seiza's automatic midtones
    /// stretch with the background placed at `target_median`, evaluated at
    /// each table entry's sample.
    pub fn table(&self, target_median: f64) -> Vec<f32> {
        let config = StretchConfig::auto_mtf(
            StretchParams {
                target_median,
                ..StretchParams::default()
            },
            ANALYSIS_SAMPLES,
        );
        let last = (STRETCH_TABLE_WIDTH - 1) as f32;
        // Entry i stands for the canvas sample (i / last) squared.
        let samples: Vec<f32> = (0..STRETCH_TABLE_WIDTH)
            .map(|index| (index as f32 / last).powi(2) * self.scale)
            .collect();
        let resolved = config
            .resolve(&self.analysis)
            .ok()
            .zip(self.domain.apply(&samples, 1).ok());
        match resolved {
            Some((plan, unit)) => {
                let curve = plan.curves()[0];
                unit.into_iter()
                    .map(|sample| curve.map(sample).clamp(0.0, 1.0))
                    .collect()
            }
            // Settings Seiza refuses leave the band unstretched.
            None => (0..STRETCH_TABLE_WIDTH)
                .map(|index| (index as f32 / last).powi(2))
                .collect(),
        }
    }
}

impl StoreTiles {
    pub fn open(path: &Path) -> Result<Self, StudioError> {
        let store = TileStore::open(path)?;
        let shape = store.shape();
        let levels = (0..store.levels())
            .map(|level| store.level(level))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            pyramid: Pyramid {
                width: shape.width as u32,
                height: shape.height as u32,
                levels: store.levels(),
            },
            bands: shape.channels,
            levels,
            scales: vec![1.0; shape.channels],
        })
    }

    /// Analyses each band for its stretch, from patches of the image at
    /// full resolution spread across it, and sets the scale the canvas's
    /// samples are divided by.
    ///
    /// Patches, not a reduced level: a reduced level's samples are means, so
    /// its noise is lower than the image's and a stretch resolved from it
    /// would clip the shadows in the wrong place.
    pub fn analyse(&mut self) -> Result<Vec<BandStretch>, String> {
        let level = &self.levels[0];
        let shape = level.shape();
        let patch = 256.min(shape.width).min(shape.height);
        let mut bands = vec![Vec::new(); self.bands];
        for row in 0..8 {
            for column in 0..8 {
                let x = (shape.width - patch) * column / 7;
                let y = (shape.height - patch) * row / 7;
                let samples = level
                    .read(Region::new(x, y, patch, patch))
                    .map_err(|error| error.to_string())?;
                for (index, sample) in samples.iter().enumerate() {
                    bands[index % self.bands].push(*sample);
                }
            }
        }
        let stretches = bands
            .iter()
            .map(|samples| BandStretch::analyse(samples))
            .collect::<Result<Vec<_>, _>>()?;
        self.scales = stretches.iter().map(|stretch| stretch.scale).collect();
        Ok(stretches)
    }
}

impl TileSource for StoreTiles {
    fn pyramid(&self) -> Pyramid {
        self.pyramid
    }

    fn bands(&self) -> usize {
        self.bands
    }

    fn tile(&self, key: TileKey) -> Vec<u16> {
        let tile = TILE as usize;
        let plane = tile * tile;
        let mut out = vec![0u16; plane * self.bands];
        let Some(level) = self.levels.get(key.level as usize) else {
            return out;
        };
        let shape = level.shape();
        let (x0, y0) = (key.x as usize * tile, key.y as usize * tile);
        if x0 >= shape.width || y0 >= shape.height {
            return out;
        }
        let region = Region::new(
            x0,
            y0,
            tile.min(shape.width - x0),
            tile.min(shape.height - y0),
        );
        // A tile that cannot be read stays black; the canvas has no way to
        // show an error for one tile yet.
        let Ok(samples) = level.read(region) else {
            return out;
        };
        for (index, sample) in samples.iter().enumerate() {
            let (pixel, band) = (index / self.bands, index % self.bands);
            let (x, y) = (pixel % region.width, pixel / region.width);
            // A missing sample is drawn as black.
            let value = if sample.is_finite() {
                (*sample / self.scales[band]).clamp(0.0, 1.0)
            } else {
                0.0
            };
            out[band * plane + y * tile + x] = half_bits(value);
        }
        out
    }
}

/// A float as half-float bits.
fn half_bits(value: f32) -> u16 {
    half::f16::from_f32(value).to_bits()
}

/// Part of the synthetic document, read by region, so it can be written to
/// a store.
struct SyntheticRegion {
    shape: ImageShape,
    /// The synthetic tile at this region's top left corner.
    origin: (u16, u16),
}

impl RegionReader for SyntheticRegion {
    fn shape(&self) -> ImageShape {
        self.shape
    }

    fn domain(&self) -> Domain {
        Domain::Linear
    }

    fn peak_bytes(&self, region: Region) -> u64 {
        // The result, and one synthetic tile.
        ((region.width * region.height + (TILE * TILE) as usize) * BANDS * 4) as u64
    }

    fn read(&self, region: Region) -> Result<Vec<f32>, StudioError> {
        region.check(self.shape)?;
        let tile = TILE as usize;
        let plane = tile * tile;
        let mut out = vec![0f32; region.width * region.height * BANDS];
        for ty in region.y / tile..=(region.bottom() - 1) / tile {
            for tx in region.x / tile..=(region.right() - 1) / tile {
                let samples = generate_tile(TileKey {
                    level: 0,
                    x: self.origin.0 + tx as u16,
                    y: self.origin.1 + ty as u16,
                });
                let rows = region.y.max(ty * tile)..region.bottom().min((ty + 1) * tile);
                let columns = region.x.max(tx * tile)..region.right().min((tx + 1) * tile);
                for y in rows {
                    for x in columns.clone() {
                        let from = (y - ty * tile) * tile + (x - tx * tile);
                        let to = ((y - region.y) * region.width + (x - region.x)) * BANDS;
                        for band in 0..BANDS {
                            out[to + band] =
                                half::f16::from_bits(samples[band * plane + from]).to_f32();
                        }
                    }
                }
            }
        }
        Ok(out)
    }
}

/// Writes a `width` by `height` part of the synthetic document to a store.
pub fn write_synthetic_store(path: &Path, width: usize, height: usize) -> Result<(), StudioError> {
    let width = width.min(DOC_WIDTH as usize / 2);
    let height = height.min(DOC_HEIGHT as usize / 2);
    let source = SyntheticRegion {
        shape: ImageShape::new(width, height, BANDS),
        origin: (8, 6),
    };
    let options = StoreOptions {
        tile: TILE as usize,
        sample: SampleFormat::F32,
        budget: MemoryBudget::new(256 << 20),
        key: "synthetic".to_string(),
    };
    TileStore::create(path, &source, &options).map(|_| ())
}

/// Each band of the synthetic document, analysed from a few of its tiles.
fn analyse_synthetic() -> Result<Vec<BandStretch>, String> {
    let plane = (TILE * TILE) as usize;
    let mut bands = vec![Vec::new(); BANDS];
    for (x, y) in [(5, 4), (20, 12), (33, 9), (45, 30), (12, 28), (50, 18)] {
        let samples = generate_tile(TileKey { level: 0, x, y });
        for (band, values) in bands.iter_mut().enumerate() {
            values.extend(
                samples[band * plane..(band + 1) * plane]
                    .iter()
                    .map(|bits| half::f16::from_bits(*bits).to_f32()),
            );
        }
    }
    bands
        .iter()
        .map(|samples| {
            // The synthetic tiles go to the canvas as they are.
            BandStretch::analyse(samples).map(|stretch| BandStretch {
                scale: 1.0,
                ..stretch
            })
        })
        .collect()
}

/// The tile source the options ask for, with each band's stretch analysis.
pub fn source(open: Option<&Path>) -> Result<(Arc<dyn TileSource>, Vec<BandStretch>), String> {
    match open {
        Some(path) => {
            let mut store = StoreTiles::open(path).map_err(|error| error.to_string())?;
            let stretches = store.analyse()?;
            Ok((Arc::new(store), stretches))
        }
        None => Ok((
            Arc::new(studio_render_spike::synth::Synthetic),
            analyse_synthetic()?,
        )),
    }
}
