//! The measurements behind flat grouping, with a pure API: image planes and
//! numbers in, plain structs out. Nothing here touches the catalog, the
//! server or files, so it can move into `seiza-calibration` as it stands
//! (see `docs/design/flat-grouping.md`, "Follow-ups: move to Seiza").
//!
//! - [`metric`]: a session's response, the star mask, the comparison of two
//!   responses (`Q`, dust, broad, spots), a change's map, and carrying a
//!   flat across a change with it.
//! - [`model`]: groups, rig-wide change events, the angle term, and where a
//!   moment stands against an event.
//! - [`evidence`]: whether a few lights' own background carries a change.
//! - [`rows`]: the numbers those take and give.

pub mod evidence;
pub mod metric;
pub mod model;
pub mod rows;

#[cfg(test)]
mod tests {
    /// The module stays liftable: it names nothing of PSF Guard's outside
    /// itself and reads no database.
    #[test]
    fn nothing_here_reaches_outside_the_module() {
        for (name, source) in [
            ("evidence.rs", include_str!("evidence.rs")),
            ("metric.rs", include_str!("metric.rs")),
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
