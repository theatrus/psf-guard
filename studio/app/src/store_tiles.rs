//! A tile store on disk as the canvas's source of tiles, and the synthetic
//! document written out as a store to try it with.

use crate::image_steps::{Domain, ImageShape, Region};
use crate::studio_model::{
    background_corrected_channel, stretched_channel, tools_channel, MemoryBudget, RegionReader,
    SampleFormat, StoreLevel, StoreOptions, StudioError, TileStore,
};
use psf_studio_render::synth::{generate_tile, TileKey, BANDS, DOC_HEIGHT, DOC_WIDTH, TILE};
use psf_studio_render::{Pyramid, TileSource};
use seiza_compose::gpu::GpuInput;
use seiza_compose::picture::{Transfer, TransferStage};
use seiza_compose::ReferenceRegion;
use seiza_stretch::{
    ResolvedSampleDomain, SampleDomain, SampleNormalization, StretchAnalysis, StretchConfig,
    StretchModel, StretchParams,
};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// A document read from a tile store.
pub struct StoreTiles {
    levels: Vec<StoreLevel>,
    pyramid: Pyramid,
    bands: usize,
    /// What each band's samples are divided by on the way to the canvas.
    scales: Vec<f32>,
    /// The channels' names, when the store has them.
    names: Vec<String>,
    /// The channels that are already stretched for display, by name.
    display: Vec<String>,
}

/// What a band's stretch is resolved from: Seiza's analysis of a sample of
/// the band at full resolution, as the samples are in the image.
pub struct BandStretch {
    domain: ResolvedSampleDomain,
    analysis: StretchAnalysis,
    /// The analysed samples, from 0 to 1: what a second stretch of a chain
    /// is resolved from, after the first has been applied to them.
    unit: Vec<f32>,
    /// Whether the band is already stretched for display, so its stretch
    /// leaves its samples as they are.
    display: bool,
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
            domain,
            analysis,
            unit,
            display: false,
        })
    }

    /// The band's stretch as a transfer of its samples, as they are in the
    /// image: Seiza's mapping of the band onto the unit range, then the
    /// stretches in `stages` in order, each resolved by Seiza and kept
    /// within 0 to 1. Each stage after the first is resolved from the
    /// band's samples as the stages before it left them, as Seiza's stretch
    /// stack does. With no stage it is Seiza's automatic midtones stretch,
    /// for a preview.
    ///
    /// `nudge` is added to the background target of every automatic
    /// midtones stage; the benchmark uses it to time a stretch change.
    ///
    /// A stage Seiza refuses is left out. The layered stretch is one: it
    /// needs the image, not samples of it.
    pub fn transfer(&self, stages: &[StretchModel], nudge: f64) -> Transfer {
        let unit = TransferStage::Clamp {
            low: 0.0,
            high: 1.0,
        };
        // A band stretched for display is shown as it is.
        if self.display {
            return Transfer::of([unit]);
        }
        let preview = [StretchModel::AutoMtf(StretchParams::default())];
        let stages = if stages.is_empty() { &preview } else { stages };
        let mut transfer = vec![TransferStage::Domain(self.domain)];
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
            transfer.extend([TransferStage::Curve(curve), unit.clone()]);
            if at + 1 < stages.len() {
                let mut next = seen.take().unwrap_or_else(|| self.unit.clone());
                for sample in &mut next {
                    *sample = curve.map(*sample).clamp(0.0, 1.0);
                }
                seen = Some(next);
            }
        }
        Transfer::of(transfer)
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
    /// Per tile channel: the value a canvas sample of 1 stands for.
    plane_scales: Vec<f32>,
    /// The bands whose stretches are resolved together: red, green and
    /// blue once color calibration has balanced them. Empty otherwise.
    linked: Vec<usize>,
    /// Per band: the tile channel that holds it after its stretches, when
    /// the store stretched it over the whole image.
    pub stretched: Vec<Option<usize>>,
    /// The bands the store holds already stretched for display, such as
    /// stars after the star stretch. They are drawn as they are.
    display: Vec<usize>,
    /// The largest rectangle every input covers, as x, y, width and height,
    /// which the store records when it is prepared.
    pub automatic_crop: Option<[u32; 4]>,
    /// What color calibration did when the store was prepared, in words.
    /// Empty when it was not asked for.
    pub calibration: String,
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
    /// Reads what color calibration did when the store was prepared, from
    /// the record beside the store. When red, green and blue were
    /// calibrated, their stretches are resolved together from then on: a
    /// stretch of each channel on its own pulls each toward its own median
    /// and undoes the balance.
    fn read_calibration(&mut self, store: &Path) {
        let record = store.with_extension("prepared.json");
        let Some(record) = std::fs::read_to_string(record)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        else {
            return;
        };
        self.automatic_crop = record["crop"].as_array().and_then(|values| {
            let value = |at: usize| Some(values.get(at)?.as_u64()? as u32);
            Some([value(0)?, value(1)?, value(2)?, value(3)?])
        });
        self.display = record["display"]
            .as_array()
            .map(|planes| {
                planes
                    .iter()
                    .filter_map(|plane| self.band_of(plane.as_str()?))
                    .collect()
            })
            .unwrap_or_default();
        let record = &record["calibration"];
        self.calibration = match record["status"].as_str() {
            Some("done") => {
                let rgb: Vec<usize> = ["red", "green", "blue"]
                    .iter()
                    .filter_map(|name| self.band_of(name))
                    .collect();
                if rgb.len() == 3 {
                    self.linked = rgb;
                }
                format!(
                    "Calibrated on {} Gaia stars",
                    record["stars"].as_u64().unwrap_or(0)
                )
            }
            Some("skipped") => format!(
                "Not calibrated: {}",
                record["why"].as_str().unwrap_or("no reason given")
            ),
            _ => String::new(),
        };
    }

    /// The tile channel that holds a band after its stretches, if the
    /// store has one.
    pub fn stretched_plane(&self, band: usize) -> Option<usize> {
        self.stretched.get(band).copied().flatten()
    }

    /// Whether a band is already stretched for display.
    pub fn is_display(&self, band: usize) -> bool {
        self.display.contains(&band)
    }

    /// Whether the store holds stars already stretched for display.
    pub fn has_star_layer(&self) -> bool {
        !self.display.is_empty()
    }

    /// Whether red, green and blue were calibrated and share one stretch.
    pub fn is_calibrated(&self) -> bool {
        !self.linked.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tile_band.len()
    }

    /// Where each tile channel is on the GPU, as Seiza's compositor reads
    /// it: its layer in a tile, and the value a canvas sample of 1 stands
    /// for.
    pub fn gpu_inputs(&self) -> Vec<GpuInput> {
        self.plane_scales
            .iter()
            .enumerate()
            .map(|(layer, scale)| GpuInput {
                layer,
                scale: *scale,
            })
            .collect()
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
    ///
    /// The bands in `linked` are analysed as one set of samples, so one
    /// stretch serves them all and their balance is kept.
    pub fn stretches(&self, states: &[BandState]) -> Result<Vec<BandStretch>, String> {
        let samples: Vec<Vec<f32>> = self
            .samples
            .iter()
            .enumerate()
            .map(|(band, [stacked, tools, corrected])| {
                let state = states.get(band).copied().unwrap_or_default();
                let stacked = stacked.as_deref().unwrap_or(&[]);
                let base = tools.as_deref().filter(|_| state.tools).unwrap_or(stacked);
                let matches = state.tools == tools.is_some();
                match corrected {
                    Some(after) if state.background > 0.0 && matches => base
                        .iter()
                        .zip(after)
                        .map(|(a, b)| a + (b - a) * state.background)
                        .collect(),
                    _ => base.to_vec(),
                }
            })
            .collect();
        let together: Vec<f32> = self
            .linked
            .iter()
            .flat_map(|band| samples[*band].iter().copied())
            .collect();
        samples
            .iter()
            .enumerate()
            .map(|(band, own)| {
                let mut stretch = if self.linked.contains(&band) {
                    BandStretch::analyse(&together)
                } else {
                    BandStretch::analyse(own)
                }?;
                stretch.display = self.display.contains(&band);
                Ok(stretch)
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
            display: Vec::new(),
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
        // The stars a tool took out of a channel are an input of their own.
        let is_band = |channel: usize| {
            self.names.get(channel).is_none_or(|name| {
                !name.contains('@') || name.ends_with("@stars") || name.ends_with("@line")
            })
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
            // The stars a tool took out of an input take the input's scale,
            // so one stretch serves both.
            let scale = name
                .as_deref()
                .and_then(|name| name.strip_suffix("@stars"))
                .and_then(|input| bands.names.iter().position(|other| other == input))
                .map_or(scale, |input| bands.scales[input]);
            // A plane stretched for display runs from 0 to 1 as it is.
            let scale = if name
                .as_ref()
                .is_some_and(|name| self.display.contains(name))
            {
                1.0
            } else {
                scale
            };
            for plane in [Some(channel), tools, corrected].into_iter().flatten() {
                self.scales[plane] = scale;
            }
            bands.tile_band.push(channel);
            bands.names.push(name.unwrap_or_default());
            bands.tools.push(tools);
            bands.corrected.push(corrected);
            bands.stretched.push(
                bands
                    .names
                    .last()
                    .and_then(|name| named(stretched_channel(name)))
                    .filter(|plane| self.display.contains(&self.names[*plane])),
            );
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
            let display = self
                .names
                .get(channel)
                .is_some_and(|name| self.display.contains(name));
            if !is_band(channel) && self.scales[channel] == 1.0 && !display {
                self.scales[channel] = largest(&channels[channel]);
            }
        }
        if self.names.is_empty() {
            bands.names = names_for(bands.len());
        }
        bands.plane_scales = self.scales.clone();
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
        plane_scales: vec![1.0; BANDS],
        ..Default::default()
    }
}

/// A store opened for the canvas, with its bands analysed and the scales
/// its channels are divided by.
pub fn open_store(path: &Path) -> Result<(StoreTiles, Bands), String> {
    let mut store = StoreTiles::open(path).map_err(|error| error.to_string())?;
    // The planes already stretched for display keep a scale of 1.
    store.display = std::fs::read_to_string(path.with_extension("prepared.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|record| {
            Some(
                record["display"]
                    .as_array()?
                    .iter()
                    .filter_map(|plane| Some(plane.as_str()?.to_string()))
                    .collect(),
            )
        })
        .unwrap_or_default();
    let mut bands = store.analyse()?;
    bands.read_calibration(path);
    Ok((store, bands))
}

/// A store's full-resolution samples as Seiza's compositor reads them for
/// an export: each tile channel an input, kept within what the canvas's
/// tiles hold (0 to the channel's scale), with a missing sample as 0, so
/// the export is the picture the canvas shows.
pub struct StoreSource {
    store: StoreTiles,
    /// The regions read lately, all channels interleaved: a tile reads each
    /// of its inputs over the same region in turn, so a couple per thread
    /// that renders tiles is enough. Each is a tile and its margin in every
    /// channel, tens of megabytes.
    recent: Mutex<VecDeque<(ReferenceRegion, Arc<Vec<f32>>)>>,
}

impl StoreSource {
    pub fn new(store: StoreTiles) -> Self {
        Self {
            store,
            recent: Mutex::new(VecDeque::new()),
        }
    }

    fn region(&self, rect: ReferenceRegion) -> Result<Arc<Vec<f32>>, seiza_compose::Error> {
        let known = |recent: &VecDeque<(ReferenceRegion, Arc<Vec<f32>>)>| {
            recent
                .iter()
                .find(|(at, _)| *at == rect)
                .map(|(_, samples)| samples.clone())
        };
        if let Some(samples) = known(&self.recent.lock().expect("recent regions")) {
            return Ok(samples);
        }
        let level =
            self.store.levels.first().ok_or_else(|| {
                seiza_compose::Error::Color("the store has no levels".to_string())
            })?;
        let samples = Arc::new(
            level
                .read(Region::new(rect.x, rect.y, rect.width, rect.height))
                .map_err(|error| seiza_compose::Error::Color(error.to_string()))?,
        );
        let mut recent = self.recent.lock().expect("recent regions");
        recent.push_front((rect, samples.clone()));
        let kept = 2 * std::thread::available_parallelism().map_or(8, |count| count.get());
        recent.truncate(kept);
        Ok(samples)
    }
}

impl seiza_compose::render::Source for StoreSource {
    fn size(&self) -> (usize, usize) {
        (
            self.store.pyramid.width as usize,
            self.store.pyramid.height as usize,
        )
    }

    fn read(&self, input: usize, rect: ReferenceRegion) -> Result<Vec<f32>, seiza_compose::Error> {
        let bands = self.store.bands;
        if input >= bands {
            return Err(seiza_compose::Error::Color(format!(
                "the store has no channel {input}"
            )));
        }
        let scale = self.store.scales[input];
        let samples = self.region(rect)?;
        Ok(samples
            .iter()
            .skip(input)
            .step_by(bands)
            .map(|sample| {
                if sample.is_finite() {
                    sample.clamp(0.0, scale)
                } else {
                    0.0
                }
            })
            .collect())
    }
}

/// The tile source the options ask for, with its bands.
pub fn source(open: Option<&Path>) -> Result<(Arc<dyn TileSource>, Bands), String> {
    match open {
        Some(path) => {
            let (store, bands) = open_store(path)?;
            Ok((Arc::new(store), bands))
        }
        None => Ok((
            Arc::new(psf_studio_render::synth::Synthetic),
            analyse_synthetic(),
        )),
    }
}
