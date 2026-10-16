//! The measurements behind flat grouping.
//!
//! The pixel work lives in Seiza (`seiza_calibration::flat_response`):
//!
//! - [`metric`]: a session's response, the star mask, the comparison of two
//!   responses (`Q`, dust, broad, spots), a change's map, and carrying a
//!   flat across a change with it.
//! - [`evidence`]: whether a few lights' own background carries a change.
//! - [`choice`]: which of several flat responses a few lights' own
//!   background follows.
//!
//! What PSF Guard builds on those numbers stays here, with a pure API: no
//! catalog, server or file types.
//!
//! - [`model`]: response families, groups, rig-wide change events, the
//!   angle term, and where a moment stands against an event.
//! - [`lights`]: which response family a light's flats come from: the
//!   rotator's angle epochs, the candidates within the angle prior, and
//!   what the lights' own pixels said.
//! - [`rows`]: the numbers those take and give.

pub use seiza_calibration::flat_response::{choice, evidence, metric};
pub mod lights;
pub mod model;
pub mod rows;

#[cfg(test)]
mod tests {
    /// The module stays pure: it names nothing of PSF Guard's outside
    /// itself and reads no database.
    #[test]
    fn nothing_here_reaches_outside_the_module() {
        for (name, source) in [
            ("lights.rs", include_str!("lights.rs")),
            ("model.rs", include_str!("model.rs")),
            ("rows.rs", include_str!("rows.rs")),
        ] {
            // Only the code: each file's tests may borrow helpers.
            let code = source.split("#[cfg(test)]").next().unwrap();
            for forbidden in [
                "crate::",
                "rusqlite",
                "super::super",
                "std::fs",
                "std::path",
            ] {
                assert!(!code.contains(forbidden), "{name} uses {forbidden}");
            }
        }
    }
}
