use anyhow::Result;

pub mod star_selection;
pub mod text_render;
mod visualize_psf_multi;

pub use self::visualize_psf_multi::visualize_psf_multi;

/// Wrapper for backwards compatibility
pub fn visualize_psf_residuals(
    fits_path: &str,
    output: Option<String>,
    star_index: Option<usize>,
    psf_type: &str,
    max_stars: usize,
    verbose: bool,
) -> Result<()> {
    // If a specific star index is requested, show just that one star
    let num_stars = if star_index.is_some() {
        1
    } else {
        max_stars.min(9)
    };

    // Call the multi-star version with appropriate parameters
    visualize_psf_multi(
        fits_path, output, num_stars, psf_type, "r2",  // Sort by R² by default
        3,     // 3 columns grid
        "top", // Default to top selection mode
        verbose,
    )
}
