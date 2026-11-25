//! The render half of the PSF-Studio canvas spike: a tiled, eight-band
//! document drawn with wgpu. It knows nothing about the window toolkit; the
//! host hands it a `wgpu::Surface`.

pub mod display;
mod renderer;
pub mod synth;
mod tiles;

pub use renderer::{
    Blend, FinishKind, FinishStep, Frame, FrameStats, Layer, Output, Renderer, Segment, View,
    MAX_FINISH_STEPS, MAX_LAYERS, MAX_MASKS,
};
