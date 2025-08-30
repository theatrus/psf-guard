use anyhow::Result;
use image::{ImageBuffer, Rgb};
use imageproc::drawing::{draw_filled_circle_mut, draw_hollow_circle_mut};

use crate::hocus_focus_star_detection::{detect_stars_hocus_focus, HocusFocusParams};
use crate::image_analysis::FitsImage;
use crate::mtf_stretch::{stretch_image, StretchParameters};
use crate::psf_fitting::PSFType;

/// Create an annotated RGB image from FITS data
pub fn create_annotated_image(
    fits: &FitsImage,
    max_stars: usize,
    midtone_factor: f64,
    shadow_clipping: f64,
    annotation_color: Rgb<u8>,
) -> Result<ImageBuffer<Rgb<u8>, Vec<u8>>> {
    let width = fits.width;
    let height = fits.height;

    // Calculate image statistics
    let stats = fits.calculate_basic_statistics();

    // Apply MTF stretch
    let stretch_params = StretchParameters {
        factor: midtone_factor,
        black_clipping: shadow_clipping,
    };

    let stretched = stretch_image(
        &fits.data,
        &stats,
        stretch_params.factor,
        stretch_params.black_clipping,
    );

    // Detect stars using HocusFocus (default for server)
    let params = HocusFocusParams {
        psf_type: PSFType::None,
        ..Default::default()
    };

    let detection_result = detect_stars_hocus_focus(&fits.data, width, height, &params);

    // Sort stars by HFR (smallest first - best focus) and take top N
    let mut stars: Vec<_> = detection_result
        .stars
        .iter()
        .map(|s| (s.position.0, s.position.1, s.hfr))
        .collect();
    stars.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap());
    let stars_to_annotate: Vec<_> = stars.into_iter().take(max_stars).collect();

    eprintln!(
        "Annotating {} stars out of {} detected",
        stars_to_annotate.len(),
        detection_result.stars.len()
    );

    // Convert stretched 16-bit data to 8-bit RGB
    let mut rgb_image = ImageBuffer::<Rgb<u8>, Vec<u8>>::new(width as u32, height as u32);

    for (x, y, pixel) in rgb_image.enumerate_pixels_mut() {
        let idx = y as usize * width + x as usize;
        let value = (stretched[idx] >> 8) as u8; // Convert 16-bit to 8-bit
        *pixel = Rgb([value, value, value]); // Grayscale to RGB
    }

    // Draw circles around detected stars
    for (x, y, hfr) in &stars_to_annotate {
        // Calculate circle radius based on HFR
        // Use 2.5 * HFR for circle radius, with minimum of 5 pixels
        let radius = (hfr * 2.5).max(5.0) as i32;

        // Draw hollow circle
        draw_hollow_circle_mut(
            &mut rgb_image,
            (*x as i32, *y as i32),
            radius,
            annotation_color,
        );

        // For very small stars, also draw a filled center point
        if radius < 8 {
            draw_filled_circle_mut(&mut rgb_image, (*x as i32, *y as i32), 1, annotation_color);
        }
    }

    Ok(rgb_image)
}
