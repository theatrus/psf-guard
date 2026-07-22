use anyhow::{Context, Result};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ColorType, ImageEncoder};
use image::{ImageBuffer, Rgb};
use imageproc::drawing::{draw_filled_circle_mut, draw_hollow_circle_mut};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use crate::hocus_focus_star_detection::{detect_stars_hocus_focus, HocusFocusParams};
use crate::image_analysis::FitsImage;
use crate::nina_star_detection::{
    detect_stars_with_original, StarDetectionParams, StarSensitivity,
};
use crate::psf_fitting::PSFType;
use seiza_stretch::{stretch_u16_to_u16, StretchParams};

/// Convert a color name to RGB values
fn parse_color(color_name: &str) -> Rgb<u8> {
    match color_name.to_lowercase().as_str() {
        "red" => Rgb([255, 0, 0]),
        "green" => Rgb([0, 255, 0]),
        "blue" => Rgb([0, 0, 255]),
        "yellow" => Rgb([255, 255, 0]),
        "cyan" => Rgb([0, 255, 255]),
        "magenta" => Rgb([255, 0, 255]),
        "white" => Rgb([255, 255, 255]),
        _ => Rgb([255, 0, 0]), // Default to red
    }
}

/// Create an annotated image with detected stars marked
#[allow(clippy::too_many_arguments)]
pub fn annotate_stars(
    fits_path: &str,
    output: Option<String>,
    max_stars: usize,
    detector: &str,
    sensitivity: &str,
    midtone_factor: f64,
    shadow_clipping: f64,
    annotation_color: &str,
    psf_type: &str,
    verbose: bool,
) -> Result<()> {
    if verbose {
        eprintln!("Loading FITS file: {}", fits_path);
    }

    // Load the FITS file
    let fits = FitsImage::from_file(Path::new(fits_path))?;
    let width = fits.width;
    let height = fits.height;

    if verbose {
        eprintln!("Image dimensions: {}x{}", width, height);
    }

    // Calculate image statistics
    let stats = fits.calculate_basic_statistics();

    if verbose {
        eprintln!(
            "Image stats - Min: {}, Max: {}, Mean: {:.2}, Median: {:.2}",
            stats.min, stats.max, stats.mean, stats.median
        );
    }

    // Apply MTF stretch
    let stretch_params = StretchParams {
        target_median: midtone_factor,
        shadows_clip: shadow_clipping,
    };

    let stretched = stretch_u16_to_u16(&fits.data, &stats.to_stretch_statistics(), &stretch_params);

    if verbose {
        eprintln!(
            "Applied MTF stretch with factor {} and shadow clipping {}",
            midtone_factor, shadow_clipping
        );
    }

    // Detect stars using the selected algorithm
    let stars = match detector.to_lowercase().as_str() {
        "nina" => {
            // Parse sensitivity
            let star_sensitivity = match sensitivity.to_lowercase().as_str() {
                "high" => StarSensitivity::High,
                "highest" => StarSensitivity::Highest,
                _ => StarSensitivity::Normal,
            };

            if verbose {
                eprintln!("Using NINA star detection with {} sensitivity", sensitivity);
            }

            let params = StarDetectionParams {
                sensitivity: star_sensitivity,
                noise_reduction: crate::nina_star_detection::NoiseReduction::None,
                use_roi: false,
            };
            let result = detect_stars_with_original(&stretched, &fits.data, width, height, &params);

            if verbose {
                eprintln!("Detected {} stars", result.star_list.len());
                eprintln!(
                    "Average HFR: {:.3}, Std Dev: {:.3}",
                    result.average_hfr, result.hfr_std_dev
                );
            }

            // Convert to common format
            result
                .star_list
                .into_iter()
                .map(|s| (s.position.0, s.position.1, s.hfr))
                .collect::<Vec<_>>()
        }
        "hocusfocus" => {
            if verbose {
                eprintln!("Using HocusFocus star detection");
            }

            // Parse PSF type
            let params = HocusFocusParams {
                psf_type: psf_type.parse().unwrap_or(PSFType::None),
                ..Default::default()
            };
            if params.psf_type != PSFType::None && verbose {
                eprintln!("  PSF Fitting: {:?}", params.psf_type);
            }

            let result = detect_stars_hocus_focus(&fits.data, width, height, &params);
            let stars = result.stars;

            if verbose {
                eprintln!("Detected {} stars", stars.len());
                if !stars.is_empty() {
                    let avg_hfr = stars.iter().map(|s| s.hfr).sum::<f64>() / stars.len() as f64;
                    eprintln!("Average HFR: {:.3}", avg_hfr);
                }
            }

            // Convert to common format
            stars
                .into_iter()
                .map(|s| (s.position.0, s.position.1, s.hfr))
                .collect::<Vec<_>>()
        }
        _ => {
            anyhow::bail!("Unknown detector: {}. Use 'nina' or 'hocusfocus'", detector);
        }
    };

    // Sort stars by HFR (smallest first - best focus) and take top N
    let mut stars_sorted = stars;
    stars_sorted.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap());
    let total_stars = stars_sorted.len();
    let stars_to_annotate: Vec<_> = stars_sorted.into_iter().take(max_stars).collect();

    if verbose {
        eprintln!(
            "Annotating {} stars (top {} by HFR)",
            stars_to_annotate.len(),
            max_stars
        );
    }

    // Convert stretched 16-bit data to 8-bit RGB
    let mut rgb_image = ImageBuffer::<Rgb<u8>, Vec<u8>>::new(width as u32, height as u32);

    for (x, y, pixel) in rgb_image.enumerate_pixels_mut() {
        let idx = y as usize * width + x as usize;
        let value = (stretched[idx] >> 8) as u8; // Convert 16-bit to 8-bit
        *pixel = Rgb([value, value, value]); // Grayscale to RGB
    }

    // Parse annotation color
    let color = parse_color(annotation_color);

    // Draw circles around detected stars
    for (x, y, hfr) in &stars_to_annotate {
        // Calculate circle radius based on HFR
        // Use 2.5 * HFR for circle radius, with minimum of 5 pixels
        let radius = (hfr * 2.5).max(5.0) as i32;

        // Draw hollow circle
        draw_hollow_circle_mut(&mut rgb_image, (*x as i32, *y as i32), radius, color);

        // For very small stars, also draw a filled center point
        if radius < 8 {
            draw_filled_circle_mut(&mut rgb_image, (*x as i32, *y as i32), 1, color);
        }
    }

    // Generate output filename
    let output_path = output.unwrap_or_else(|| {
        let base = fits_path.trim_end_matches(".fits").trim_end_matches(".fit");
        format!("{}_annotated.png", base)
    });

    // Save the annotated image with compression
    let file = File::create(&output_path)
        .with_context(|| format!("Failed to create output file: {}", output_path))?;
    let writer = BufWriter::new(file);

    // Create PNG encoder with best compression
    let encoder = PngEncoder::new_with_quality(writer, CompressionType::Best, FilterType::Adaptive);

    // Write the image data
    encoder
        .write_image(
            &rgb_image,
            width as u32,
            height as u32,
            ColorType::Rgb8.into(),
        )
        .with_context(|| format!("Failed to write PNG image to {}", output_path))?;

    println!("Created annotated image: {}", output_path);
    println!(
        "Annotated {} stars out of {} detected",
        stars_to_annotate.len(),
        total_stars
    );

    if verbose && !stars_to_annotate.is_empty() {
        println!("\nTop 10 stars by HFR:");
        for (i, (x, y, hfr)) in stars_to_annotate.iter().take(10).enumerate() {
            println!(
                "  {}. Position: ({:.1}, {:.1}), HFR: {:.3}",
                i + 1,
                x,
                y,
                hfr
            );
        }
    }

    Ok(())
}
