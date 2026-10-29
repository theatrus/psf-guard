//! The measurements and model behind flat grouping, which Seiza holds
//! (`seiza_calibration::flat_response`) so the desktop apps group flats as
//! this server does:
//!
//! - [`metric`]: a session's response, the star mask, the comparison of two
//!   responses (`Q`, dust, broad, spots), a change's map, and carrying a
//!   flat across a change with it.
//! - [`evidence`]: whether a few lights' own background carries a change.
//! - [`choice`]: which of several flat responses a few lights' own
//!   background follows.
//! - [`model`]: response families, groups, rig-wide change events, the
//!   angle term, and where a moment stands against an event.
//! - [`lights`]: which response family a light's flats come from: the
//!   rotator's angle epochs, the candidates within the angle prior, and
//!   what the lights' own pixels said.
//! - [`overrides`]: a person's splits, joins and choices for lights,
//!   applied over what the measurements make.
//! - [`rows`]: the numbers those take and give.

pub use seiza_calibration::flat_response::{choice, evidence, metric};
pub mod lights;
pub mod model;
pub mod overrides;
pub mod rows;
