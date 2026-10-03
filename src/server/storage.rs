//! Folder layout below each database's storage roots.
//!
//! A database keeps three roots on [`DatabaseContext`]:
//! - `cache_dir_path`: per-image results (previews, star lists, astrometry,
//!   satellites, spatial metrics, stats);
//! - `stack_root`: [`STACKS`], [`STACK_PROCESSING`] and [`WBPP_RUNS`];
//! - `calibration_root`: [`CALIBRATION_MASTERS`].
//!
//! Every path below a root goes through this module, so a folder name lives
//! in one place.
//!
//! [`DatabaseContext`]: super::database_context::DatabaseContext

use std::path::{Path, PathBuf};

/// Stack outputs and their indices, below the stack root.
pub const STACKS: &str = "stack-previews";
/// Saved stretch and processing choices per stack, below the stack root.
pub const STACK_PROCESSING: &str = "stack-processing";
/// WBPP work folders when neither the settings nor the database name one,
/// below the stack root.
pub const WBPP_RUNS: &str = "wbpp";
/// Built calibration masters, below the calibration root.
pub const CALIBRATION_MASTERS: &str = "calibration-masters";

/// Image preview folders below the cache root, which the disk budget culls
/// first.
pub const PREVIEW_CATEGORIES: [&str; 3] = ["previews", "annotated", "stars"];
/// Preview folders whose files carry the configured image encoding.
pub const ENCODED_PREVIEW_CATEGORIES: [&str; 2] = ["previews", "annotated"];

/// Folders below [`STACKS`] that are not stack jobs.
pub mod stack_kind {
    pub const COLOR: &str = "color";
    pub const COLOR_INPUTS: &str = "color-inputs";
    pub const STRETCH: &str = "stretch";
    pub const DECONVOLUTION: &str = "deconvolution";
    pub const RC_ASTRO: &str = "rc-astro";
    pub const ARTIFACT_SEARCHES: &str = "artifact-searches";
    pub const REFERENCE_SCORES: &str = "reference-scores";
    pub const RESUME: &str = "resume";
}

/// `<stack_root>/stack-previews`.
pub fn stacks(stack_root: &Path) -> PathBuf {
    stack_root.join(STACKS)
}

/// `<stack_root>/stack-previews/<kind>`, for one of [`stack_kind`].
pub fn stack_folder(stack_root: &Path, kind: &str) -> PathBuf {
    stacks(stack_root).join(kind)
}

/// `<stack_root>/stack-processing`.
pub fn stack_processing(stack_root: &Path) -> PathBuf {
    stack_root.join(STACK_PROCESSING)
}

/// `<calibration_root>/calibration-masters`.
pub fn calibration_masters(calibration_root: &Path) -> PathBuf {
    calibration_root.join(CALIBRATION_MASTERS)
}
