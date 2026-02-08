pub mod accord_imaging;
pub mod cli;
pub mod commands;
pub mod config;
pub mod db;
pub mod debug;
pub mod directory_tree;
pub mod grading;
pub mod hocus_focus_star_detection;
pub mod image_analysis;
pub mod models;
pub mod mtf_stretch;
pub mod nina_star_detection;
pub mod opencv_canny;
pub mod opencv_contours;
pub mod opencv_morphology;
#[cfg(feature = "opencv")]
pub mod opencv_utils;
pub mod opencv_wavelets;
pub mod psf_fitting;
pub mod sequence_analysis;
pub mod server;
pub mod utils;

// Main entry points
pub mod cli_main;
#[cfg(feature = "tauri")]
pub mod tauri_main;

#[cfg(test)]
mod test_star_detection;

// Re-export commonly used items
pub use image_analysis::{FitsImage, ImageStatistics};
