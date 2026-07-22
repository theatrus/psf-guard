pub mod accord_imaging;
pub mod acquisition_context;
pub mod astrometry;
pub mod astrometry_headers;
pub mod cli;
pub mod commands;
pub mod concurrency;
pub mod config;
pub mod db;
pub mod db_registry;
pub mod debug;
pub mod directory_tree;
pub mod grading;
pub mod hocus_focus_star_detection;
pub mod image_analysis;
pub mod models;
pub mod nina_star_detection;
pub mod photometry;
pub mod psf_fitting;
pub mod satellites;
pub mod sequence_analysis;
pub mod server;
pub mod spatial_analysis;
pub mod star_contours;
pub mod ts_schema;
pub mod utils;

// Main entry points
pub mod cli_main;
#[cfg(feature = "tauri")]
pub mod tauri_main;

#[cfg(test)]
mod test_star_detection;

// Re-export commonly used items
pub use image_analysis::{FitsImage, ImageStatistics};
