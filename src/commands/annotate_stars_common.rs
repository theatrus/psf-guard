use anyhow::Result;
use image::{ImageBuffer, Rgb};
use imageproc::drawing::{draw_filled_circle_mut, draw_hollow_circle_mut};

use crate::hocus_focus_star_detection::{detect_stars_hocus_focus, HocusFocusParams};
use crate::image_analysis::FitsImage;
use seiza_stretch::{stretch_u16_to_u16, StretchParams};

/// Create an annotated RGB image from FITS data. `params` selects the
/// detection configuration — pass a telescope-class preset
/// (`HocusFocusParams::for_frame_path`) when the frame's headers are
/// available, or defaults otherwise.
///
/// `hfr_label_scale` draws each star's measured HFR beside its circle at
/// the given bitmap-font scale (glyphs are 5×7, so a scale of `s` renders
/// `7·s`-pixel-tall text). `None` draws circles only. The annotation is
/// drawn at native resolution, so callers that downscale afterwards should
/// pick the scale from their resize factor to keep labels legible.
pub fn create_annotated_image(
    fits: &FitsImage,
    params: &HocusFocusParams,
    max_stars: usize,
    midtone_factor: f64,
    shadow_clipping: f64,
    annotation_color: Rgb<u8>,
    hfr_label_scale: Option<u32>,
) -> Result<ImageBuffer<Rgb<u8>, Vec<u8>>> {
    let width = fits.width;
    let height = fits.height;

    // Calculate image statistics
    let stats = fits.calculate_basic_statistics();

    // Apply MTF stretch
    let stretch_params = StretchParams {
        target_median: midtone_factor,
        shadows_clip: shadow_clipping,
    };

    let stretched = stretch_u16_to_u16(&fits.data, &stats.to_stretch_statistics(), &stretch_params);

    let detection_result = detect_stars_hocus_focus(&fits.data, width, height, params);

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

        // Fitted HFR beside the circle, vertically centered on the star.
        if let Some(scale) = hfr_label_scale.filter(|&s| s > 0) {
            let label = format!("{hfr:.2}");
            let glyph_height = 7 * scale;
            let label_x = (*x as i64 + radius as i64 + (2 * scale) as i64).max(0) as u32;
            let label_y = (*y as i64 - (glyph_height / 2) as i64).max(0) as u32;
            crate::commands::visualize_psf::text_render::draw_text_on(
                &mut rgb_image,
                label_x,
                label_y,
                &label,
                annotation_color,
                scale,
            );
        }
    }

    Ok(rgb_image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hocus_focus_star_detection::StructureRemovalMethod;

    /// A synthetic frame with three Gaussian stars the detector finds.
    fn star_frame() -> FitsImage {
        let (width, height) = (256, 256);
        let mut data = vec![1000u16; width * height];
        for &(cx, cy) in &[(60.0f64, 60.0f64), (180.0, 90.0), (100.0, 200.0)] {
            for dy in -9i64..=9 {
                for dx in -9i64..=9 {
                    let x = (cx as i64 + dx) as usize;
                    let y = (cy as i64 + dy) as usize;
                    let d2 = (x as f64 - cx).powi(2) + (y as f64 - cy).powi(2);
                    let value = 25000.0 * (-d2 / (2.0 * 1.8f64.powi(2))).exp();
                    let index = y * width + x;
                    data[index] = (data[index] as f64 + value).min(65535.0) as u16;
                }
            }
        }
        FitsImage {
            width,
            height,
            data,
            raw_min: 0.0,
            raw_scale: 1.0,
            bzero: 0.0,
        }
    }

    #[test]
    fn hfr_labels_add_annotation_pixels_beside_stars() {
        let fits = star_frame();
        let params = crate::hocus_focus_star_detection::HocusFocusParams {
            structure_removal: StructureRemovalMethod::Atrous,
            noise_reduction_radius: 0,
            ..Default::default()
        };
        let color = Rgb([255u8, 255, 0]);
        let count_annotation =
            |img: &ImageBuffer<Rgb<u8>, Vec<u8>>| img.pixels().filter(|p| **p == color).count();

        let plain = create_annotated_image(&fits, &params, 100, 0.2, -2.8, color, None).unwrap();
        let labeled =
            create_annotated_image(&fits, &params, 100, 0.2, -2.8, color, Some(2)).unwrap();

        let plain_pixels = count_annotation(&plain);
        let labeled_pixels = count_annotation(&labeled);
        assert!(plain_pixels > 0, "circles drawn");
        // Each label is 4 glyphs of a 5x7 font at scale 2: substantially
        // more annotation pixels than circles alone.
        assert!(
            labeled_pixels > plain_pixels + 100,
            "labels add pixels: {labeled_pixels} vs {plain_pixels}"
        );
        // Zero scale means no labels.
        let zero = create_annotated_image(&fits, &params, 100, 0.2, -2.8, color, Some(0)).unwrap();
        assert_eq!(count_annotation(&zero), plain_pixels);
    }
}
