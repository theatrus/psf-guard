//! The canvas's passes on the CPU, at full resolution and in 32-bit floats,
//! for an export of the finished picture.
//!
//! The canvas holds its tiles and stretch tables in half floats, about three
//! significant digits, which is too coarse for a 16-bit file. This render
//! takes the same [`Pane`] the canvas draws, reads the store's samples as
//! 32-bit floats, and follows `layers.wgsl` and `finish.wgsl` step for
//! step: each function here is named after the shader function it mirrors.
//! A change to a shader changes its twin here in the same commit.
//!
//! Where the two differ by design:
//!
//! - The canvas draws at the view's zoom; this render draws at full
//!   resolution, where a sample is a texel and needs no filtering.
//! - The canvas reads a pane-sized texture, so a mask's blur and a heal stop
//!   at the pane's edge. This render works in tiles with a margin wide
//!   enough for both, so it has no seams.
//! - The overlay of a stroke being drawn and the display transform are not
//!   part of the picture, and are left out.

use crate::renderer::{
    Adjustment, Blend, ComputedMask, FinishKind, Layer, Mark, MaskBand, Pane, Segment,
    MAX_COMPUTED_MASKS, MAX_FINISH_STEPS, MAX_HEALED_LAYERS, MAX_HEALS, MAX_LAYERS, MAX_MASKS,
};
use rayon::prelude::*;

/// The samples of every tile channel of a document at full resolution, as
/// the canvas holds them: each channel divided by the value the canvas's 1
/// stands for.
pub trait SampleReader: Sync {
    /// The channels each pixel has.
    fn channels(&self) -> usize;
    /// The samples of `rect` (x, y, width and height in document pixels),
    /// interleaved by pixel, row after row.
    fn read(&self, rect: [u32; 4]) -> Result<Vec<f32>, String>;
}

/// A finished picture: red, green and blue interleaved, row after row.
#[derive(Clone, Debug, PartialEq)]
pub struct CpuImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<f32>,
}

/// The side of the square tiles the render works in.
const TILE: u32 = 256;

/// Rec. 709 luminance, as the shaders weigh it.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Renders `rect` of the document (x, y, width and height) as `pane`
/// describes it, Finish steps included. `document` is the document's width
/// and height. `stop` is asked between tiles; when it says to stop, the
/// render ends with an error. `progress` hears the share of tiles done.
pub fn render(
    pane: &Pane,
    reader: &dyn SampleReader,
    document: (u32, u32),
    rect: [u32; 4],
    stop: &(dyn Fn() -> bool + Sync),
    progress: &(dyn Fn(f32) + Sync),
) -> Result<CpuImage, String> {
    let [x0, y0, width, height] = rect;
    if width == 0 || height == 0 {
        return Err("the picture to render is empty".to_string());
    }
    let plan = Plan::new(pane, reader.channels());
    let margin = plan.margin();
    let tiles: Vec<[u32; 4]> = (0..height.div_ceil(TILE))
        .flat_map(|row| {
            (0..width.div_ceil(TILE)).map(move |column| {
                let x = x0 + column * TILE;
                let y = y0 + row * TILE;
                [x, y, TILE.min(x0 + width - x), TILE.min(y0 + height - y)]
            })
        })
        .collect();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let rendered: Vec<(usize, Vec<f32>)> = tiles
        .par_iter()
        .enumerate()
        .map(|(index, tile)| {
            if stop() {
                return Err("stopped".to_string());
            }
            let pixels = plan.render_tile(pane, reader, document, *tile, margin)?;
            let finished = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            progress(finished as f32 / tiles.len() as f32);
            Ok((index, pixels))
        })
        .collect::<Result<_, String>>()?;
    let mut rgb = vec![0.0; width as usize * height as usize * 3];
    for (index, pixels) in rendered {
        let [x, y, w, _] = tiles[index];
        for (row, line) in pixels.chunks(w as usize * 3).enumerate() {
            let start = ((y - y0) as usize + row) * width as usize + (x - x0) as usize;
            rgb[start * 3..start * 3 + line.len()].copy_from_slice(line);
        }
    }
    Ok(CpuImage { width, height, rgb })
}

/// The combined picture of `rect`, before the Finish steps, as the canvas's
/// combined texture holds it: what [`crate::Renderer::sample_picture`]
/// reads back, for comparing the two renders.
pub fn render_combined(
    pane: &Pane,
    reader: &dyn SampleReader,
    document: (u32, u32),
    rect: [u32; 4],
) -> Result<Vec<f32>, String> {
    let plan = Plan::new(pane, reader.channels());
    let region = Region::around(rect, plan.margin(), document);
    let samples = reader.read(region.rect())?;
    let masks = plan.masks(pane, &region);
    let combined = plan.combined(pane, &region, &samples, &masks);
    Ok(region.cut(&combined, rect, 3))
}

/// Applies the Finish steps that map each pixel alone to `rgb`, red, green
/// and blue interleaved: every step but a heal, each in full where its mask
/// is unknown. For a histogram of samples, not for an export.
pub fn finish_samples(pane: &Pane, rgb: &mut [f32]) {
    let steps = &pane.finish[..pane.finish.len().min(MAX_FINISH_STEPS)];
    for pixel in rgb.as_chunks_mut::<3>().0 {
        let mut color = *pixel;
        for step in steps {
            if !matches!(step.kind, FinishKind::Heal { .. }) {
                let applies = step.opacity.clamp(0.0, 1.0);
                color = mix3(color, adjust_finish(pane, color, step), applies);
            }
        }
        *pixel = color;
    }
}

/// The part of the document a tile is drawn from: the tile and its margin,
/// kept inside the document.
struct Region {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl Region {
    fn around(rect: [u32; 4], margin: u32, document: (u32, u32)) -> Self {
        let x = rect[0].saturating_sub(margin);
        let y = rect[1].saturating_sub(margin);
        let right = (rect[0] + rect[2] + margin).min(document.0);
        let bottom = (rect[1] + rect[3] + margin).min(document.1);
        Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }

    fn rect(&self) -> [u32; 4] {
        [self.x, self.y, self.width, self.height]
    }

    fn pixels(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// The index of a pixel given in region coordinates, or `None` outside.
    fn at(&self, x: i64, y: i64) -> Option<usize> {
        (x >= 0 && y >= 0 && x < i64::from(self.width) && y < i64::from(self.height))
            .then(|| y as usize * self.width as usize + x as usize)
    }

    /// The values of `rect`, in document pixels, from a region-sized buffer
    /// with `channels` values per pixel.
    fn cut(&self, values: &[f32], rect: [u32; 4], channels: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(rect[2] as usize * rect[3] as usize * channels);
        for row in 0..rect[3] {
            let y = (rect[1] + row - self.y) as usize;
            let x = (rect[0] - self.x) as usize;
            let start = (y * self.width as usize + x) * channels;
            out.extend_from_slice(&values[start..start + rect[2] as usize * channels]);
        }
        out
    }
}

/// What the shaders' globals hold, worked out once for a pane: which
/// stretch row each layer reads, and which heals apply.
struct Plan {
    channels: usize,
    layers: Vec<Layer>,
    /// Per layer: the row of the stretch tables its band is stretched by.
    rows: Vec<usize>,
    /// Per layer: its place among the healed layers, if it has heals.
    healed_place: Vec<Option<usize>>,
    /// Per healed place: the layer's band, and the band blended into it.
    healed: Vec<(usize, Option<(usize, f32)>)>,
    /// The heals drawn, in order: layer, whether before the stretch, mask,
    /// shift, and offset in pixels.
    heals: Vec<(usize, bool, usize, f32, [i64; 2])>,
    computed: Vec<ComputedMask>,
    mask_count: usize,
}

impl Plan {
    fn new(pane: &Pane, channels: usize) -> Self {
        let layers = pane.layers[..pane.layers.len().min(MAX_LAYERS)].to_vec();
        let mut row = 0;
        let rows = layers
            .iter()
            .map(|layer| {
                let this = row;
                if layer.mark == Mark::Layer && layer.adjustment.is_none() {
                    row += 1;
                }
                this
            })
            .collect();
        let mask_count = pane.masks.len().min(MAX_MASKS);
        // As `write_globals` places heals: a heal on a layer with no place
        // left, or past the most heals, is not drawn.
        let mut healed_layers: Vec<usize> = Vec::new();
        let mut healed_place = vec![None; layers.len()];
        let mut healed = Vec::new();
        let mut heals = Vec::new();
        for heal in pane.heals {
            let known = healed_layers.iter().position(|layer| *layer == heal.layer);
            let Some(layer) = layers
                .get(heal.layer)
                .filter(|layer| layer.mark == Mark::Layer && layer.adjustment.is_none())
            else {
                continue;
            };
            if heals.len() == MAX_HEALS
                || heal.mask >= mask_count
                || (known.is_none() && healed_layers.len() == MAX_HEALED_LAYERS)
            {
                continue;
            }
            let place = known.unwrap_or_else(|| {
                healed_layers.push(heal.layer);
                healed.push((
                    layer.band.min(channels.saturating_sub(1)),
                    layer.blended.map(|(band, amount)| {
                        (band.min(channels.saturating_sub(1)), amount.clamp(0.0, 1.0))
                    }),
                ));
                healed_layers.len() - 1
            });
            healed_place[heal.layer] = Some(place);
            heals.push((
                heal.layer,
                heal.before_stretch,
                heal.mask,
                heal.shift,
                [heal.offset[0].round() as i64, heal.offset[1].round() as i64],
            ));
        }
        Self {
            channels,
            layers,
            rows,
            healed_place,
            healed,
            heals,
            computed: pane.computed[..pane.computed.len().min(MAX_COMPUTED_MASKS)].to_vec(),
            mask_count,
        }
    }

    /// How far outside a tile its pixels can read: the reach of a mask's
    /// blur, plus the offsets of the heals, which can read through one
    /// another.
    fn margin(&self) -> u32 {
        let blur = self
            .computed
            .iter()
            .map(|mask| (mask.smoothing.max(0.0) * 3.0).ceil() as u32 + 2)
            .max()
            .unwrap_or(0);
        let reach = |offset: [i64; 2]| offset[0].unsigned_abs().max(offset[1].unsigned_abs());
        let layer_heals: u64 = self.heals.iter().map(|heal| reach(heal.4)).sum();
        blur + 2 * layer_heals as u32
    }

    fn render_tile(
        &self,
        pane: &Pane,
        reader: &dyn SampleReader,
        document: (u32, u32),
        tile: [u32; 4],
        margin: u32,
    ) -> Result<Vec<f32>, String> {
        let finish_reach = pane
            .finish
            .iter()
            .filter_map(|step| match step.kind {
                FinishKind::Heal { offset, .. } => {
                    Some(offset[0].abs().max(offset[1].abs()).ceil() as u32)
                }
                _ => None,
            })
            .sum::<u32>();
        let region = Region::around(tile, margin + finish_reach, document);
        let samples = reader.read(region.rect())?;
        if samples.len() < region.pixels() * self.channels {
            return Err("the store gave fewer samples than asked for".to_string());
        }
        let masks = self.masks(pane, &region);
        let combined = self.combined(pane, &region, &samples, &masks);
        let mut out = Vec::with_capacity(tile[2] as usize * tile[3] as usize * 3);
        for row in 0..tile[3] {
            for column in 0..tile[2] {
                let x = i64::from(tile[0] + column - region.x);
                let y = i64::from(tile[1] + row - region.y);
                out.extend(finish_fragment(pane, &region, &combined, &masks, x, y));
            }
        }
        Ok(out)
    }

    /// Every painted mask over the region: each stroke segment drawn in
    /// order over what the ones before it left, as the stroke pass blends.
    fn masks(&self, pane: &Pane, region: &Region) -> Vec<Vec<f32>> {
        pane.masks[..self.mask_count]
            .iter()
            .map(|segments| {
                let mut mask = vec![0.0_f32; region.pixels()];
                for segment in segments {
                    draw_segment(&mut mask, region, segment);
                }
                mask
            })
            .collect()
    }

    /// The combined picture over the region, before the Finish steps.
    fn combined(
        &self,
        pane: &Pane,
        region: &Region,
        samples: &[f32],
        masks: &[Vec<f32>],
    ) -> Vec<f32> {
        let smoothed = self.smoothed(pane, region, samples);
        let healed = self.healed_samples(region, samples, masks);
        let context = Context {
            pane,
            plan: self,
            region,
            samples,
            masks,
            smoothed: &smoothed,
            healed: &healed,
        };
        let mut out = vec![0.0; region.pixels() * 3];
        out.par_chunks_mut(region.width as usize * 3)
            .enumerate()
            .for_each(|(y, line)| {
                for (x, pixel) in line.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    *pixel = context.tile_fragment(x as i64, y as i64);
                }
            });
        out
    }

    /// Each smoothed computed mask over the region, blurred across and then
    /// down, as the canvas's two blur passes do. `None` for a mask that is
    /// not smoothed.
    fn smoothed(&self, pane: &Pane, region: &Region, samples: &[f32]) -> Vec<Option<Vec<f32>>> {
        self.computed
            .iter()
            .enumerate()
            .map(|(index, mask)| {
                if mask.smoothing <= 0.0 {
                    return None;
                }
                let shape: Vec<f32> = (0..region.pixels())
                    .map(|at| self.computed_shape(pane, samples, index, at))
                    .collect();
                let across = blur(&shape, region, mask.smoothing, [1, 0]);
                Some(blur(&across, region, mask.smoothing, [0, 1]))
            })
            .collect()
    }

    /// Per healed place, the layer's samples with its heals before the
    /// stretch applied: `healed_sample` at every pixel of the region.
    fn healed_samples(
        &self,
        region: &Region,
        samples: &[f32],
        masks: &[Vec<f32>],
    ) -> Vec<Vec<f32>> {
        self.healed
            .iter()
            .enumerate()
            .map(|(place, (band, blended))| {
                // The layer's samples, unhealed: the healed-layers pass.
                let raw: Vec<f32> = (0..region.pixels())
                    .map(|at| band_sample(samples, self.channels, at, *band, *blended))
                    .collect();
                let layer = self
                    .healed_place
                    .iter()
                    .position(|known| *known == Some(place))
                    .expect("a healed place has its layer");
                let mut healed = raw.clone();
                for (at, value) in healed.iter_mut().enumerate() {
                    let (x, y) = (
                        (at % region.width as usize) as i64,
                        (at / region.width as usize) as i64,
                    );
                    for (heal_layer, before, mask, shift, offset) in &self.heals {
                        if *heal_layer != layer || !*before {
                            continue;
                        }
                        if let Some(source) = region.at(x + offset[0], y + offset[1]) {
                            let copy = raw[source] + shift;
                            *value = mix(*value, copy, masks[*mask][at]);
                        }
                    }
                }
                healed
            })
            .collect()
    }

    /// `computed_shape`: the product of one or two stretched bands, shaped.
    fn computed_shape(&self, pane: &Pane, samples: &[f32], index: usize, at: usize) -> f32 {
        let mask = &self.computed[index];
        let mut product = 1.0;
        for band in mask.bands.iter().flatten() {
            product *= self.mask_band(pane, samples, at, band);
        }
        product = product.clamp(0.0, 1.0);
        if mask.power_of_inverse {
            product = product.powf(1.0 - product);
        }
        product.powf(mask.reach.clamp(0.05, 20.0))
    }

    fn mask_band(&self, pane: &Pane, samples: &[f32], at: usize, band: &MaskBand) -> f32 {
        let last = self.channels.saturating_sub(1);
        let sample = band_sample(
            samples,
            self.channels,
            at,
            band.band.min(last),
            band.blended
                .map(|(other, amount)| (other.min(last), amount.clamp(0.0, 1.0))),
        );
        stretched(pane, sample, band.stretch_row.min(MAX_LAYERS - 1))
    }
}

/// What one pixel of the layer pass reads.
struct Context<'a> {
    pane: &'a Pane<'a>,
    plan: &'a Plan,
    region: &'a Region,
    samples: &'a [f32],
    masks: &'a [Vec<f32>],
    smoothed: &'a [Option<Vec<f32>>],
    healed: &'a [Vec<f32>],
}

impl Context<'_> {
    fn mask(&self, mask: Option<usize>, at: usize) -> f32 {
        mask.filter(|mask| *mask < self.masks.len())
            .map_or(1.0, |mask| self.masks[mask][at])
    }

    /// `computed_mask`.
    fn computed_mask(&self, index: usize, at: usize) -> f32 {
        let mask = &self.plan.computed[index];
        let mut value = match &self.smoothed[index] {
            Some(smoothed) => smoothed[at],
            None => self.plan.computed_shape(self.pane, self.samples, index, at),
        };
        if let Some(added) = mask.added.filter(|mask| *mask < self.masks.len()) {
            value = value.max(self.masks[added][at]);
        }
        if let Some(removed) = mask.removed.filter(|mask| *mask < self.masks.len()) {
            value *= 1.0 - self.masks[removed][at];
        }
        if mask.inverted {
            value = 1.0 - value;
        }
        value
    }

    /// `healed_value`: a healed layer's stretched value, with its heals
    /// after the stretch applied.
    fn healed_value(&self, index: usize, place: usize, x: i64, y: i64) -> f32 {
        let layer = &self.plan.layers[index];
        let row = self.plan.rows[index];
        let healed = &self.healed[place];
        let at = self.region.at(x, y).expect("a pixel of the region");
        let stretch = |sample: f32| {
            contrast_curve(
                stretched(self.pane, sample, row),
                layer.curve.clamp(-1.0, 1.0),
            )
        };
        let mut value = stretch(healed[at]);
        for (heal_layer, before, mask, shift, offset) in &self.plan.heals {
            if *heal_layer != index || *before {
                continue;
            }
            if let Some(source) = self.region.at(x + offset[0], y + offset[1]) {
                let copy = stretch(healed[source]) + shift;
                value = mix(value, copy, self.masks[*mask][at]);
            }
        }
        value
    }

    /// `tile_fragment`: the layers blended at one pixel.
    fn tile_fragment(&self, x: i64, y: i64) -> [f32; 3] {
        let at = self.region.at(x, y).expect("a pixel of the region");
        let mut color = [0.0_f32; 3];
        let mut beneath = [0.0_f32; 3];
        let mut waiting = false;
        let mut own = [0.0_f32; 3];
        let mut own_cover = 0.0;
        let mut own_mode = Blend::Normal;
        for (index, layer) in self.plan.layers.iter().enumerate() {
            let mut cover = layer.opacity.clamp(0.0, 1.0);
            cover *= self.mask(layer.mask, at);
            cover *= self.mask(layer.clip, at);
            if let Some(computed) = layer
                .computed
                .filter(|mask| *mask < self.plan.computed.len())
            {
                cover *= self.computed_mask(computed, at);
            }
            let adjustment = (layer.mark == Mark::Layer)
                .then_some(layer.adjustment)
                .flatten();
            if let Some(adjustment) = adjustment
                && layer.own
            {
                if waiting {
                    own = mix3(own, adjusted(own, adjustment), cover);
                }
                continue;
            }
            if waiting {
                color = mix3(color, blend(own_mode, color, own), own_cover);
                waiting = false;
            }
            match layer.mark {
                Mark::Solid => {
                    own = layer.color;
                    own_cover = cover;
                    own_mode = layer.blend;
                    waiting = true;
                    continue;
                }
                Mark::GroupEnd => {
                    own = color;
                    own_cover = cover;
                    own_mode = layer.blend;
                    waiting = true;
                    color = beneath;
                    continue;
                }
                Mark::GroupBegin => {
                    beneath = color;
                    color = [0.0; 3];
                    continue;
                }
                Mark::Layer => {}
            }
            if let Some(adjustment) = adjustment {
                color = mix3(color, adjusted(color, adjustment), cover);
                continue;
            }
            let value = match self.plan.healed_place[index] {
                Some(place) => self.healed_value(index, place, x, y),
                None => {
                    let last = self.plan.channels.saturating_sub(1);
                    let sample = band_sample(
                        self.samples,
                        self.plan.channels,
                        at,
                        layer.band.min(last),
                        layer
                            .blended
                            .map(|(band, amount)| (band.min(last), amount.clamp(0.0, 1.0))),
                    );
                    contrast_curve(
                        stretched(self.pane, sample, self.plan.rows[index]),
                        layer.curve.clamp(-1.0, 1.0),
                    )
                }
            };
            own = layer.color.map(|channel| channel * value);
            own_cover = cover;
            own_mode = layer.blend;
            waiting = true;
        }
        if waiting {
            color = mix3(color, blend(own_mode, color, own), own_cover);
        }
        color
    }
}

/// `finish_fragment`: the Finish steps at one pixel of the region.
fn finish_fragment(
    pane: &Pane,
    region: &Region,
    combined: &[f32],
    masks: &[Vec<f32>],
    x: i64,
    y: i64,
) -> [f32; 3] {
    let steps = &pane.finish[..pane.finish.len().min(MAX_FINISH_STEPS)];
    let at = region.at(x, y).expect("a pixel of the region");
    let coverage = |mask: Option<usize>, at: usize| {
        mask.filter(|mask| *mask < masks.len())
            .map_or(1.0, |mask| masks[mask][at])
    };
    let color_at = |at: usize| [combined[at * 3], combined[at * 3 + 1], combined[at * 3 + 2]];
    // `before`: the color at a pixel after the steps before `upto`, heals
    // left out.
    let before = |at: usize, upto: usize| {
        let mut color = color_at(at);
        for step in &steps[..upto] {
            if !matches!(step.kind, FinishKind::Heal { .. }) {
                let applies = coverage(step.mask, at) * step.opacity.clamp(0.0, 1.0);
                color = mix3(color, adjust_finish(pane, color, step), applies);
            }
        }
        color
    };
    let mut color = color_at(at);
    for (index, step) in steps.iter().enumerate() {
        let applies = coverage(step.mask, at) * step.opacity.clamp(0.0, 1.0);
        match step.kind {
            FinishKind::Heal { offset, shift } => {
                let source = region.at(x + offset[0].round() as i64, y + offset[1].round() as i64);
                if let Some(source) = source {
                    let copied = before(source, index);
                    let healed = [
                        copied[0] + shift[0],
                        copied[1] + shift[1],
                        copied[2] + shift[2],
                    ];
                    color = mix3(color, healed, applies);
                }
            }
            _ => color = mix3(color, adjust_finish(pane, color, step), applies),
        }
    }
    color
}

/// `adjust` in `finish.wgsl`: a Finish step that maps a pixel alone.
fn adjust_finish(pane: &Pane, color: [f32; 3], step: &crate::FinishStep) -> [f32; 3] {
    match step.kind {
        FinishKind::Curves => color.map(|channel| contrast_curve(channel, step.amount)),
        FinishKind::Saturation => {
            let luminance = luma(color);
            let mixed = mix3([luminance; 3], color, step.amount);
            mixed.map(|channel| channel.max(0.0))
        }
        FinishKind::Stretch { table, luminance } => stretch_picture(pane, color, table, luminance),
        FinishKind::Tone {
            table,
            vibrance,
            saturation,
        } => colorfulness(
            stretch_picture(pane, color, table, true),
            vibrance.clamp(-1.0, 1.0),
            saturation.clamp(-1.0, 1.0),
        ),
        FinishKind::Heal { .. } => color,
    }
}

/// `stretch_picture`.
fn stretch_picture(pane: &Pane, color: [f32; 3], table: usize, luminance: bool) -> [f32; 3] {
    let Some(rows) = pane.finish_stretches.get(table.min(MAX_FINISH_STEPS - 1)) else {
        return color;
    };
    if !luminance {
        return [
            table_value(&rows[0], color[0]),
            table_value(&rows[1], color[1]),
            table_value(&rows[2], color[2]),
        ];
    }
    let luminance = luma(color);
    let mapped = table_value(&rows[0], luminance);
    if luminance <= 1.0e-8 {
        return [mapped; 3];
    }
    let mut scale = mapped / luminance;
    let maximum = color[0].max(color[1]).max(color[2]).max(0.0);
    if maximum * scale > 1.0 {
        scale = 1.0 / maximum;
    }
    color.map(|channel| (channel * scale).clamp(0.0, 1.0))
}

/// `colorfulness`.
pub fn colorfulness(color: [f32; 3], vibrance: f32, saturation: f32) -> [f32; 3] {
    let luminance = luma(color);
    let high = color[0].max(color[1]).max(color[2]);
    let low = color[0].min(color[1]).min(color[2]);
    let vivid = if high > 1.0e-6 {
        ((high - low) / high).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let gain = (1.0 + saturation) * (1.0 + vibrance * (1.0 - vivid));
    color.map(|channel| (luminance + (channel - luminance) * gain).max(0.0))
}

/// `blend`.
fn blend(mode: Blend, below: [f32; 3], layer: [f32; 3]) -> [f32; 3] {
    match mode {
        Blend::Normal => layer,
        Blend::Screen => {
            let mut out = [0.0; 3];
            for channel in 0..3 {
                out[channel] = 1.0
                    - (1.0 - below[channel].clamp(0.0, 1.0))
                        * (1.0 - layer[channel].clamp(0.0, 1.0));
            }
            out
        }
        Blend::Add => [
            below[0] + layer[0],
            below[1] + layer[1],
            below[2] + layer[2],
        ],
        Blend::Multiply => [
            below[0] * layer[0],
            below[1] * layer[1],
            below[2] * layer[2],
        ],
        Blend::Lighten => [
            below[0].max(layer[0]),
            below[1].max(layer[1]),
            below[2].max(layer[2]),
        ],
        Blend::Darken => [
            below[0].min(layer[0]),
            below[1].min(layer[1]),
            below[2].min(layer[2]),
        ],
        Blend::Luminance => {
            let shift = luma(layer) - luma(below);
            below.map(|channel| (channel + shift).max(0.0))
        }
    }
}

/// `adjusted`: an adjustment layer.
fn adjusted(color: [f32; 3], (kind, amount): (Adjustment, f32)) -> [f32; 3] {
    match kind {
        Adjustment::Exposure => color.map(|channel| channel * amount.exp2()),
        Adjustment::Curves => color.map(|channel| contrast_curve(channel, amount)),
        Adjustment::Saturation => {
            let luminance = luma(color);
            mix3([luminance; 3], color, amount).map(|channel| channel.max(0.0))
        }
    }
}

/// `contrast_curve` in `globals.wgsl`.
pub fn contrast_curve(x: f32, amount: f32) -> f32 {
    let clamped = x.clamp(0.0, 1.0);
    if amount >= 0.0 {
        mix(
            clamped,
            clamped * clamped * (3.0 - 2.0 * clamped),
            amount.min(1.0),
        )
    } else {
        mix(clamped, 0.5, (-amount).min(1.0) * 0.5)
    }
}

/// `band_sample`: a band's sample, with a second band blended in.
fn band_sample(
    samples: &[f32],
    channels: usize,
    at: usize,
    band: usize,
    blended: Option<(usize, f32)>,
) -> f32 {
    let sample = samples[at * channels + band];
    match blended {
        Some((other, amount)) => mix(sample, samples[at * channels + other], amount),
        None => sample,
    }
}

/// `stretched`: a layer's stretch-table row at a sample.
fn stretched(pane: &Pane, sample: f32, row: usize) -> f32 {
    pane.stretches
        .get(row)
        .map_or(sample.clamp(0.0, 1.0), |row| table_value(row, sample))
}

/// One row of a stretch table at a value from 0 to 1, indexed by the
/// value's square root and interpolated, as the shaders sample a row.
fn table_value(row: &[f32], value: f32) -> f32 {
    if row.is_empty() {
        return value.clamp(0.0, 1.0);
    }
    let place = value.clamp(0.0, 1.0).sqrt() * (row.len() - 1) as f32;
    let low = (place.floor() as usize).min(row.len() - 1);
    let high = (low + 1).min(row.len() - 1);
    mix(row[low], row[high], place - low as f32)
}

/// One brush segment over a mask, as the stroke pass draws it: a capsule
/// whose coverage falls off from `radius * hardness` to `radius`, blended
/// over the mask with its value.
fn draw_segment(mask: &mut [f32], region: &Region, segment: &Segment) {
    let radius = segment.radius.max(0.0);
    let left = segment.from[0].min(segment.to[0]) - radius;
    let right = segment.from[0].max(segment.to[0]) + radius;
    let top = segment.from[1].min(segment.to[1]) - radius;
    let bottom = segment.from[1].max(segment.to[1]) + radius;
    let x0 = ((left - region.x as f32).floor().max(0.0)) as u32;
    let y0 = ((top - region.y as f32).floor().max(0.0)) as u32;
    let x1 = ((right - region.x as f32).ceil().max(0.0) as u32).min(region.width);
    let y1 = ((bottom - region.y as f32).ceil().max(0.0) as u32).min(region.height);
    let delta = [
        segment.to[0] - segment.from[0],
        segment.to[1] - segment.from[1],
    ];
    let length_squared = (delta[0] * delta[0] + delta[1] * delta[1]).max(0.0001);
    for y in y0..y1 {
        for x in x0..x1 {
            // The pixel's center in document pixels.
            let doc = [(region.x + x) as f32 + 0.5, (region.y + y) as f32 + 0.5];
            let along = (((doc[0] - segment.from[0]) * delta[0]
                + (doc[1] - segment.from[1]) * delta[1])
                / length_squared)
                .clamp(0.0, 1.0);
            let nearest = [
                segment.from[0] + delta[0] * along,
                segment.from[1] + delta[1] * along,
            ];
            let distance = ((doc[0] - nearest[0]).powi(2) + (doc[1] - nearest[1]).powi(2)).sqrt();
            let coverage = 1.0 - smoothstep(radius * segment.hardness, radius, distance);
            if coverage <= 0.0 {
                continue;
            }
            let at = y as usize * region.width as usize + x as usize;
            mask[at] = mask[at] * (1.0 - coverage) + segment.value * coverage;
        }
    }
}

/// `blurred`: a Gaussian blur in one direction, of 33 taps spaced
/// 3σ/16 apart, each read between pixels by linear interpolation and
/// clamped at the edge, as the canvas's sampler reads.
fn blur(values: &[f32], region: &Region, sigma: f32, direction: [i64; 2]) -> Vec<f32> {
    let spacing = sigma * 3.0 / 16.0;
    let taps: Vec<(f32, f32)> = (-16..=16)
        .map(|tap| {
            let offset = tap as f32 * spacing;
            (offset, (-offset * offset / (2.0 * sigma * sigma)).exp())
        })
        .collect();
    let weights: f32 = taps.iter().map(|(_, weight)| weight).sum();
    let (width, height) = (region.width as i64, region.height as i64);
    let read = |x: f32, y: f32| {
        // The sampler's texel centers are at whole pixels here.
        let x = x.clamp(0.0, (width - 1) as f32);
        let y = y.clamp(0.0, (height - 1) as f32);
        let (x0, y0) = (x.floor() as i64, y.floor() as i64);
        let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let value = |x: i64, y: i64| values[(y * width + x) as usize];
        mix(
            mix(value(x0, y0), value(x1, y0), tx),
            mix(value(x0, y1), value(x1, y1), tx),
            ty,
        )
    };
    let mut out = vec![0.0; values.len()];
    out.par_chunks_mut(width as usize)
        .enumerate()
        .for_each(|(y, line)| {
            for (x, value) in line.iter_mut().enumerate() {
                let mut sum = 0.0;
                for (offset, weight) in &taps {
                    sum += weight
                        * read(
                            x as f32 + direction[0] as f32 * offset,
                            y as f32 + direction[1] as f32 * offset,
                        );
                }
                *value = sum / weights;
            }
        });
    out
}

fn luma(color: [f32; 3]) -> f32 {
    LUMA[0] * color[0] + LUMA[1] * color[1] + LUMA[2] * color[2]
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [mix(a[0], b[0], t), mix(a[1], b[1], t), mix(a[2], b[2], t)]
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::{FinishStep, LayerHeal, STRETCH_TABLE_WIDTH};

    /// A document of two bands: a smooth gradient and a pattern of dots.
    struct Synthetic {
        width: u32,
    }

    impl Synthetic {
        fn sample(&self, x: u32, y: u32, band: usize) -> f32 {
            match band {
                0 => (x as f32 / self.width as f32) * 0.8 + 0.05,
                _ => {
                    if (x / 7 + y / 5).is_multiple_of(3) {
                        0.9
                    } else {
                        0.1 + 0.001 * (y % 50) as f32
                    }
                }
            }
        }
    }

    impl SampleReader for Synthetic {
        fn channels(&self) -> usize {
            2
        }

        fn read(&self, rect: [u32; 4]) -> Result<Vec<f32>, String> {
            let [x0, y0, width, height] = rect;
            let mut out = Vec::new();
            for y in y0..y0 + height {
                for x in x0..x0 + width {
                    out.extend([self.sample(x, y, 0), self.sample(x, y, 1)]);
                }
            }
            Ok(out)
        }
    }

    /// A row that leaves samples as they are.
    fn identity_row() -> Vec<f32> {
        let last = (STRETCH_TABLE_WIDTH - 1) as f32;
        (0..STRETCH_TABLE_WIDTH)
            .map(|index| (index as f32 / last).powi(2))
            .collect()
    }

    fn band_layer(band: usize, color: [f32; 3], blend: Blend) -> Layer {
        Layer {
            mark: Mark::Layer,
            clip: None,
            computed: None,
            band,
            color,
            opacity: 1.0,
            blend,
            curve: 0.0,
            blended: None,
            mask: None,
            adjustment: None,
            own: false,
        }
    }

    fn pane<'a>(
        layers: &'a [Layer],
        stretches: &'a [Vec<f32>],
        finish: &'a [FinishStep],
        masks: &'a [Vec<Segment>],
        computed: &'a [ComputedMask],
        heals: &'a [LayerHeal],
    ) -> Pane<'a> {
        Pane {
            layers,
            stretches,
            finish,
            finish_stretches: &[],
            masks,
            computed,
            heals,
            viewport: [0, 0, 1, 1],
            scissor: [0, 0, 1, 1],
        }
    }

    fn no_stop() -> bool {
        false
    }

    #[test]
    fn identity_stretches_show_the_samples_in_their_colors() {
        let source = Synthetic { width: 300 };
        let layers = [
            band_layer(0, [1.0, 0.0, 0.0], Blend::Add),
            band_layer(1, [0.0, 1.0, 1.0], Blend::Add),
        ];
        let stretches = [identity_row(), identity_row()];
        let pane = pane(&layers, &stretches, &[], &[], &[], &[]);
        let image = render(
            &pane,
            &source,
            (300, 200),
            [10, 20, 40, 30],
            &no_stop,
            &|_| {},
        )
        .unwrap();
        assert_eq!((image.width, image.height), (40, 30));
        for (index, pixel) in image.rgb.as_chunks::<3>().0.iter().enumerate() {
            let (x, y) = (10 + index as u32 % 40, 20 + index as u32 / 40);
            assert!((pixel[0] - source.sample(x, y, 0)).abs() < 1e-4);
            assert!((pixel[1] - source.sample(x, y, 1)).abs() < 1e-4);
            assert_eq!(pixel[1], pixel[2]);
        }
    }

    #[test]
    fn a_painted_mask_limits_its_layer() {
        let source = Synthetic { width: 300 };
        let mut masked = band_layer(1, [0.0, 1.0, 0.0], Blend::Normal);
        masked.mask = Some(0);
        let layers = [band_layer(0, [1.0, 0.0, 0.0], Blend::Normal), masked];
        let stretches = [identity_row(), identity_row()];
        let masks = [vec![Segment {
            from: [50.0, 50.0],
            to: [50.0, 50.0],
            radius: 10.0,
            hardness: 1.0,
            value: 1.0,
            _pad: 0.0,
        }]];
        let pane = pane(&layers, &stretches, &[], &masks, &[], &[]);
        let image = render(
            &pane,
            &source,
            (300, 200),
            [0, 0, 100, 100],
            &no_stop,
            &|_| {},
        )
        .unwrap();
        let at = |x: usize, y: usize| &image.rgb[(y * 100 + x) * 3..(y * 100 + x) * 3 + 3];
        // Inside the stroke the green layer covers the red one.
        assert_eq!(at(50, 50)[0], 0.0);
        assert!(at(50, 50)[1] > 0.0);
        // Outside it the red layer shows.
        assert!(at(10, 10)[0] > 0.0);
        assert_eq!(at(10, 10)[1], 0.0);
    }

    /// A picture drawn as one tile and drawn as many is the same picture:
    /// the margin covers a smoothed mask's blur and the heals.
    #[test]
    fn tiles_leave_no_seams() {
        let source = Synthetic { width: 700 };
        let mut masked = band_layer(1, [0.2, 0.6, 1.0], Blend::Screen);
        masked.computed = Some(0);
        let layers = [band_layer(0, [1.0, 0.3, 0.1], Blend::Normal), masked];
        let stretches = [identity_row(), identity_row()];
        let masks = [vec![Segment {
            from: [240.0, 240.0],
            to: [300.0, 270.0],
            radius: 30.0,
            hardness: 0.5,
            value: 1.0,
            _pad: 0.0,
        }]];
        let computed = [ComputedMask {
            bands: [
                Some(MaskBand {
                    band: 1,
                    blended: None,
                    stretch_row: 1,
                }),
                None,
            ],
            power_of_inverse: false,
            reach: 1.0,
            smoothing: 6.0,
            added: None,
            removed: None,
            inverted: false,
        }];
        let heals = [LayerHeal {
            layer: 0,
            before_stretch: true,
            offset: [-40.0, 12.0],
            shift: 0.0,
            mask: 0,
        }];
        let finish = [FinishStep {
            kind: FinishKind::Heal {
                offset: [25.0, -9.0],
                shift: [0.0; 3],
            },
            amount: 0.0,
            opacity: 1.0,
            mask: Some(0),
        }];
        let pane = pane(&layers, &stretches, &finish, &masks, &computed, &heals);
        // 520 pixels across is three tiles; 200 down is one.
        let whole = render(
            &pane,
            &source,
            (700, 600),
            [100, 150, 520, 200],
            &no_stop,
            &|_| {},
        )
        .unwrap();
        let plan = Plan::new(&pane, 2);
        let single = plan
            .render_tile(
                &pane,
                &source,
                (700, 600),
                [100, 150, 520, 200],
                plan.margin(),
            )
            .unwrap();
        let largest = whole
            .rgb
            .iter()
            .zip(&single)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert!(largest < 1e-5, "{largest}");
    }

    #[test]
    fn a_stop_ends_the_render() {
        let source = Synthetic { width: 300 };
        let layers = [band_layer(0, [1.0; 3], Blend::Normal)];
        let stretches = [identity_row()];
        let pane = pane(&layers, &stretches, &[], &[], &[], &[]);
        let stopped = render(
            &pane,
            &source,
            (300, 200),
            [0, 0, 300, 200],
            &|| true,
            &|_| {},
        );
        assert!(stopped.is_err());
    }
}
