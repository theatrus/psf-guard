//! A tile store on disk as the canvas's source of tiles, and the synthetic
//! document written out as a store to try it with.

use crate::image_steps::{Domain, ImageShape, Region};
use crate::studio_model::{
    background_corrected_channel, tools_channel, MemoryBudget, RegionReader, SampleFormat,
    StoreLevel, StoreOptions, StudioError, TileStore,
};
use seiza_stretch::{
    ResolvedSampleDomain, SampleDomain, SampleNormalization, StretchAnalysis, StretchConfig,
    StretchModel, StretchParams,
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
    /// The channels' names, when the store has them.
    names: Vec<String>,
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
    /// The analysed samples, from 0 to 1: what a second stretch of a chain
    /// is resolved from, after the first has been applied to them.
    unit: Vec<f32>,
}

/// The samples Seiza analyses per band, at most.
const ANALYSIS_SAMPLES: usize = 300_000;

impl BandStretch {
    /// Analyses one band's samples, as they are in the image. `scale` is
    /// the value a canvas sample of 1 stands for; without one it is the
    /// largest sample.
    fn analyse(samples: &[f32], scale: Option<f32>) -> Result<Self, String> {
        let finite: Vec<f32> = samples
            .iter()
            .copied()
            .filter(|sample| sample.is_finite())
            .collect();
        if finite.is_empty() {
            return Err("a band has no finite samples".to_string());
        }
        let scale =
            scale.unwrap_or_else(|| finite.iter().copied().fold(f32::MIN_POSITIVE, f32::max));
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
            unit,
        })
    }

    /// The band's stretch as the canvas's table: the stretches in `stages`
    /// applied in order, each resolved by Seiza, evaluated at each table
    /// entry's sample. Each stage after the first is resolved from the
    /// band's samples as the stages before it left them, as Seiza's stretch
    /// stack does. With no stage the table is Seiza's automatic midtones
    /// stretch, for a preview.
    ///
    /// `nudge` is added to the background target of every automatic
    /// midtones stage; the benchmark uses it to time a stretch change.
    ///
    /// A stage Seiza refuses is left out. The layered stretch is one: it
    /// needs the image, not samples of it.
    pub fn table(&self, stages: &[StretchModel], nudge: f64) -> Vec<f32> {
        let preview = [StretchModel::AutoMtf(StretchParams::default())];
        let stages = if stages.is_empty() { &preview } else { stages };
        let last = (STRETCH_TABLE_WIDTH - 1) as f32;
        // Entry i stands for the canvas sample (i / last) squared.
        let samples: Vec<f32> = (0..STRETCH_TABLE_WIDTH)
            .map(|index| (index as f32 / last).powi(2) * self.scale)
            .collect();
        let mut values = self.domain.apply(&samples, 1).unwrap_or_else(|_| {
            (0..STRETCH_TABLE_WIDTH)
                .map(|index| (index as f32 / last).powi(2))
                .collect()
        });
        // The band's samples as the stages so far left them. They are only
        // needed from the second stage on.
        let mut seen: Option<Vec<f32>> = None;
        for (at, stage) in stages.iter().enumerate() {
            let model = match stage {
                StretchModel::AutoMtf(params) if nudge != 0.0 => {
                    StretchModel::AutoMtf(StretchParams {
                        target_median: (params.target_median + nudge).clamp(0.05, 0.5),
                        ..*params
                    })
                }
                other => other.clone(),
            };
            let config = StretchConfig {
                model,
                color_strategy: Default::default(),
                max_analysis_samples: ANALYSIS_SAMPLES,
            };
            let plan = match &seen {
                None => config.resolve(&self.analysis),
                Some(seen) => config.resolve_for(seen, 1),
            };
            let Ok(plan) = plan else { continue };
            let curve = plan.curves()[0];
            for value in &mut values {
                *value = curve.map(*value).clamp(0.0, 1.0);
            }
            if at + 1 < stages.len() {
                let mut next = seen.take().unwrap_or_else(|| self.unit.clone());
                for sample in &mut next {
                    *sample = curve.map(*sample).clamp(0.0, 1.0);
                }
                seen = Some(next);
            }
        }
        values
    }
}

/// What one band is drawn from at a place in the document's steps.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BandState {
    /// Whether the band's processing tools have been applied.
    pub tools: bool,
    /// How much of the background correction is applied, from 0 to 1.
    pub background: f32,
}

/// The bands of a document, as the canvas needs them: where each is in a
/// tile, samples to analyse for its stretch, and the tile channels that
/// hold it after its processing tools and with its background corrected in
/// full, when the store has them.
#[derive(Default)]
pub struct Bands {
    /// Per band: the input's name, as PSF Guard's settings give it, such
    /// as `ha` or `red`.
    pub names: Vec<String>,
    /// Per band: the tile channel that holds it as stacked.
    pub tile_band: Vec<usize>,
    /// Per band: the tile channel that holds it after its tools, if any.
    pub tools: Vec<Option<usize>>,
    /// Per band: the tile channel that holds it corrected, if any. The
    /// corrected band is the tools' result corrected, where the band has
    /// tools.
    pub corrected: Vec<Option<usize>>,
    /// Per band: samples of the band as stacked, after its tools, and
    /// corrected.
    samples: Vec<[Option<Vec<f32>>; 3]>,
    /// Per band: the value a canvas sample of 1 stands for.
    scales: Vec<f32>,
}

/// Names for the bands of a store that records none: by how many it has.
fn names_for(count: usize) -> Vec<String> {
    let named: &[&str] = match count {
        1 => &["luminance"],
        3 => &["red", "green", "blue"],
        4 => &["luminance", "red", "green", "blue"],
        8 => &[
            "luminance",
            "red",
            "green",
            "blue",
            "ha",
            "oiii",
            "sii",
            "nii",
        ],
        _ => &[],
    };
    (0..count)
        .map(|band| {
            named
                .get(band)
                .map_or_else(|| format!("band {}", band + 1), |name| name.to_string())
        })
        .collect()
}

impl Bands {
    pub fn len(&self) -> usize {
        self.tile_band.len()
    }

    /// The band that holds an input, by the input's name.
    pub fn band_of(&self, channel: &str) -> Option<usize> {
        self.names.iter().position(|name| name == channel)
    }

    /// Whether any band has a corrected twin to blend with.
    pub fn has_correction(&self) -> bool {
        self.corrected.iter().any(Option::is_some)
    }

    /// Whether the store holds a band after its processing tools.
    pub fn has_tools(&self, band: usize) -> bool {
        self.tools.get(band).is_some_and(Option::is_some)
    }

    /// The tile channel a band is drawn from in `state`, and the channel
    /// to blend into it for the background correction, if the store's
    /// corrected channel is that state's: the store corrects the tools'
    /// result where a band has tools, so a view before the tools has no
    /// corrected channel to blend.
    pub fn planes(&self, band: usize, state: BandState) -> (usize, Option<usize>) {
        let tools = self.tools[band].filter(|_| state.tools);
        let matches = tools.is_some() == self.tools[band].is_some();
        (
            tools.unwrap_or(self.tile_band[band]),
            self.corrected[band].filter(|_| matches),
        )
    }

    /// Each band's stretch analysis in the state given for it: the
    /// analysis of the samples the stretch is applied to. A band with no
    /// state given is as stacked.
    pub fn stretches(&self, states: &[BandState]) -> Result<Vec<BandStretch>, String> {
        self.samples
            .iter()
            .zip(&self.scales)
            .enumerate()
            .map(|(band, ([stacked, tools, corrected], scale))| {
                let state = states.get(band).copied().unwrap_or_default();
                let stacked = stacked.as_deref().unwrap_or(&[]);
                let base = tools.as_deref().filter(|_| state.tools).unwrap_or(stacked);
                let matches = state.tools == tools.is_some();
                match corrected {
                    Some(after) if state.background > 0.0 && matches => {
                        let blended: Vec<f32> = base
                            .iter()
                            .zip(after)
                            .map(|(a, b)| a + (b - a) * state.background)
                            .collect();
                        BandStretch::analyse(&blended, Some(*scale))
                    }
                    _ => BandStretch::analyse(base, Some(*scale)),
                }
            })
            .collect()
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
            names: store.channel_names().to_vec(),
            levels,
            scales: vec![1.0; shape.channels],
        })
    }

    /// Reads samples of each band for its stretch, from patches of the
    /// image at full resolution spread across it, and sets the scale the
    /// canvas's samples are divided by.
    ///
    /// Patches, not a reduced level: a reduced level's samples are means, so
    /// its noise is lower than the image's and a stretch resolved from it
    /// would clip the shadows in the wrong place.
    ///
    /// A channel named as another's background-corrected twin is not a band
    /// of its own. It and its twin get one scale, so the canvas can blend
    /// their samples.
    pub fn analyse(&mut self) -> Result<Bands, String> {
        let level = &self.levels[0];
        let shape = level.shape();
        let patch = 128.min(shape.width).min(shape.height);
        let mut channels = vec![Vec::new(); self.bands];
        for row in 0..8 {
            for column in 0..8 {
                let x = (shape.width - patch) * column / 7;
                let y = (shape.height - patch) * row / 7;
                let samples = level
                    .read(Region::new(x, y, patch, patch))
                    .map_err(|error| error.to_string())?;
                for (index, sample) in samples.iter().enumerate() {
                    channels[index % self.bands].push(*sample);
                }
            }
        }
        let largest = |samples: &[f32]| {
            samples
                .iter()
                .copied()
                .filter(|sample| sample.is_finite())
                .fold(f32::MIN_POSITIVE, f32::max)
        };
        // A channel named after another with a suffix holds that channel
        // in another state, and is not a band of its own.
        let named = |name: String| self.names.iter().position(|other| *other == name);
        let is_band = |channel: usize| {
            self.names
                .get(channel)
                .is_none_or(|name| !name.contains('@'))
        };
        let mut bands = Bands::default();
        for channel in (0..self.bands).filter(|channel| is_band(*channel)) {
            let name = self.names.get(channel).cloned();
            let tools = name.as_deref().and_then(|name| named(tools_channel(name)));
            let corrected = name
                .as_deref()
                .and_then(|name| named(background_corrected_channel(name)));
            // The band's channels share one scale, so the canvas can blend
            // their samples.
            let scale = [Some(channel), tools, corrected]
                .into_iter()
                .flatten()
                .map(|plane| largest(&channels[plane]))
                .fold(f32::MIN_POSITIVE, f32::max);
            for plane in [Some(channel), tools, corrected].into_iter().flatten() {
                self.scales[plane] = scale;
            }
            bands.tile_band.push(channel);
            bands.names.push(name.unwrap_or_default());
            bands.tools.push(tools);
            bands.corrected.push(corrected);
            bands.scales.push(scale);
            bands.samples.push([
                Some(channels[channel].clone()),
                tools.map(|plane| channels[plane].clone()),
                corrected.map(|plane| channels[plane].clone()),
            ]);
        }
        // A channel that is no band and no band's other state, such as the
        // stars a tool took out, keeps a scale of its own.
        for channel in 0..self.bands {
            if !is_band(channel) && self.scales[channel] == 1.0 {
                self.scales[channel] = largest(&channels[channel]);
            }
        }
        if self.names.is_empty() {
            bands.names = names_for(bands.len());
        }
        Ok(bands)
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
        channels: Vec::new(),
    };
    TileStore::create(path, &source, &options).map(|_| ())
}

/// Each band of the synthetic document, analysed from a few of its tiles.
fn analyse_synthetic() -> Bands {
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
    Bands {
        names: names_for(BANDS),
        tile_band: (0..BANDS).collect(),
        tools: vec![None; BANDS],
        corrected: vec![None; BANDS],
        samples: bands
            .into_iter()
            .map(|samples| [Some(samples), None, None])
            .collect(),
        // The synthetic tiles go to the canvas as they are.
        scales: vec![1.0; BANDS],
    }
}

/// The tile source the options ask for, with its bands.
pub fn source(open: Option<&Path>) -> Result<(Arc<dyn TileSource>, Bands), String> {
    match open {
        Some(path) => {
            let mut store = StoreTiles::open(path).map_err(|error| error.to_string())?;
            let bands = store.analyse()?;
            Ok((Arc::new(store), bands))
        }
        None => Ok((
            Arc::new(studio_render_spike::synth::Synthetic),
            analyse_synthetic(),
        )),
    }
}
