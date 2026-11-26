//! The render half of the PSF-Studio canvas: a tiled, many-band document
//! drawn with Seiza's GPU compositor. It knows nothing about the window
//! toolkit; the host hands it a `wgpu::Surface`.

pub mod display;
mod renderer;
mod source;
pub mod synth;
mod tiles;

pub use renderer::{pane_rect, Frame, FrameStats, Output, Pane, Renderer, Sampled, View};
pub use source::{Pyramid, TileSource};
