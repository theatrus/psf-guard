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
//!
//! A [`TileStore`] keeps an image on disk in tiles, with a pyramid of
//! reduced levels, and reads any region of any level.

mod budget;
mod document;
mod error;
mod flow;
mod junction;
mod palette;
mod pipeline;
mod place;
mod recipe;
mod source;
mod step;
mod store;
mod stroke;
mod view;

#[cfg(test)]
mod flow_tests;
#[cfg(test)]
mod place_tests;
#[cfg(test)]
mod recipe_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod test_steps;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod view_tests;

pub use budget::{read_within, MemoryBudget};
pub use document::{
    BlendMode, CropRecord, Document, LayerRecord, LayerSource, MaskRecord, MaskShape, PaletteUse,
    SharedMask, Stage, StageKind, StepKind, StepRecord, DOCUMENT_VERSION,
};
pub use error::StudioError;
pub use flow::{
    background_corrected_channel, color_group_of, color_setup_from_document,
    document_from_color_setup, flow_operations, stars_channel, stretched_channel, tools_channel,
    COLOR_GROUPS,
};
pub use junction::{open_in_editor_command, FileStamp, Junction, JunctionState, JunctionStatus};
pub use palette::{
    balance_colors, band_layer, channel_label, color_from_hue, color_word, default_palette, hue_of,
    layer_name, palettes_for, Palette, PaletteLayer, PaletteMask, PALETTES,
};
pub use pipeline::{level_for_zoom, Pipeline, PreviewChain};
pub use place::{input_steps, panes, InputStep, Place, Row, Split};
pub use recipe::{recipes_for, PartKind, Recipe, Route, RECIPES};
pub use source::{MemoryImage, RegionReader};
pub use step::{Applied, LevelForm, MaskSource, Masked, Step};
pub use store::{SampleFormat, StoreLevel, StoreOptions, TileStore};
pub use stroke::{Heal, PaintedMask, Stroke, StrokeMode};
pub use view::{
    own_step_for_channel, plan, step_shown_for, ChannelPlan, DisplayStretch, OwnUntil,
    PlannedStretch, StackEntry, Subject, UpTo, View, ViewPlan,
};
