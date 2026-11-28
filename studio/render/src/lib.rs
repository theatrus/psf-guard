//! The render half of the PSF-Studio canvas: a tiled, many-band document
//! drawn with Seiza's GPU compositor. It knows nothing about the window
//! toolkit. It draws either to a surface or to a captured image.

pub mod display;
mod renderer;
mod source;
pub mod synth;
mod tiles;

pub use renderer::{
    pane_rect, Captured, CapturedPixels, Frame, FrameStats, Output, Pane, Previewed, ProbeSlot,
    Renderer, Sampled, View, WorkingSpace,
};
pub use source::{Pyramid, TileSource};
