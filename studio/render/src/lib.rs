//! The render half of the PSF-Studio canvas spike: a tiled, eight-band
//! document drawn with wgpu. It knows nothing about the window toolkit; the
//! host hands it a `wgpu::Surface`.

pub mod display;
mod renderer;
mod source;
pub mod synth;
mod tiles;

pub use renderer::{
    pane_rect, Adjustment, Blend, ComputedMask, FinishKind, FinishStep, Frame, FrameStats, Layer,
    LayerHeal, Mark, MaskBand, Output, Pane, Renderer, Sampled, Segment, View, MAX_COMPUTED_MASKS,
    MAX_FINISH_STEPS, MAX_HEALED_LAYERS, MAX_HEALS, MAX_LAYERS, MAX_MASKS, STRETCH_TABLE_WIDTH,
};
pub use source::{Pyramid, TileSource};
