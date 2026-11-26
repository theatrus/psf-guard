//! What a PSF Studio canvas draws, and the same picture rendered on the CPU.
//!
//! [`Pane`] describes one picture: its layers, their stretch tables, the
//! masks, heals and Finish steps. PSF Studio's GPU canvas
//! (`psf-studio-render`) draws a pane on screen. [`cpu`] renders the same
//! pane at full resolution in 32-bit floats, for an export and for PSF
//! Guard, which has no GPU canvas. Both follow the canvas's shaders, so the
//! two renders of one pane agree.

pub mod cpu;
pub mod histogram;
pub mod output_stretch;
pub mod tone;

use bytemuck::{Pod, Zeroable};

/// The most layers one frame combines.
pub const MAX_LAYERS: usize = 24;
/// The most Finish steps one frame applies.
pub const MAX_FINISH_STEPS: usize = 8;
/// The most masks one frame draws, besides the stroke overlay.
pub const MAX_MASKS: usize = 15;
/// The most heals of single layers one frame draws.
pub const MAX_HEALS: usize = 16;
/// The most layers with heals of their own one frame draws. Each needs the
/// layer's samples as a surface-sized image, to copy from.
pub const MAX_HEALED_LAYERS: usize = 4;
/// The most masks computed from bands one frame draws.
pub const MAX_COMPUTED_MASKS: usize = 8;

/// How a layer combines with the layers beneath it. The order matches the
/// shader's `blend`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    Normal,
    Screen,
    Add,
    Multiply,
    Lighten,
    Darken,
    /// The layer's brightness with the color beneath it.
    Luminance,
}

/// What an entry in a pane's list of layers is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mark {
    /// A layer: one that shows a band, or an adjustment.
    #[default]
    Layer,
    /// The start of a group. The entries up to the group's end blend
    /// together on their own, starting from nothing. Only its place in the
    /// list is used.
    GroupBegin,
    /// The end of a group: the members' blend becomes one layer with this
    /// entry's opacity, blend and mask. Adjustments of the entry's own
    /// follow it, as they follow a layer.
    GroupEnd,
    /// A layer of one color everywhere, with no band: what a mask is drawn
    /// with, so the mask itself shows.
    Solid,
}

/// One layer of the Combine stage: a band, stretched, colored and blended.
#[derive(Clone, Copy, Debug)]
pub struct Layer {
    /// Whether the entry is a layer or one end of a group. Groups do not
    /// hold groups.
    pub mark: Mark,
    /// A second mask that limits the layer: the mask of the layer it is
    /// clipped to. `None` is not clipped.
    pub clip: Option<usize>,
    /// A mask computed from bands that limits the layer, as an index into
    /// [`Pane::computed`]. `None` is no such mask.
    pub computed: Option<usize>,
    /// The band the layer shows.
    pub band: usize,
    pub color: [f32; 3],
    /// From 0 to 1. A hidden layer has 0.
    pub opacity: f32,
    pub blend: Blend,
    /// The layer's own contrast curve, from -1 to 1. 0 is none.
    pub curve: f32,
    /// A second band to blend into the first before the stretch, and how
    /// much of it, from 0 to 1. A document uses it for a correction whose
    /// strength a person moves: the band uncorrected and corrected in full.
    pub blended: Option<(usize, f32)>,
    /// The mask that limits where the layer applies. `None` is everywhere.
    pub mask: Option<usize>,
    /// For an adjustment: what it does, and by how much. An adjustment has
    /// no pixels: `band`, `color`, `blend`, `curve` and `blended` are not
    /// used. `None` is a layer that shows a band.
    pub adjustment: Option<(Adjustment, f32)>,
    /// For an adjustment: whether it changes only the band layer before it
    /// in the list, before that layer is blended. Otherwise it changes the
    /// blend of every layer before it.
    pub own: bool,
}

/// What an adjustment layer does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adjustment {
    /// Brightens or darkens by the amount in stops.
    Exposure = 1,
    /// A contrast curve. The amount runs from -1 to 1.
    Curves = 2,
    /// Saturation. 1 leaves the color unchanged.
    Saturation = 3,
}

/// The entries in one layer's stretch table.
pub const STRETCH_TABLE_WIDTH: usize = 4096;

/// The rows of the stretch table: one per layer, then three per Finish step
/// for a stretch of the picture, one per channel.
pub const STRETCH_TABLE_ROWS: usize = MAX_LAYERS + 3 * MAX_FINISH_STEPS;

/// What a Finish step does.
#[derive(Clone, Copy, Debug)]
pub enum FinishKind {
    /// A contrast curve. The amount runs from -1 to 1.
    Curves,
    /// Saturation. An amount of 1 leaves the image as it is.
    Saturation,
    /// A heal: the masked area is replaced by the area `offset` document
    /// pixels away, with `shift` added.
    Heal { offset: [f32; 2], shift: [f32; 3] },
    /// A stretch of the picture: [`Pane::finish_stretches`] entry `table`
    /// maps each channel. With `luminance`, the first of its curves maps
    /// the pixel's Rec. 709 luminance and the pixel is scaled to match, as
    /// Seiza's luminance-preserving strategy does.
    Stretch { table: usize, luminance: bool },
    /// Tone and color: [`Pane::finish_stretches`] entry `table` holds a
    /// curve of the luminance, applied as a luminance stretch is, then
    /// `vibrance` and `saturation`, each from -1 to 1, change the colors.
    Tone {
        table: usize,
        vibrance: f32,
        saturation: f32,
    },
}

/// One step of the Finish stage, applied to the combined image.
#[derive(Clone, Copy, Debug)]
pub struct FinishStep {
    pub kind: FinishKind,
    pub amount: f32,
    /// How strongly the step applies, from 0 to 1.
    pub opacity: f32,
    /// The mask that limits the step, as an index into [`Pane::masks`].
    /// `None` is everywhere. A heal's mask is its stroke.
    pub mask: Option<usize>,
}

/// One band a computed mask reads, stretched.
#[derive(Clone, Copy, Debug)]
pub struct MaskBand {
    pub band: usize,
    /// A second band blended into the first before the stretch, and how
    /// much of it, as for a layer.
    pub blended: Option<(usize, f32)>,
    /// The row of the pane's stretch tables that holds the band's stretch.
    pub stretch_row: usize,
}

/// A mask computed at each pixel from the stretched samples of one or two
/// bands: their product, shaped.
#[derive(Clone, Copy, Debug)]
pub struct ComputedMask {
    pub bands: [Option<MaskBand>; 2],
    /// Whether the product is raised to the power of 1 minus itself.
    pub power_of_inverse: bool,
    /// The power the shaped product is raised to. 1 leaves it as it is.
    pub reach: f32,
    /// How far the mask is smoothed before the strokes: the standard
    /// deviation of a Gaussian blur, in document pixels. 0 is not at all.
    pub smoothing: f32,
    /// Strokes that set the mask to 1 under them, as an index into
    /// [`Pane::masks`].
    pub added: Option<usize>,
    /// Strokes that set the mask to 0 under them, as an index into
    /// [`Pane::masks`].
    pub removed: Option<usize>,
    /// Whether the mask is 1 minus all of the above.
    pub inverted: bool,
}

/// A heal of one layer: the masked area takes the layer's own samples from
/// `offset` document pixels away.
#[derive(Clone, Copy, Debug)]
pub struct LayerHeal {
    /// The layer healed, as an index into [`Pane::layers`].
    pub layer: usize,
    /// Whether the heal changes the band's samples before the stretch, as
    /// a heal of an input does. Otherwise it changes the stretched layer.
    pub before_stretch: bool,
    pub offset: [f32; 2],
    /// The brightness added to the copied samples.
    pub shift: f32,
    /// The heal's stroke, as an index into [`Pane::masks`].
    pub mask: usize,
}

/// One segment of a brush stroke, in document pixels.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Segment {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub radius: f32,
    pub hardness: f32,
    /// 1 adds to the mask, 0 subtracts from it.
    pub value: f32,
    pub _pad: f32,
}

/// What one pane of the canvas draws.
pub struct Pane<'a> {
    /// The layers, bottom first. At most [`MAX_LAYERS`].
    pub layers: &'a [Layer],
    /// Each layer's stretch, in the layers' order: [`STRETCH_TABLE_WIDTH`]
    /// outputs from 0 to 1. Entry `i` is the stretch of the band sample
    /// `(i / (width - 1))²`, where a band's samples run from 0 to 1.
    pub stretches: &'a [Vec<f32>],
    /// The Finish steps, in order. At most [`MAX_FINISH_STEPS`].
    pub finish: &'a [FinishStep],
    /// The curves of the Finish steps that stretch the picture, in the
    /// form of [`Self::stretches`]: one per channel, red, green and blue.
    pub finish_stretches: &'a [[Vec<f32>; 3]],
    /// The masks the layers and the Finish steps refer to. At most
    /// [`MAX_MASKS`].
    pub masks: &'a [Vec<Segment>],
    /// The masks computed from bands that the layers refer to. At most
    /// [`MAX_COMPUTED_MASKS`].
    pub computed: &'a [ComputedMask],
    /// Heals of single layers, in order. At most [`MAX_HEALS`], on at most
    /// [`MAX_HEALED_LAYERS`] layers.
    pub heals: &'a [LayerHeal],
    /// Where the pane's image is laid out in the surface: x, y, width and
    /// height in pixels. The view's center is at its middle.
    pub viewport: [u32; 4],
    /// The part of the surface the pane draws to. It is the viewport for a
    /// pane of its own, and a part of it for one side of a wipe.
    pub scissor: [u32; 4],
}
