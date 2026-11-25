//! The scaffolding of PSF-Studio: a staged document, evaluated by region.
//!
//! See "Staged workflow and the scaffolding" in `docs/design/psf-studio.md`.
//! This module has no window and no GPU code; the canvas and the panes call
//! it. It builds on [`crate::image_steps`].
//!
//! Three rules keep memory bounded for very large images:
//!
//! 1. Everything reads by region ([`RegionReader`]). A step applied to a
//!    source ([`Applied`]) is itself a reader, so stages stack and none holds
//!    more than the block it is working on.
//! 2. A reader reports the peak bytes a region needs before it is read.
//! 3. A read over the [`MemoryBudget`] is cut into tiles ([`read_within`]),
//!    or refused. Nothing falls back to loading a whole image.

mod budget;
mod document;
mod error;
mod junction;
mod pipeline;
mod source;
mod step;
mod stroke;

#[cfg(test)]
mod test_steps;
#[cfg(test)]
mod tests;

pub use budget::{read_within, MemoryBudget};
pub use document::{
    BlendMode, Document, LayerRecord, LayerSource, MaskRecord, Stage, StageKind, StepKind,
    StepRecord, DOCUMENT_VERSION,
};
pub use error::StudioError;
pub use junction::{open_in_editor_command, FileStamp, Junction, JunctionState, JunctionStatus};
pub use pipeline::{level_for_zoom, Pipeline, PreviewChain};
pub use source::{MemoryImage, RegionReader};
pub use step::{Applied, LevelForm, MaskSource, Masked, Step};
pub use stroke::{Heal, PaintedMask, Stroke, StrokeMode};
