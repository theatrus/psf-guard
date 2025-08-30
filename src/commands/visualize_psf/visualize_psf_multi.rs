use anyhow::{Context, Result};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ColorType, ImageEncoder};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use crate::commands::visualize_psf_multi_common::create_psf_multi_image;
use crate::image_analysis::FitsImage;
use crate::psf_fitting::PSFType;

/// Enhanced PSF visualization showing multiple stars
#[allow(clippy::too_many_arguments)]
pub fn visualize_psf_multi(
    fits_path: &str,
    output: Option<String>,
    num_stars: usize,
    psf_type: &str,
    sort_by: &str,
    grid_cols: usize,
    selection_mode: &str,
    verbose: bool,
) -> Result<()> {
    if verbose {
        eprintln!("Loading FITS file: {}", fits_path);
    }

    // Load the FITS file
    let fits = FitsImage::from_file(Path::new(fits_path))?;

    // Parse PSF type
    let psf_type_enum: PSFType = psf_type.parse().unwrap_or(PSFType::Moffat4);

    if psf_type_enum == PSFType::None {
        anyhow::bail!("PSF type cannot be 'none' for residual visualization");
    }

    if verbose {
        eprintln!(
            "Creating PSF visualization with {} stars, {:?} PSF model",
            num_stars, psf_type_enum
        );
    }

    // Create the PSF multi image using the common function
    let rgba_image = create_psf_multi_image(
        &fits,
        num_stars,
        psf_type_enum,
        sort_by,
        Some(grid_cols),
        selection_mode,
    )?;

    // Generate output filename
    let output_path = output.unwrap_or_else(|| {
        let base = fits_path.trim_end_matches(".fits").trim_end_matches(".fit");
        format!("{}_psf_multi.png", base)
    });

    // Save the image with compression
    let file = File::create(&output_path)
        .with_context(|| format!("Failed to create output file: {}", output_path))?;
    let writer = BufWriter::new(file);

    // Create PNG encoder with best compression
    let encoder = PngEncoder::new_with_quality(writer, CompressionType::Best, FilterType::Adaptive);

    // Write the image data
    encoder
        .write_image(
            &rgba_image,
            rgba_image.width(),
            rgba_image.height(),
            ColorType::Rgba8.into(),
        )
        .with_context(|| format!("Failed to write PNG image to {}", output_path))?;

    println!("Created PSF visualization: {}", output_path);

    if verbose {
        eprintln!("Visualization complete");
    }

    Ok(())
}
