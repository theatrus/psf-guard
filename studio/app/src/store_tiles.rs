//! A tile store on disk as the canvas's source of tiles, and the synthetic
//! document written out as a store to try it with.

use crate::derived::{BandRead, DerivedKey, DerivedKind, DerivedPlane};
use crate::image_steps::{Domain, ImageShape, Region};
use crate::studio_model::{
    background_corrected_channel, tools_channel, MemoryBudget, RegionReader, SampleFormat,
    StoreLevel, StoreOptions, StudioError, TileStore,
};
use psf_studio_render::synth::{generate_tile, TileKey, BANDS, DOC_HEIGHT, DOC_WIDTH, TILE};
use psf_studio_render::{Pyramid, TileSource};
use seiza_compose::gpu::GpuInput;
use seiza_compose::picture::{LayeredStage, Transfer, TransferStage};
use seiza_compose::ReferenceRegion;
use seiza_stretch::{
    LayeredMaps, ResolvedCurve, ResolvedLayered, ResolvedSampleDomain, SampleDomain,
    SampleNormalization, StretchAnalysis, StretchConfig, StretchModel, StretchParams,
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
    /// The whole band at a quarter of the resolution, from 0 to 1 as the
    /// stretch reads it, with its width and height: what a layered
    /// stretch's maps are measured from.
    quarter: Option<Arc<(Vec<f32>, usize, usize)>>,
    /// Layered maps measured before, by what moves them.
    maps: Mutex<Vec<(MapsKey, Arc<LayeredMaps>)>>,
    /// Which band this is, in which state: what names the planes worked
    /// out for its stretch.
    band: usize,
    state: BandState,
    /// For red, green and blue stretched together: their luminance, which
    /// a layered stretch reads and each channel follows.
    linked: Option<Arc<LinkedLuminance>>,
}

/// The luminance of red, green and blue stretched together, as a layered
/// stretch reads it.
pub struct LinkedLuminance {
    /// Red, green and blue, and each one's state.
    bands: Vec<usize>,
    states: Vec<BandState>,
    /// The analysis of the luminance's samples, from 0 to 1.
    analysis: StretchAnalysis,
    /// The luminance at a quarter of the resolution, from 0 to 1.
    quarter: Option<Arc<(Vec<f32>, usize, usize)>>,
}

/// Every channel of an image at a quarter of the resolution, and their
/// width and height.
type Quarter = (Vec<Arc<Vec<f32>>>, (usize, usize));

/// What a layered stretch's maps depend on: the band's background
/// statistics, the core compression's settings and whether there is
/// contrast recovery and at what scale. The black point and background
/// target move the curve, not the maps.
type MapsKey = [u64; 7];

fn maps_key(layered: &ResolvedLayered) -> MapsKey {
    let hdr = layered.hdr.unwrap_or_default();
    let recovery = if layered.params.contrast_recovery > 0.0 {
        f64::from(layered.params.scale_separation)
    } else {
        0.0
    };
    [
        layered.median.to_bits(),
        layered.noise.to_bits(),
        u64::from(layered.hdr.is_some()),
        hdr.core_compression.to_bits(),
        hdr.core_threshold.to_bits(),
        hdr.core_scale.to_bits(),
        recovery.to_bits(),
    ]
}

/// What a plane stretched for display is divided by on the way to the
/// canvas: the brightest value an HDR stretch gives, at the most headroom.
/// An SDR plane runs from 0 to 1 within it.
fn display_scale() -> f32 {
    seiza_compose::tone::extended_srgb_encode(seiza_compose::tone::MAX_HEADROOM.exp2())
}

/// How much brighter than the quarter-resolution level's largest sample a
/// band's largest sample at full resolution may be: those samples are means
/// of sixteen.
const QUARTER_MARGIN: f32 = 1.25;

/// About how many samples of each channel a band's stretch is resolved
/// from, evenly over the whole image.
const WHOLE_SAMPLES: usize = 600_000;

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
            quarter: None,
            maps: Mutex::new(Vec::new()),
            band: 0,
            state: BandState::default(),
            linked: None,
        })
    }

    /// The maps of a layered `curve` over this band, measured from its
    /// quarter-resolution copy, or taken from the last measure with the
    /// same settings. `None` for another curve or a band with no copy.
    fn layered_maps(
        &self,
        curve: &ResolvedCurve,
        quarter: Option<&Arc<(Vec<f32>, usize, usize)>>,
    ) -> Option<Arc<LayeredMaps>> {
        let ResolvedCurve::Layered(layered) = curve else {
            return None;
        };
        let quarter = quarter?;
        let key = maps_key(layered);
        let mut known = self.maps.lock().ok()?;
        if let Some((_, maps)) = known.iter().find(|(other, _)| *other == key) {
            return Some(maps.clone());
        }
        let (samples, width, height) = quarter.as_ref();
        let maps = Arc::new(layered.measure(samples, *width, *height));
        // A few settings back and forth are kept, not every one.
        if known.len() == 4 {
            known.remove(0);
        }
        known.push((key, maps.clone()));
        Some(maps)
    }

    /// Gives the band its quarter-resolution copy, `width` by `height`
    /// samples as they are in the image.
    fn set_quarter(&mut self, samples: &[f32], width: usize, height: usize) {
        if let Ok(unit) = self.domain.apply(samples, 1) {
            self.quarter = Some(Arc::new((unit, width, height)));
        }
    }

    /// The band's stretch as a transfer of its samples, as they are in the
    /// image: Seiza's mapping of the band onto the unit range, then the
    /// stretches in `stages` in order, each resolved by Seiza and kept
    /// within 0 to 1. Each stage after the first is resolved from the
    /// band's samples as the stages before it left them, as Seiza's stretch
    /// stack does. With no stage the band is left linear, kept within 0 to
    /// 1.
    ///
    /// `nudge` is added to the background target of every automatic
    /// midtones stage; the benchmark uses it to time a stretch change.
    ///
    /// A stage Seiza refuses is left out. A layered stretch reads maps of
    /// the whole band at a quarter of its resolution; `headroom`, in stops,
    /// is the picture's, which the HDR layered stretch reaches into (0 for
    /// an SDR picture).
    ///
    /// A layered stretch reads planes worked out from the whole image: the
    /// luminance of red, green and blue stretched together, and the fine
    /// blur star-core compression needs. `planes` gives the tile channel of
    /// one that is done; one that is not is added to `missing`, and the
    /// stretch is drawn without it meanwhile.
    pub fn transfer(
        &self,
        stages: &[StretchModel],
        nudge: f64,
        headroom: f64,
        planes: &dyn Fn(&DerivedKey) -> Option<usize>,
        missing: &mut Vec<DerivedKey>,
    ) -> Transfer {
        let unit = TransferStage::Clamp {
            low: 0.0,
            high: 1.0,
        };
        // A band stretched for display is shown as it is.
        if self.display {
            return Transfer::of([unit]);
        }
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
                // The HDR layered stretch reaches as far as the picture does.
                StretchModel::LayeredHdr(params) => {
                    let mut params = *params;
                    params.hdr.headroom = headroom;
                    StretchModel::LayeredHdr(params)
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
            let mut curve = plan.curves()[0];
            // Red, green and blue stretched together with a layered stretch
            // read their luminance, once the plane of it is done.
            let range = self.domain.input_range();
            let mut quarter = self.quarter.as_ref();
            let mut luminance = None;
            if let (ResolvedCurve::Layered(_), Some(linked), 0) = (curve, &self.linked, at) {
                let key = DerivedKey {
                    kind: DerivedKind::Luminance,
                    bands: linked.bands.clone(),
                    states: linked.states.clone(),
                    range,
                };
                match (planes(&key), config.resolve(&linked.analysis)) {
                    (Some(channel), Ok(plan)) => {
                        curve = plan.curves()[0];
                        quarter = linked.quarter.as_ref();
                        luminance = Some(channel);
                    }
                    _ => missing.push(key),
                }
            }
            // A layered stretch reads maps of the whole band, so it is
            // drawn as the first stage only.
            if let (ResolvedCurve::Layered(layered), Some(maps), 0) =
                (curve, self.layered_maps(&curve, quarter), at)
            {
                // Star cores are measured against the fine blur of what the
                // stretch reads.
                let fine = layered.compresses_star_cores().then(|| {
                    let key = match (&self.linked, luminance) {
                        (Some(linked), Some(_)) => DerivedKey {
                            kind: DerivedKind::Fine,
                            bands: linked.bands.clone(),
                            states: linked.states.clone(),
                            range,
                        },
                        _ => DerivedKey {
                            kind: DerivedKind::Fine,
                            bands: vec![self.band],
                            states: vec![self.state],
                            range,
                        },
                    };
                    let channel = planes(&key);
                    if channel.is_none() {
                        missing.push(key);
                    }
                    channel
                });
                let stage = LayeredStage::new(layered, maps).with_guides(luminance, fine.flatten());
                let peak = stage.peak();
                transfer.extend([
                    TransferStage::Layered(stage),
                    TransferStage::Clamp {
                        low: 0.0,
                        high: peak,
                    },
                ]);
            } else {
                transfer.extend([TransferStage::Curve(curve), unit.clone()]);
            }
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
#[derive(Clone, Default)]
pub struct Bands {
    /// The store the bands are in, for the planes worked out from it.
    pub store: Option<Arc<StoreTiles>>,
    /// The planes worked out from the store, and the tile channel each is
    /// drawn from: after the store's own.
    derived: Vec<(DerivedKey, usize)>,
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
    /// Per band: the whole band as stacked, after its tools, and corrected,
    /// at a quarter of the resolution, for layered stretches' maps.
    quarter: Vec<[Option<Arc<Vec<f32>>>; 3]>,
    /// The width and height of the quarter-resolution copies.
    quarter_size: (usize, usize),
    /// Per band: the value a canvas sample of 1 stands for.
    scales: Vec<f32>,
    /// Per tile channel: the value a canvas sample of 1 stands for.
    plane_scales: Vec<f32>,
    /// The bands whose stretches are resolved together: red, green and
    /// blue once color calibration has balanced them. Empty otherwise.
    linked: Vec<usize>,
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

    /// The bands with each band's stretch analysed from `channels`, an even
    /// sample of every tile channel over the whole image, in place of the
    /// patches read when the store was opened.
    pub fn with_whole_samples(&self, channels: &[Vec<f32>]) -> Bands {
        let mut bands = self.clone();
        let plane = |channel: Option<usize>| channel.and_then(|at| channels.get(at).cloned());
        bands.samples = (0..bands.len())
            .map(|band| {
                [
                    plane(Some(bands.tile_band[band])),
                    plane(bands.tools[band]),
                    plane(bands.corrected[band]),
                ]
            })
            .collect();
        bands
    }

    /// The bands with `planes` worked out from the store drawn after the
    /// store's channels, from `self` without any.
    pub fn with_derived(&self, planes: &[Arc<DerivedPlane>]) -> Bands {
        let mut bands = self.clone();
        let first = bands.plane_scales.len() - bands.derived.len();
        bands.plane_scales.truncate(first);
        bands.derived = planes
            .iter()
            .enumerate()
            .map(|(index, plane)| (plane.key.clone(), first + index))
            .collect();
        bands
            .plane_scales
            .extend(planes.iter().map(|plane| plane.scale));
        bands
    }

    /// The planes worked out from the store, and their tile channels.
    pub fn derived_planes(&self) -> Vec<(DerivedKey, usize)> {
        self.derived.clone()
    }

    /// Where a band's samples are read from in `state`, for a plane worked
    /// out from them.
    pub fn band_read(&self, band: usize, state: BandState) -> BandRead {
        let (plane, corrected) = self.planes(band, state);
        BandRead {
            plane,
            corrected: corrected
                .filter(|_| state.background > 0.0)
                .map(|channel| (channel, state.background)),
        }
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

    /// The whole band at a quarter of the resolution in the state given
    /// for it, as the samples of its stretch analysis are.
    fn quarter_in(&self, band: usize, states: &[BandState]) -> Option<Vec<f32>> {
        let [stacked, tools, corrected] = self.quarter.get(band)?;
        let state = states.get(band).copied().unwrap_or_default();
        let stacked = stacked.as_ref()?;
        let base = tools.as_ref().filter(|_| state.tools).unwrap_or(stacked);
        let matches = state.tools == tools.is_some();
        Some(match corrected {
            Some(after) if state.background > 0.0 && matches => base
                .iter()
                .zip(after.iter())
                .map(|(a, b)| a + (b - a) * state.background)
                .collect(),
            _ => base.to_vec(),
        })
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
        let state_of = |band: usize| states.get(band).copied().unwrap_or_default();
        // Red, green and blue together: their luminance, in the range they
        // share, for a layered stretch.
        let linked = match self.linked.as_slice() {
            [red, green, blue] if !together.is_empty() => {
                let shared = BandStretch::analyse(&together)?;
                let luminance = |channels: [&[f32]; 3]| -> Vec<f32> {
                    let count = channels.iter().map(|plane| plane.len()).min().unwrap_or(0);
                    (0..count)
                        .map(|at| {
                            seiza_stretch::rgb_luminance(
                                channels[0][at],
                                channels[1][at],
                                channels[2][at],
                            )
                        })
                        .collect()
                };
                let sampled = luminance([&samples[*red], &samples[*green], &samples[*blue]]);
                let finite: Vec<f32> = sampled.iter().copied().filter(|v| v.is_finite()).collect();
                let unit = shared
                    .domain
                    .apply(&finite, 1)
                    .map_err(|error| error.to_string())?;
                let analysis = StretchAnalysis::analyze(&unit, 1, ANALYSIS_SAMPLES)
                    .map_err(|error| error.to_string())?;
                let quarters: Option<Vec<Vec<f32>>> = [*red, *green, *blue]
                    .iter()
                    .map(|band| self.quarter_in(*band, states))
                    .collect();
                let quarter = quarters.and_then(|planes| {
                    let physical = luminance([&planes[0], &planes[1], &planes[2]]);
                    let unit = shared.domain.apply(&physical, 1).ok()?;
                    let (width, height) = self.quarter_size;
                    Some(Arc::new((unit, width, height)))
                });
                Some(Arc::new(LinkedLuminance {
                    bands: vec![*red, *green, *blue],
                    states: vec![state_of(*red), state_of(*green), state_of(*blue)],
                    analysis,
                    quarter,
                }))
            }
            _ => None,
        };
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
                stretch.band = band;
                stretch.state = state_of(band);
                if self.linked.contains(&band) {
                    stretch.linked = linked.clone();
                }
                if let Some(quarter) = self.quarter_in(band, states) {
                    let (width, height) = self.quarter_size;
                    stretch.set_quarter(&quarter, width, height);
                }
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

    /// An even sample of every channel over the whole image at full
    /// resolution: every so many pixels in reading order, about
    /// `WHOLE_SAMPLES` of each channel. What a band's stretch is resolved
    /// from once it is read, in place of the patches. With `covered`, the
    /// region every input covers as x, y, width and height, the empty edges
    /// registration leaves are not sampled.
    pub fn whole_samples(
        &self,
        covered: Option<[u32; 4]>,
        stop: &std::sync::atomic::AtomicBool,
    ) -> Result<Vec<Vec<f32>>, String> {
        let level = self.full_level().ok_or("the store has no levels")?;
        let shape = level.shape();
        let [left, top, wide, high] = covered
            .map(|[x, y, width, height]| [x as usize, y as usize, width as usize, height as usize])
            .unwrap_or([0, 0, shape.width, shape.height]);
        let right = (left + wide).min(shape.width);
        let bottom = (top + high).min(shape.height);
        let inside = |pixel: usize, y: usize| {
            let (x, y) = (pixel % shape.width, y + pixel / shape.width);
            x >= left && x < right && y >= top && y < bottom
        };
        let pixels = (right.saturating_sub(left) * bottom.saturating_sub(top)).max(1);
        // Odd, so the sample does not fall on one column of a pattern.
        let stride = (pixels / WHOLE_SAMPLES).max(1) | 1;
        let mut channels = vec![Vec::with_capacity(pixels / stride + 1); self.bands];
        let strip = 256.min(shape.height.max(1));
        let mut y = 0;
        while y < shape.height {
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                return Err("it was stopped".to_string());
            }
            let rows = strip.min(shape.height - y);
            let samples = level
                .read(Region::new(0, y, shape.width, rows))
                .map_err(|error| error.to_string())?;
            let first = y * shape.width;
            let start = (stride - first % stride) % stride;
            for pixel in (start..shape.width * rows).step_by(stride) {
                if !inside(pixel, y) {
                    continue;
                }
                for (channel, plane) in channels.iter_mut().enumerate() {
                    plane.push(samples[pixel * self.bands + channel]);
                }
            }
            y += rows;
        }
        Ok(channels)
    }

    /// The store's full-resolution level.
    pub fn full_level(&self) -> Option<&crate::studio_model::StoreLevel> {
        self.levels.first()
    }

    /// Every channel of the whole image at a quarter of the resolution, and
    /// its width and height: the store's level of that size, whose pixels
    /// are means of the image's, or the image reduced here when the store
    /// has no such level.
    fn quarter(&self) -> Result<Quarter, String> {
        let (level, reduce) = match self.levels.get(2) {
            Some(level) => (level, false),
            None => (&self.levels[0], true),
        };
        let shape = level.shape();
        let mut channels = vec![Vec::with_capacity(shape.width * shape.height); self.bands];
        let strip = 256.min(shape.height.max(1));
        let mut y = 0;
        while y < shape.height {
            let rows = strip.min(shape.height - y);
            let samples = level
                .read(Region::new(0, y, shape.width, rows))
                .map_err(|error| error.to_string())?;
            for (index, sample) in samples.iter().enumerate() {
                channels[index % self.bands].push(*sample);
            }
            y += rows;
        }
        if !reduce {
            return Ok((
                channels.into_iter().map(Arc::new).collect(),
                (shape.width, shape.height),
            ));
        }
        let mut size = (0, 0);
        let reduced = channels
            .iter()
            .map(|plane| {
                let (reduced, width, height) =
                    seiza_stretch::reduce_luminance(plane, shape.width, shape.height);
                size = (width, height);
                Arc::new(reduced)
            })
            .collect();
        Ok((reduced, size))
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
        // The whole image at a quarter of the resolution, each channel's
        // own: what layered stretches measure their maps from, and where a
        // small bright object the patches miss is found.
        let (quarter, quarter_size) = self.quarter()?;
        let largest_of = |channel: usize| {
            let brightest = quarter.get(channel).map_or(f32::MIN_POSITIVE, |plane| {
                plane
                    .iter()
                    .copied()
                    .filter(|sample| sample.is_finite())
                    .fold(f32::MIN_POSITIVE, f32::max)
            });
            channels[channel]
                .iter()
                .copied()
                .filter(|sample| sample.is_finite())
                .fold(brightest * QUARTER_MARGIN, f32::max)
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
                .map(largest_of)
                .fold(f32::MIN_POSITIVE, f32::max);
            // The stars a tool took out of an input take the input's scale,
            // so one stretch serves both.
            let scale = name
                .as_deref()
                .and_then(|name| name.strip_suffix("@stars"))
                .and_then(|input| bands.names.iter().position(|other| other == input))
                .map_or(scale, |input| bands.scales[input]);
            // A plane stretched for display is drawn as it is, up to the
            // brightest an HDR stretch can make it.
            let scale = if name
                .as_ref()
                .is_some_and(|name| self.display.contains(name))
            {
                display_scale()
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
            bands.scales.push(scale);
            bands.samples.push([
                Some(channels[channel].clone()),
                tools.map(|plane| channels[plane].clone()),
                corrected.map(|plane| channels[plane].clone()),
            ]);
            bands.quarter.push([
                quarter.get(channel).cloned(),
                tools.and_then(|plane| quarter.get(plane).cloned()),
                corrected.and_then(|plane| quarter.get(plane).cloned()),
            ]);
        }
        // A channel that is no band and no band's other state, such as the
        // stars a tool took out, keeps a scale of its own.
        for channel in 0..self.bands {
            let display = self
                .names
                .get(channel)
                .is_some_and(|name| self.display.contains(name));
            if display {
                self.scales[channel] = display_scale();
            } else if !is_band(channel) && self.scales[channel] == 1.0 {
                self.scales[channel] = largest_of(channel);
            }
        }
        if self.names.is_empty() {
            bands.names = names_for(bands.len());
        }
        bands.plane_scales = self.scales.clone();
        bands.quarter_size = quarter_size;
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
    /// Planes worked out from the store, read after its own channels.
    planes: Vec<Arc<DerivedPlane>>,
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
            planes: Vec::new(),
            recent: Mutex::new(VecDeque::new()),
        }
    }

    /// The source with `planes` worked out from the store as inputs after
    /// its channels, as the canvas draws them.
    pub fn with_planes(self, planes: Vec<Arc<DerivedPlane>>) -> Self {
        Self { planes, ..self }
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
            return self
                .planes
                .get(input - bands)
                .map(|plane| plane.read(rect))
                .ok_or_else(|| {
                    seiza_compose::Error::Color(format!("the store has no channel {input}"))
                });
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
            let (store, mut bands) = open_store(path)?;
            let store = Arc::new(store);
            bands.store = Some(store.clone());
            Ok((store, bands))
        }
        None => Ok((
            Arc::new(psf_studio_render::synth::Synthetic),
            analyse_synthetic(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layered_stretch_reads_maps_of_the_whole_band_and_the_picture_s_headroom() {
        // A faint sky with a bright core, at full and quarter resolution.
        let (width, height) = (64, 64);
        let value = |x: usize, y: usize| {
            let r2 = (x as f32 - 32.0).powi(2) + (y as f32 - 32.0).powi(2);
            100.0 + ((x * 7 + y * 13) % 11) as f32 + 5000.0 * (-r2 / 40.0).exp()
        };
        let full: Vec<f32> = (0..width * height)
            .map(|at| value(at % width, at / width))
            .collect();
        let quarter: Vec<f32> = (0..16 * 16)
            .map(|at| {
                let (x, y) = ((at % 16) * 4, (at / 16) * 4);
                (0..16).map(|k| value(x + k % 4, y + k / 4)).sum::<f32>() / 16.0
            })
            .collect();
        let mut band = BandStretch::analyse(&full).unwrap();
        band.set_quarter(&quarter, 16, 16);
        let model: StretchModel =
            serde_json::from_value(serde_json::json!({ "type": "layered-hdr" })).unwrap();
        let hdr = band.transfer(
            std::slice::from_ref(&model),
            0.0,
            2.0,
            &|_| None,
            &mut Vec::new(),
        );
        let stage = hdr
            .stages
            .iter()
            .find_map(|stage| match stage {
                TransferStage::Layered(stage) => Some(stage),
                _ => None,
            })
            .expect("a layered stage");
        assert!(stage.peak() > 1.5, "{}", stage.peak());
        assert!(!stage.maps.core.is_empty());
        // An SDR picture's HDR stretch rolls off to white.
        let sdr = band.transfer(
            std::slice::from_ref(&model),
            0.0,
            0.0,
            &|_| None,
            &mut Vec::new(),
        );
        assert!(sdr.stages.iter().any(|stage| matches!(
            stage,
            TransferStage::Clamp { high, .. } if (*high - 1.0).abs() < 1e-4
        )));
    }

    #[test]
    fn star_cores_wait_for_their_plane_then_read_it() {
        let full: Vec<f32> = (0..64 * 64)
            .map(|at| 100.0 + (at % 13) as f32 + if at == 2080 { 3000.0 } else { 0.0 })
            .collect();
        let mut band = BandStretch::analyse(&full).unwrap();
        band.set_quarter(&full[..16 * 16], 16, 16);
        let model: StretchModel = serde_json::from_value(serde_json::json!({
            "type": "layered", "dynamic_range_compression": 0.8
        }))
        .unwrap();
        let fine = |stages: &[TransferStage]| {
            stages.iter().find_map(|stage| match stage {
                TransferStage::Layered(stage) => Some(stage.fine),
                _ => None,
            })
        };
        // Before the plane is done the stretch is drawn without star cores,
        // and asks for it.
        let mut missing = Vec::new();
        let waiting = band.transfer(
            std::slice::from_ref(&model),
            0.0,
            0.0,
            &|_| None,
            &mut missing,
        );
        assert_eq!(fine(&waiting.stages), Some(None));
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].kind, DerivedKind::Fine);
        // Once it is done, the stage reads it.
        let key = missing[0].clone();
        let ready = band.transfer(
            std::slice::from_ref(&model),
            0.0,
            0.0,
            &|wanted| (*wanted == key).then_some(7),
            &mut Vec::new(),
        );
        assert_eq!(fine(&ready.stages), Some(Some(7)));
    }

    #[test]
    fn whole_image_samples_replace_the_patches() {
        let bands = analyse_synthetic();
        let channels: Vec<Vec<f32>> = (0..bands.plane_scales.len())
            .map(|channel| vec![channel as f32; 10])
            .collect();
        let sampled = bands.with_whole_samples(&channels);
        for band in 0..sampled.len() {
            let stacked = sampled.samples[band][0].as_ref().unwrap();
            assert_eq!(stacked, &channels[sampled.tile_band[band]]);
        }
    }

    #[test]
    fn a_band_with_no_stretch_is_left_linear() {
        let samples: Vec<f32> = (0..4000).map(|index| 100.0 + (index % 97) as f32).collect();
        let band = BandStretch::analyse(&samples).unwrap();
        // No stretch: the band's range mapped to 0 to 1, and no curve. The
        // view stretch is given as a stretch when a view wants one.
        let linear = band.transfer(&[], 0.0, 0.0, &|_| None, &mut Vec::new());
        assert!(linear
            .stages
            .iter()
            .all(|stage| !matches!(stage, TransferStage::Curve(_))));
        let viewed = band.transfer(
            &[StretchModel::AutoMtf(StretchParams::default())],
            0.0,
            0.0,
            &|_| None,
            &mut Vec::new(),
        );
        assert!(viewed
            .stages
            .iter()
            .any(|stage| matches!(stage, TransferStage::Curve(_))));
    }
}
