//! What PSF Guard and PSF Studio share.
//!
//! PSF Guard prepares pictures and stores Studio documents; PSF Studio edits
//! them. Both build this crate, so a document, a tile store or a frame is
//! read the same way by each:
//!
//! - [`channels`]: channel stacks registered onto one grid with the
//!   document's Linear steps run on them, ready for a tile store.
//! - [`color`]: channel roles, and the color steps both run: registration,
//!   background extraction, display stretches.
//! - [`prepare`]: preparing a picture: a tile store from channel stacks and
//!   a document.
//! - [`studio`]: the Studio document model, its tile store and flows.
//! - [`image_steps`]: the contract of a step on an image: shapes, regions
//!   and sample domains.
//! - [`image_io`]: reading a FITS or XISF frame, on one physical scale.
//! - [`studio_frames`]: a frame on disk as a region reader for a store.

pub mod channels;
pub mod color;
pub mod image_io;
pub mod image_steps;
pub mod prepare;
pub mod studio;
pub mod studio_frames;

/// The built-in color pipeline, which PSF Guard's seeded "Default" setup
/// starts with and a new Studio picture starts from: background extraction,
/// an automatic MTF stretch on every channel, solar color calibration, and
/// inputs that follow their channel cards. The frontend's
/// `defaultColorProcessing` must produce exactly this JSON; a test on each
/// side checks it.
pub const DEFAULT_COLOR_SETTINGS: &str = include_str!("default_color.json");
