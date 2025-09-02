use anyhow::{Context, Result};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ColorType, ImageEncoder};
use image::{ImageBuffer, Luma};
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use crate::image_analysis::FitsImage;
use crate::mtf_stretch::StretchParameters;

pub fn stretch_to_png(
    fits_path: &str,
    output: Option<String>,
    midtone_factor: f64,
    shadow_clipping: f64,
    logarithmic: bool,
    invert: bool,
) -> Result<()> {
    stretch_to_png_with_resize(
        fits_path,
        output,
        midtone_factor,
        shadow_clipping,
        logarithmic,
        invert,
        None, // No resize
    )
}

pub fn stretch_to_png_with_resize(
    fits_path: &str,
    output: Option<String>,
    midtone_factor: f64,
    shadow_clipping: f64,
    logarithmic: bool,
    invert: bool,
    max_dimensions: Option<(u32, u32)>,
) -> Result<()> {
    // Load FITS file
    let fits_path = Path::new(fits_path);
    println!("Loading FITS file: {}", fits_path.display());

    let image = FitsImage::from_file(fits_path)
        .with_context(|| format!("Failed to load FITS file: {}", fits_path.display()))?;

    println!("Image dimensions: {}x{}", image.width, image.height);

    // Calculate statistics
    let stats = image.calculate_basic_statistics();
    println!("Statistics:");
    println!("  Mean: {:.3}", stats.mean);
    println!("  Median: {:.3}", stats.median);
    println!("  MAD: {:.3}", stats.mad.unwrap_or(0.0));
    println!("  Min: {:.0}", stats.min);
    println!("  Max: {:.0}", stats.max);

    // Determine output path
    let output_path = match output {
        Some(path) => PathBuf::from(path),
        None => {
            let mut path = fits_path.to_path_buf();
            path.set_extension("png");
            path
        }
    };

    println!("Processing image...");

    // Apply stretch or logarithmic scaling
    let processed_data = if logarithmic {
        apply_logarithmic_stretch(&image, invert)
    } else {
        apply_mtf_stretch(&image, &stats, midtone_factor, shadow_clipping, invert)?
    };

    // Create PNG image
    let img_buffer = ImageBuffer::<Luma<u8>, Vec<u8>>::from_raw(
        image.width as u32,
        image.height as u32,
        processed_data,
    )
    .context("Failed to create image buffer")?;

    // Resize if requested
    let final_buffer = if let Some((max_width, max_height)) = max_dimensions {
        let (orig_width, orig_height) = (img_buffer.width(), img_buffer.height());

        // Calculate scaling to fit within max dimensions while preserving aspect ratio
        let scale_x = max_width as f32 / orig_width as f32;
        let scale_y = max_height as f32 / orig_height as f32;
        let scale = scale_x.min(scale_y).min(1.0); // Don't upscale

        if scale < 1.0 {
            let new_width = (orig_width as f32 * scale) as u32;
            let new_height = (orig_height as f32 * scale) as u32;

            println!(
                "Resizing from {}x{} to {}x{}",
                orig_width, orig_height, new_width, new_height
            );

            image::imageops::resize(
                &img_buffer,
                new_width,
                new_height,
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            img_buffer
        }
    } else {
        img_buffer
    };

    // Save PNG with compression
    let file = File::create(&output_path)
        .with_context(|| format!("Failed to create output file: {}", output_path.display()))?;
    let writer = BufWriter::new(file);

    // Create PNG encoder with best compression
    let encoder = PngEncoder::new_with_quality(writer, CompressionType::Best, FilterType::Adaptive);

    // Write the image data
    encoder
        .write_image(
            &final_buffer,
            final_buffer.width(),
            final_buffer.height(),
            ColorType::L8.into(),
        )
        .with_context(|| format!("Failed to write PNG image to {}", output_path.display()))?;

    println!("Saved stretched image to: {}", output_path.display());
    Ok(())
}

fn apply_mtf_stretch(
    image: &FitsImage,
    stats: &crate::image_analysis::ImageStatistics,
    midtone_factor: f64,
    shadow_clipping: f64,
    invert: bool,
) -> Result<Vec<u8>> {
    use crate::mtf_stretch::stretch_image;

    // Create stretch parameters
    let stretch_params = StretchParameters {
        factor: midtone_factor,
        black_clipping: shadow_clipping,
    };

    println!(
        "Applying MTF stretch (factor: {:.2}, shadow clipping: {:.2})",
        midtone_factor, shadow_clipping
    );

    // Apply MTF stretch to get 16-bit data
    let stretched_16bit = stretch_image(
        &image.data,
        stats,
        stretch_params.factor,
        stretch_params.black_clipping,
    );

    // Convert to 8-bit
    let mut result = Vec::with_capacity(stretched_16bit.len());
    for &pixel in &stretched_16bit {
        let eight_bit = (pixel >> 8) as u8;
        let final_pixel = if invert { 255 - eight_bit } else { eight_bit };
        result.push(final_pixel);
    }

    Ok(result)
}

fn apply_logarithmic_stretch(image: &FitsImage, invert: bool) -> Vec<u8> {
    println!("Applying logarithmic stretch");

    // Find min/max for scaling
    let min_val = *image.data.iter().min().unwrap() as f64;
    let max_val = *image.data.iter().max().unwrap() as f64;

    println!("Value range: {:.0} - {:.0}", min_val, max_val);

    let mut result = Vec::with_capacity(image.data.len());

    // Apply logarithmic scaling: log(1 + x)
    let log_max = (1.0 + max_val - min_val).ln();

    for &pixel in &image.data {
        let normalized = (pixel as f64 - min_val).max(0.0);
        let log_val = (1.0 + normalized).ln();
        let scaled = (log_val / log_max * 255.0) as u8;
        let final_pixel = if invert { 255 - scaled } else { scaled };
        result.push(final_pixel);
    }

    result
}
