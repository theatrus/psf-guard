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

/// Write a colour frame as a stretched preview.
///
/// The transfer is the same midtone stretch the greyscale preview uses, so a
/// colour and a mono rendition of the same exposure sit at the same
/// brightness. For a linked frame it is measured once on luminance and
/// applied identically to red, green, and blue: that keeps the ratios
/// between channels, and with them the colour. Stretching each channel
/// against its own statistics would pull all three toward a common median,
/// which is wrong for a raw sub but is what an uncalibrated master wants, so
/// an unlinked frame gets a transfer per channel.
fn write_color_preview(
    frame: &ColorFrame,
    output_path: &Path,
    midtone_factor: f64,
    shadow_clipping: f64,
    max_dimensions: Option<(u32, u32)>,
    encoding: PreviewEncoding,
) -> Result<()> {
    use seiza_stretch::{statistics_u16, stretch_u16_to_u16, StretchParams};

    // A linked stretch measures luminance so every channel shares one
    // transfer.
    let linked = frame.linked.then(|| statistics_u16(&frame.luminance()));
    let params = StretchParams {
        target_median: midtone_factor,
        shadows_clip: shadow_clipping,
    };

    // One channel at a time, written straight into the interleaved output.
    // Holding all three stretched planes at once costs three more copies of a
    // full frame, which on a 60-megapixel sub is most of the memory budget a
    // preview worker is allowed.
    let pixels = frame.width * frame.height;
    let mut interleaved = vec![0u8; pixels * 3];
    for channel in 0..3 {
        let samples = frame.channel(channel);
        let statistics = linked.clone().unwrap_or_else(|| statistics_u16(&samples));
        let stretched = stretch_u16_to_u16(&samples, &statistics, &params);
        for (pixel, value) in stretched.iter().enumerate() {
            interleaved[pixel * 3 + channel] = (value >> 8) as u8;
        }
    }

    let buffer = image::RgbImage::from_raw(frame.width as u32, frame.height as u32, interleaved)
        .context("Failed to create colour image buffer")?;
    let buffer = match max_dimensions {
        Some((max_width, max_height)) => resize_to_fit(buffer, max_width, max_height),
        None => buffer,
    };

    encoding.write(
        output_path,
        buffer.as_raw(),
        buffer.width(),
        buffer.height(),
        ColorType::Rgb8,
    )
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
    render_preview(
        fits_path,
        output,
        midtone_factor,
        shadow_clipping,
        logarithmic,
        invert,
        max_dimensions,
        false,
        PreviewEncoding::png(),
    )
}

/// As above, in a chosen format. The CLI writes PNG; the server writes
/// whatever its configuration says.
///
/// Planar RGB, such as an integrated master, is always shown in colour. A
/// raw mosaic is debayered to colour only when `color` asks for it. The
/// logarithmic and inverted renditions are greyscale only.
#[allow(clippy::too_many_arguments)]
pub fn render_preview(
    fits_path: &str,
    output: Option<String>,
    midtone_factor: f64,
    shadow_clipping: f64,
    logarithmic: bool,
    invert: bool,
    max_dimensions: Option<(u32, u32)>,
    color: bool,
    encoding: PreviewEncoding,
) -> Result<()> {
    // Load FITS file
    let fits_path = Path::new(fits_path);
    println!("Loading FITS file: {}", fits_path.display());

    let fits = crate::image_io::open(fits_path)
        .map_err(|e| anyhow::anyhow!("Failed to load FITS file {}: {e:?}", fits_path.display()))?;

    let colour = (!logarithmic && !invert)
        .then(|| ColorFrame::from_decoded(&fits, color))
        .flatten();
    if let Some(frame) = colour {
        // A float master's decoded planes are twice the size of the frame
        // kept here; free them before stretching.
        drop(fits);
        println!("Colour image dimensions: {}x{}", frame.width, frame.height);
        let output_path = output.map_or_else(
            // Name the file after what is about to be written to it, not
            // after the format this function used to be able to produce.
            || fits_path.with_extension(encoding.extension()),
            PathBuf::from,
        );
        write_color_preview(
            &frame,
            &output_path,
            midtone_factor,
            shadow_clipping,
            max_dimensions,
            encoding,
        )?;
        println!("Saved stretched image to: {}", output_path.display());
        return Ok(());
    }

    let image = FitsImage::from_decoded(fits);

    println!("Image dimensions: {}x{}", image.width, image.height);

    // The encoder asserts on a length mismatch, which takes down a server
    // worker; refuse here instead.
    anyhow::ensure!(
        image.data.len() == image.width * image.height,
        "{} decoded to {} samples for a {}x{} image",
        fits_path.display(),
        image.data.len(),
        image.width,
        image.height
    );

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

#[cfg(test)]
mod tests {
    use super::*;

    /// An integrated master is planar float RGB. Before, its three planes
    /// landed in a greyscale buffer three times the image size and the
    /// encoder panicked. It previews in colour whether or not mosaics are
    /// set to, stretched per channel, and in greyscale for the logarithmic
    /// rendition.
    #[test]
    fn a_planar_rgb_master_previews_in_colour() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("master.xisf");
        let (width, height) = (8usize, 6usize);
        let pixels = width * height;
        // A red cast: red over green over blue, each with the same spread so
        // the stretch has a histogram to work from.
        let planes: Vec<f32> = (0..3 * pixels)
            .map(|index| {
                let (plane, pixel) = (index / pixels, index % pixels);
                (0.6 - 0.2 * plane as f32) + 0.004 * pixel as f32
            })
            .collect();
        seiza_xisf::write_f32_image(
            &path,
            width,
            height,
            seiza_fits::F32ImageData::RgbPlanar(&planes),
            &[],
        )
        .unwrap();

        let render = |name: &str, logarithmic: bool| {
            let out = directory.path().join(name);
            render_preview(
                path.to_str().unwrap(),
                Some(out.display().to_string()),
                0.2,
                -2.8,
                logarithmic,
                false,
                None,
                false,
                PreviewEncoding::png(),
            )
            .unwrap();
            image::open(out).unwrap()
        };

        let colour = render("colour.png", false);
        assert_eq!(colour.color(), image::ColorType::Rgb8);
        assert_eq!((colour.width(), colour.height()), (8, 6));
        // Each channel is stretched against its own statistics, so the cast
        // is gone: every channel lands on the same value.
        let centre = *colour.to_rgb8().get_pixel(4, 3);
        let spread = centre.0.iter().max().unwrap() - centre.0.iter().min().unwrap();
        assert!(
            spread <= 2,
            "an unlinked stretch should neutralize the cast: {centre:?}"
        );

        let grey = render("grey.png", true);
        assert_eq!(grey.color(), image::ColorType::L8);
        assert_eq!((grey.width(), grey.height()), (8, 6));
    }
}
