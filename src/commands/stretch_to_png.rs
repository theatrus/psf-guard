use anyhow::{Context, Result};
use image::ColorType;
use image::{ImageBuffer, Luma};
use std::path::{Path, PathBuf};

use crate::image_analysis::{ColorFrame, FitsImage};
use crate::preview_format::PreviewEncoding;

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

/// Render a one-shot-color frame in colour, or report that it is not a mosaic.
///
/// Returns `Ok(false)` for a frame with no `BAYERPAT`, leaving the caller to
/// fall back to the greyscale path — a mono camera has no colour to show, and
/// refusing outright would make the option useless on a mixed rig.
///
/// The transfer is the same midtone stretch the greyscale preview uses, so a
/// colour and a mono rendition of the same exposure sit at the same
/// brightness. It is measured once on luminance and applied identically to
/// red, green, and blue: that keeps the ratios between channels, and with
/// them the colour. Stretching each channel against its own statistics would
/// pull all three toward a common median and wash the image out.
pub fn color_to_png_with_resize(
    fits_path: &str,
    output: Option<String>,
    midtone_factor: f64,
    shadow_clipping: f64,
    max_dimensions: Option<(u32, u32)>,
    encoding: PreviewEncoding,
) -> Result<bool> {
    use seiza_stretch::{stretch_u16_to_u16, StretchParams};

    let path = Path::new(fits_path);
    let Some(frame) = ColorFrame::from_file(path)
        .with_context(|| format!("Failed to load FITS file: {}", path.display()))?
    else {
        return Ok(false);
    };

    // Statistics come from luminance so every channel shares one transfer.
    let luminance = FitsImage {
        width: frame.width,
        height: frame.height,
        data: frame.luminance(),
        raw_min: 0.0,
        raw_scale: 1.0,
        bzero: 0.0,
    };
    let statistics = luminance
        .calculate_basic_statistics()
        .to_stretch_statistics();
    let params = StretchParams {
        target_median: midtone_factor,
        shadows_clip: shadow_clipping,
    };

    let planes: Vec<Vec<u16>> = (0..3)
        .map(|channel| stretch_u16_to_u16(&frame.channel(channel), &statistics, &params))
        .collect();
    let mut interleaved = Vec::with_capacity(frame.width * frame.height * 3);
    for pixel in 0..frame.width * frame.height {
        for plane in &planes {
            interleaved.push((plane[pixel] >> 8) as u8);
        }
    }

    let buffer = image::RgbImage::from_raw(frame.width as u32, frame.height as u32, interleaved)
        .context("Failed to create colour image buffer")?;
    let buffer = match max_dimensions {
        Some((max_width, max_height)) => resize_to_fit(buffer, max_width, max_height),
        None => buffer,
    };

    let output_path = output.map_or_else(
        // Name the file after what is about to be written to it, not after
        // the format this function used to be able to produce.
        || path.with_extension(encoding.extension()),
        PathBuf::from,
    );
    encoding.write(
        &output_path,
        buffer.as_raw(),
        buffer.width(),
        buffer.height(),
        ColorType::Rgb8,
    )?;
    Ok(true)
}

/// Scale down to fit a box, keeping the aspect ratio. Never scales up: an
/// enlarged preview costs bytes and shows nothing extra.
fn resize_to_fit(buffer: image::RgbImage, max_width: u32, max_height: u32) -> image::RgbImage {
    let scale =
        (max_width as f32 / buffer.width() as f32).min(max_height as f32 / buffer.height() as f32);
    if scale >= 1.0 {
        return buffer;
    }
    let width = ((buffer.width() as f32 * scale) as u32).max(1);
    let height = ((buffer.height() as f32 * scale) as u32).max(1);
    image::imageops::resize(
        &buffer,
        width,
        height,
        image::imageops::FilterType::Lanczos3,
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
    stretch_to_png_with_format(
        fits_path,
        output,
        midtone_factor,
        shadow_clipping,
        logarithmic,
        invert,
        max_dimensions,
        PreviewEncoding::png(),
    )
}

/// As above, in a chosen format. The CLI writes PNG; the server writes
/// whatever its configuration says.
#[allow(clippy::too_many_arguments)]
pub fn stretch_to_png_with_format(
    fits_path: &str,
    output: Option<String>,
    midtone_factor: f64,
    shadow_clipping: f64,
    logarithmic: bool,
    invert: bool,
    max_dimensions: Option<(u32, u32)>,
    encoding: PreviewEncoding,
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

    encoding.write(
        &output_path,
        &final_buffer,
        final_buffer.width(),
        final_buffer.height(),
        ColorType::L8,
    )?;

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
    use seiza_stretch::{stretch_u16_to_u16, StretchParams};

    // Create stretch parameters
    let stretch_params = StretchParams {
        target_median: midtone_factor,
        shadows_clip: shadow_clipping,
    };

    println!(
        "Applying MTF stretch (factor: {:.2}, shadow clipping: {:.2})",
        midtone_factor, shadow_clipping
    );

    // Apply MTF stretch to get 16-bit data
    let stretched_16bit =
        stretch_u16_to_u16(&image.data, &stats.to_stretch_statistics(), &stretch_params);

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
