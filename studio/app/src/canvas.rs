//! What the canvas is asked to draw, in the document's terms, and the Seiza
//! picture it becomes.
//!
//! A scene lists layers as the document has them: a layer shows an input's
//! band through a stretch the document resolves later, from each band's
//! analysis. [`picture`] turns a scene into a [`seiza_compose`] picture once
//! those stretches are known. Seiza draws that picture: on the GPU for the
//! canvas, and on the CPU for an export, the parity report and the
//! histogram.

use seiza_compose::picture::{
    self, Adjustment as SeizaAdjustment, Blend, FinishStep, InputRead, LayerHeal, LayerKind,
    Picture, Segment, Transfer,
};
use seiza_compose::render::{RenderOptions, Sink, Source};
use seiza_compose::space::WorkingSpace;
use seiza_compose::{Error, ReferenceRegion};
use std::sync::Mutex;

/// The space Studio composes a picture in: Rec. 2020's primaries on the
/// sRGB curve. It is wide enough that no color a display can show is lost
/// before the canvas or an export maps the picture to its destination.
pub const WORKING_SPACE: WorkingSpace = WorkingSpace::Rec2020;

/// The space a document's colors are in: Display P3, what Apple's
/// wide-gamut displays show, so any hue a layer is given is one the screen
/// can show.
pub const DOCUMENT_COLORS: WorkingSpace = WorkingSpace::DisplayP3;

/// A document color in the working space. A layer's color multiplies its
/// band's stretched values, and the layers add, so the color is converted
/// by the matrix alone: layers whose colors add up to white in Display P3
/// still add up to white, and a calibrated picture keeps its balance.
pub fn working_color(color: [f32; 3]) -> [f32; 3] {
    let matrix = DOCUMENT_COLORS.matrix_to(WORKING_SPACE);
    matrix.map(|row| (row[0] * color[0] + row[1] * color[1] + row[2] * color[2]).max(0.0))
}

/// What an entry in a scene's list of layers is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mark {
    /// A layer: one that shows a band, or an adjustment.
    #[default]
    Layer,
    /// The start of a group. The entries up to the group's end blend
    /// together on their own, starting from nothing.
    GroupBegin,
    /// The end of a group: the members' blend becomes one layer with this
    /// entry's opacity, blend and mask. Adjustments of the entry's own
    /// follow it, as they follow a layer.
    GroupEnd,
    /// A layer of one color everywhere, with no band: what a mask is drawn
    /// with, so the mask itself shows.
    Solid,
    /// A Finish step that changes the layers beneath it, as an index into
    /// the scene's steps.
    Step(usize),
}

/// What an adjustment layer does. Its amount is the layer's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adjustment {
    /// Brightens or darkens by the amount in stops.
    Exposure,
    /// A contrast curve. The amount runs from -1 to 1.
    Curves,
    /// Saturation. 1 leaves the color unchanged.
    Saturation,
    /// Green down toward the brighter of red and blue, from 0 to 1.
    GreenReduction,
}

/// One entry of a scene's layers: a band, stretched, colored and blended,
/// or an adjustment, or one end of a group.
#[derive(Clone, Copy, Debug)]
pub struct Layer {
    pub mark: Mark,
    /// A second mask that limits the layer: the mask of the layer it is
    /// clipped to, as an index into the scene's masks.
    pub clip: Option<usize>,
    /// A mask computed from bands that limits the layer, as an index into
    /// the scene's computed masks.
    pub computed: Option<usize>,
    /// The tile channel the layer shows.
    pub band: usize,
    pub color: [f32; 3],
    /// From 0 to 1. A hidden layer has 0.
    pub opacity: f32,
    pub blend: Blend,
    /// The layer's own contrast curve, from -1 to 1. 0 is none.
    pub curve: f32,
    /// A second tile channel blended into the first before the stretch, and
    /// how much of it, from 0 to 1: the band uncorrected and corrected in
    /// full, for a correction whose strength a person moves.
    pub blended: Option<(usize, f32)>,
    /// The painted mask that limits where the layer applies.
    pub mask: Option<usize>,
    /// For an adjustment: what it does, and by how much. `None` is a layer
    /// that shows a band.
    pub adjustment: Option<(Adjustment, f32)>,
    /// For an adjustment: whether it changes only the entry before it.
    pub own: bool,
    /// Whether `color` is in the working space already: a channel of an
    /// image a tool made from the picture, which goes back as it came.
    pub working: bool,
}

/// One band a computed mask reads, stretched.
#[derive(Clone, Copy, Debug)]
pub struct MaskBand {
    /// The tile channel read.
    pub band: usize,
    /// A second tile channel blended into the first, as for a layer.
    pub blended: Option<(usize, f32)>,
    /// The scene's stretch the band is read through, by its place among
    /// the scene's stretches.
    pub stretch_row: usize,
}

/// A mask computed at each pixel from the stretched samples of one or two
/// bands.
#[derive(Clone, Copy, Debug)]
pub struct ComputedMask {
    pub bands: [Option<MaskBand>; 2],
    /// Whether the product `p` becomes `p^(1-p)`.
    pub power_of_inverse: bool,
    /// The power the shaped product is raised to. 1 leaves it as it is.
    pub reach: f32,
    /// The standard deviation of the blur the mask is smoothed with, in
    /// document pixels. 0 is not smoothed.
    pub smoothing: f32,
    /// Strokes that set the mask to 1 under them.
    pub added: Option<usize>,
    /// Strokes that set the mask to 0 under them.
    pub removed: Option<usize>,
    /// Whether the mask is 1 minus all of the above.
    pub inverted: bool,
}

/// What a scene holds besides its stretches.
pub struct SceneParts<'a> {
    pub layers: &'a [Layer],
    pub computed: &'a [ComputedMask],
    pub masks: &'a [Vec<Segment>],
    pub heals: &'a [LayerHeal],
    pub finish: &'a [FinishStep],
    /// What the layers marked as steps do.
    pub steps: &'a [picture::FinishKind],
}

/// The scene as a Seiza picture, with `stretches` as its stretches in
/// order: one per layer that shows a band, then the ones only computed
/// masks read. A stretch the list does not have draws nothing.
pub fn picture(parts: &SceneParts, stretches: &[Transfer]) -> Picture {
    let stretch = |row: usize| {
        stretches
            .get(row)
            .cloned()
            .unwrap_or_else(|| Transfer::of([picture::TransferStage::Scale(0.0)]))
    };
    let mut row = 0;
    let layers = parts
        .layers
        .iter()
        .map(|layer| {
            let kind = match (layer.mark, layer.adjustment) {
                (Mark::GroupBegin, _) => LayerKind::GroupBegin,
                (Mark::GroupEnd, _) => LayerKind::GroupEnd,
                (Mark::Solid, _) => LayerKind::Solid {
                    color: working_color(layer.color),
                },
                // A step the list does not have changes nothing.
                (Mark::Step(index), _) => parts.steps.get(index).map_or(
                    LayerKind::Adjustment(SeizaAdjustment::Exposure(0.0)),
                    |kind| LayerKind::Step(kind.clone()),
                ),
                (Mark::Layer, Some((kind, amount))) => LayerKind::Adjustment(match kind {
                    Adjustment::Exposure => SeizaAdjustment::Exposure(amount),
                    Adjustment::Curves => SeizaAdjustment::Curves(amount.clamp(-1.0, 1.0)),
                    Adjustment::Saturation => SeizaAdjustment::Saturation(amount),
                    Adjustment::GreenReduction => {
                        SeizaAdjustment::GreenReduction(amount.clamp(0.0, 1.0))
                    }
                }),
                (Mark::Layer, None) => {
                    let transfer = stretch(row);
                    row += 1;
                    LayerKind::Input {
                        read: InputRead {
                            input: layer.band,
                            blended: layer
                                .blended
                                .map(|(band, share)| (band, share.clamp(0.0, 1.0))),
                            transfer,
                        },
                        color: if layer.working {
                            layer.color
                        } else {
                            working_color(layer.color)
                        },
                        curve: layer.curve.clamp(-1.0, 1.0),
                    }
                }
            };
            picture::Layer {
                kind,
                opacity: layer.opacity.clamp(0.0, 1.0),
                blend: layer.blend,
                mask: layer.mask,
                clip: layer.clip,
                computed: layer.computed,
                own: layer.own,
            }
        })
        .collect();
    let computed = parts
        .computed
        .iter()
        .map(|mask| picture::ComputedMask {
            reads: mask
                .bands
                .iter()
                .flatten()
                .map(|band| InputRead {
                    input: band.band,
                    blended: band
                        .blended
                        .map(|(other, share)| (other, share.clamp(0.0, 1.0))),
                    transfer: stretch(band.stretch_row),
                })
                .collect(),
            power_of_inverse: mask.power_of_inverse,
            reach: mask.reach.clamp(0.05, 20.0),
            smoothing: mask.smoothing.max(0.0),
            added: mask.added,
            removed: mask.removed,
            inverted: mask.inverted,
        })
        .collect();
    Picture {
        layers,
        computed,
        masks: parts.masks.to_vec(),
        heals: parts.heals.to_vec(),
        finish: parts.finish.to_vec(),
        space: WORKING_SPACE,
        // SDR; a scene gives the document's headroom.
        headroom: 0.0,
    }
}

/// The picture's samples `rgb`, red, green and blue interleaved, after the
/// Finish steps that change each pixel alone: what the histogram shows.
/// Masks and heals, which need the pixels around, are left out. Seiza's CPU
/// render applies the steps, as it does for an export, with the picture's
/// `headroom` in stops, so an HDR picture's samples keep what is above SDR
/// white.
pub fn finished_samples(finish: &[FinishStep], headroom: f32, rgb: &[f32]) -> Vec<f32> {
    let pixels = rgb.len() / 3;
    if pixels == 0 {
        return Vec::new();
    }
    let steps: Vec<FinishStep> = finish
        .iter()
        .filter(|step| !matches!(step.kind, picture::FinishKind::Heal { .. }))
        .map(|step| FinishStep {
            mask: None,
            ..step.clone()
        })
        .collect();
    // Each channel is an input, added in its own primary over black, which
    // gives back the samples as they are before the steps.
    let layers = (0..3)
        .map(|channel| {
            let mut color = [0.0; 3];
            color[channel] = 1.0;
            picture::Layer::added(InputRead::new(channel, Transfer::identity()), color)
        })
        .collect();
    let picture = Picture {
        layers,
        finish: steps,
        space: WORKING_SPACE,
        headroom,
        ..Picture::default()
    };
    let source = InterleavedSource { rgb, pixels };
    let region = ReferenceRegion {
        x: 0,
        y: 0,
        width: pixels,
        height: 1,
    };
    let sink = RowSink(Mutex::new(vec![f32::NAN; pixels * 3]));
    match seiza_compose::render::render(&picture, &source, region, &sink, &RenderOptions::default())
    {
        Ok(()) => sink.0.into_inner().unwrap_or_default(),
        Err(_) => rgb.to_vec(),
    }
}

/// The noise of the picture as the Finish steps `before` leave it, from
/// `samples`: `patches` square patches of RGB samples of the combined
/// picture, patch after patch. The median of each patch's measurement, so a
/// patch full of stars or nebula edges does not count for more. 0 when
/// there are no samples.
pub fn picture_noise(before: &[FinishStep], samples: &[f32], patches: usize) -> f32 {
    let pixels = samples.len() / 3 / patches.max(1);
    let size = (pixels as f64).sqrt() as usize;
    if size < 3 {
        return 0.0;
    }
    // Every fourth patch is plenty for a median, and keeps the CPU render
    // of the steps before small.
    let chosen: Vec<f32> = samples
        .chunks_exact(size * size * 3)
        .step_by(4)
        .flatten()
        .copied()
        .collect();
    // The sky's noise is far below SDR white, so the headroom does not
    // change it.
    let finished = finished_samples(before, 0.0, &chosen);
    let mut measured: Vec<f32> = finished
        .chunks_exact(size * size * 3)
        .map(|patch| seiza_compose::tone::luminance_noise(patch, size, size, WORKING_SPACE))
        .filter(|noise| noise.is_finite() && *noise > 0.0)
        .collect();
    if measured.is_empty() {
        return 0.0;
    }
    measured.sort_by(f32::total_cmp);
    measured[measured.len() / 2]
}

/// Interleaved RGB samples as three inputs one pixel high.
struct InterleavedSource<'a> {
    rgb: &'a [f32],
    pixels: usize,
}

impl Source for InterleavedSource<'_> {
    fn size(&self) -> (usize, usize) {
        (self.pixels, 1)
    }

    fn read(&self, input: usize, rect: ReferenceRegion) -> Result<Vec<f32>, Error> {
        Ok((rect.x..rect.x + rect.width)
            .map(|pixel| self.rgb[pixel * 3 + input])
            .collect())
    }
}

/// A picture one pixel high, kept whole.
struct RowSink(Mutex<Vec<f32>>);

impl Sink for RowSink {
    fn put(&self, rect: ReferenceRegion, rgb: &[f32]) -> Result<(), Error> {
        let mut out = self
            .0
            .lock()
            .map_err(|_| Error::Color("a tile's render panicked".into()))?;
        out[rect.x * 3..(rect.x + rect.width) * 3].copy_from_slice(rgb);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seiza_compose::picture::FinishKind;
    use seiza_compose::tone::Tone;

    fn band_layer(band: usize) -> Layer {
        Layer {
            mark: Mark::Layer,
            clip: None,
            computed: None,
            band,
            color: [1.0, 0.5, 0.25],
            opacity: 1.0,
            blend: Blend::Add,
            curve: 0.0,
            blended: None,
            mask: None,
            adjustment: None,
            own: false,
            working: false,
        }
    }

    #[test]
    fn each_band_layer_takes_the_next_stretch_and_masks_name_theirs() {
        let exposure = Layer {
            adjustment: Some((Adjustment::Exposure, 1.0)),
            ..band_layer(0)
        };
        let layers = [band_layer(3), exposure, band_layer(5)];
        let computed = [ComputedMask {
            bands: [
                Some(MaskBand {
                    band: 7,
                    blended: None,
                    stretch_row: 2,
                }),
                None,
            ],
            power_of_inverse: true,
            reach: 1.0,
            smoothing: 0.0,
            added: None,
            removed: None,
            inverted: false,
        }];
        let stretches: Vec<Transfer> = (1..=3)
            .map(|scale| Transfer::of([picture::TransferStage::Scale(scale as f32)]))
            .collect();
        let parts = SceneParts {
            layers: &layers,
            computed: &computed,
            masks: &[],
            heals: &[],
            finish: &[],
            steps: &[],
        };
        let picture = picture(&parts, &stretches);
        let LayerKind::Input { read, .. } = &picture.layers[0].kind else {
            panic!("a band layer");
        };
        assert_eq!((read.input, &read.transfer), (3, &stretches[0]));
        assert_eq!(
            picture.layers[1].kind,
            LayerKind::Adjustment(SeizaAdjustment::Exposure(1.0))
        );
        let LayerKind::Input { read, .. } = &picture.layers[2].kind else {
            panic!("a band layer");
        };
        assert_eq!((read.input, &read.transfer), (5, &stretches[1]));
        assert_eq!(picture.computed[0].reads[0].transfer, stretches[2]);
    }

    #[test]
    fn the_histogram_samples_follow_the_finish_steps() {
        let rgb = [0.1, 0.2, 0.3, 0.5, 0.5, 0.5];
        assert_eq!(finished_samples(&[], 0.0, &rgb), rgb);
        let brighter = [FinishStep {
            kind: FinishKind::Tone(Tone {
                exposure: 1.0,
                ..Tone::default()
            }),
            opacity: 1.0,
            // A mask is left out: the histogram is of the picture's tones.
            mask: Some(0),
        }];
        let finished = finished_samples(&brighter, 0.0, &rgb);
        assert!(finished[0] > rgb[0] * 1.5, "{finished:?}");
        assert!(finished[3] > 0.9, "{finished:?}");
    }

    #[test]
    fn the_picture_s_noise_is_the_median_of_its_patches() {
        // Nine patches of 32 × 32: noise of about 0.01, and a sharp
        // checkerboard in the fifth, which the median of every fourth
        // patch (the first, fifth and ninth) leaves out.
        let size = 32;
        let uniform = |index: u64| {
            // splitmix64, so neighboring pixels are independent.
            let mut z = index.wrapping_add(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            (z ^ (z >> 31)) as f32 / u64::MAX as f32
        };
        let mut samples = Vec::new();
        for patch in 0..9_u64 {
            for at in 0..(size * size) as u64 {
                let value = if patch == 4 {
                    if (at / size as u64 + at % size as u64).is_multiple_of(2) {
                        0.9
                    } else {
                        0.1
                    }
                } else {
                    let sum: f32 = (0..12)
                        .map(|draw| uniform((patch * 100_000 + at) * 12 + draw))
                        .sum();
                    0.3 + (sum - 6.0) * 0.01
                };
                samples.extend([value; 3]);
            }
        }
        let noise = picture_noise(&[], &samples, 9);
        assert!((noise - 0.01).abs() < 0.0015, "{noise}");
        assert_eq!(picture_noise(&[], &[], 4), 0.0);
    }
}
