use anyhow::Result;
use image::{ImageBuffer, Rgba};
use imageproc::drawing::draw_hollow_rect_mut;
use imageproc::rect::Rect;

use crate::hocus_focus_star_detection::{detect_stars_hocus_focus, HocusFocusParams};
use crate::image_analysis::FitsImage;
use crate::psf_fitting::{PSFFitter, PSFType};

use super::visualize_psf::star_selection::{select_stars, SelectionStrategy, SortMetric};
use super::visualize_psf::text_render::{draw_text, draw_text_with_bg};

/// Create a heatmap color from value (0.0 to 1.0)
fn heatmap_color(value: f64, mode: &str) -> (u8, u8, u8) {
    let clamped = value.clamp(0.0, 1.0);

    match mode {
        "residual" => {
            // Red-white-blue for residuals (negative to positive)
            if value < 0.5 {
                // Blue to white (negative residuals)
                let t = value * 2.0;
                let r = (255.0 * t) as u8;
                let g = (255.0 * t) as u8;
                let b = 255;
                (r, g, b)
            } else {
                // White to red (positive residuals)
                let t = (value - 0.5) * 2.0;
                let r = 255;
                let g = (255.0 * (1.0 - t)) as u8;
                let b = (255.0 * (1.0 - t)) as u8;
                (r, g, b)
            }
        }
        _ => {
            // Grayscale for observed/fitted
            let gray = (255.0 * clamped) as u8;
            (gray, gray, gray)
        }
    }
}

/// Generate PSF multi visualization image
pub fn create_psf_multi_image(
    fits: &FitsImage,
    num_stars: usize,
    psf_type: PSFType,
    sort_by: &str,
    grid_cols: Option<usize>,
    selection_mode: &str,
) -> Result<ImageBuffer<Rgba<u8>, Vec<u8>>> {
    let width = fits.width;
    let height = fits.height;

    // Detect stars using HocusFocus
    let params = HocusFocusParams {
        psf_type,
        ..Default::default()
    };

    let result = detect_stars_hocus_focus(&fits.data, width, height, &params);

    if result.stars.is_empty() {
        anyhow::bail!("No stars detected in image");
    }

    // Filter stars with PSF fits
    let stars_with_psf: Vec<_> = result
        .stars
        .into_iter()
        .filter(|s| s.psf_model.is_some())
        .collect();

    if stars_with_psf.is_empty() {
        anyhow::bail!("No stars with successful PSF fits found");
    }

    // Parse sort metric
    let sort_metric = match sort_by {
        "hfr" => SortMetric::Hfr,
        "r2" => SortMetric::R2,
        "brightness" => SortMetric::Brightness,
        _ => SortMetric::R2,
    };

    // Select stars based on strategy
    let strategy = match selection_mode {
        "regions" => SelectionStrategy::FiveRegions {
            per_region: num_stars.div_ceil(5),
        },
        "quality" => SelectionStrategy::QualityRange {
            per_tier: num_stars.div_ceil(4),
        },
        "corners" => SelectionStrategy::Corners,
        _ => SelectionStrategy::TopN {
            n: num_stars,
            metric: sort_metric,
        },
    };

    let stars_to_show = select_stars(stars_with_psf, &strategy, width, height);

    if stars_to_show.is_empty() {
        anyhow::bail!("No stars selected with the given criteria");
    }

    // Calculate square grid layout
    let num_stars_actual = stars_to_show.len();
    let grid_size = (num_stars_actual as f64).sqrt().ceil() as usize;
    let grid_cols = grid_cols.unwrap_or_else(|| {
        if selection_mode == "corners" {
            3
        } else {
            grid_size
        }
    });
    let num_rows = num_stars_actual.div_ceil(grid_cols);

    // Panel dimensions
    let panel_size = 200; // Smaller panels for better fit
    let panel_spacing = 15;

    // Each star gets 3 panels (observed, fitted, residual)
    let star_panel_width = panel_size * 3 + panel_spacing * 2;
    let star_panel_height = panel_size + 120; // Extra space for two lines of larger text

    // Total image size
    let total_width = grid_cols * star_panel_width + (grid_cols - 1) * panel_spacing + 40;
    let total_height = num_rows * star_panel_height + (num_rows - 1) * panel_spacing + 40;

    // Calculate minimap size maintaining aspect ratio
    let max_map_width = 800;
    let max_map_height = 600;
    let aspect_ratio = width as f64 / height as f64;

    let (map_width, map_height) = if aspect_ratio > (max_map_width as f64 / max_map_height as f64) {
        // Image is wider - constrain by width
        let map_w = max_map_width.min(width);
        let map_h = (map_w as f64 / aspect_ratio) as usize;
        (map_w, map_h)
    } else {
        // Image is taller - constrain by height
        let map_h = max_map_height.min(height);
        let map_w = (map_h as f64 * aspect_ratio) as usize;
        (map_w, map_h)
    };

    let final_width = total_width.max(map_width + 80);
    let final_height = total_height + map_height + 80;

    let mut img = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(final_width as u32, final_height as u32);

    // Fill background
    for pixel in img.pixels_mut() {
        *pixel = Rgba([30, 30, 30, 255]); // Dark gray background
    }

    // Generate residual maps for each star
    let fitter = PSFFitter::new(psf_type);

    for (star_idx, star) in stars_to_show.iter().enumerate() {
        let row = star_idx / grid_cols;
        let col = star_idx % grid_cols;

        let x_offset = 20 + col * (star_panel_width + panel_spacing);
        let y_offset = 20 + row * (star_panel_height + panel_spacing);

        let psf_model = star.psf_model.as_ref().unwrap();

        // Generate residual maps
        if let Some((observed, fitted, residuals)) = fitter.generate_residuals(
            &fits.data,
            width,
            height,
            star.position.0,
            star.position.1,
            psf_model,
        ) {
            // Normalize data for visualization
            let obs_min = observed
                .iter()
                .flat_map(|row| row.iter())
                .fold(f64::INFINITY, |a, &b| a.min(b));
            let obs_max = observed
                .iter()
                .flat_map(|row| row.iter())
                .fold(f64::NEG_INFINITY, |a, &b| a.max(b));
            let obs_range = obs_max - obs_min;

            let fit_min = fitted
                .iter()
                .flat_map(|row| row.iter())
                .fold(f64::INFINITY, |a, &b| a.min(b));
            let fit_max = fitted
                .iter()
                .flat_map(|row| row.iter())
                .fold(f64::NEG_INFINITY, |a, &b| a.max(b));
            let fit_range = fit_max - fit_min;

            let res_min = residuals
                .iter()
                .flat_map(|row| row.iter())
                .fold(f64::INFINITY, |a, &b| a.min(b));
            let res_max = residuals
                .iter()
                .flat_map(|row| row.iter())
                .fold(f64::NEG_INFINITY, |a, &b| a.max(b));
            let res_absmax = res_min.abs().max(res_max.abs());

            // Draw panels
            let panels = [
                (&observed, "Observed", obs_min, obs_range, "grayscale"),
                (&fitted, "Fitted", fit_min, fit_range, "grayscale"),
                (
                    &residuals,
                    "Residual",
                    -res_absmax,
                    2.0 * res_absmax,
                    "residual",
                ),
            ];

            for (panel_idx, (data, _title, min_val, range, color_mode)) in panels.iter().enumerate()
            {
                let panel_x = x_offset + panel_idx * (panel_size + panel_spacing);
                let panel_y = y_offset + 40;

                // Draw panel data with scaling
                let data_size = data.len();
                let scale_factor = panel_size as f64 / data_size as f64;

                for py in 0..panel_size {
                    for px in 0..panel_size {
                        // Map panel coordinates back to data coordinates
                        let data_y = (py as f64 / scale_factor) as usize;
                        let data_x = (px as f64 / scale_factor) as usize;

                        if data_y < data_size && data_x < data[data_y].len() {
                            let value = data[data_y][data_x];
                            let normalized = if *range > 0.0 {
                                (value - min_val) / range
                            } else {
                                0.5
                            };

                            let (r, g, b) = heatmap_color(normalized, color_mode);
                            img.put_pixel(
                                (panel_x + px) as u32,
                                (panel_y + py) as u32,
                                Rgba([r, g, b, 255]),
                            );
                        }
                    }
                }

                // Draw panel border with better color
                draw_hollow_rect_mut(
                    &mut img,
                    Rect::at(panel_x as i32 - 1, panel_y as i32 - 1)
                        .of_size((panel_size + 2) as u32, (panel_size + 2) as u32),
                    Rgba([200, 200, 200, 255]), // Lighter gray for better visibility
                );

                // Draw title with larger text
                let title_text = match panel_idx {
                    0 => "OBSERVED",
                    1 => "FITTED",
                    2 => "RESIDUAL",
                    _ => "",
                };
                draw_text_with_bg(
                    &mut img,
                    (panel_x + 5) as u32,
                    (panel_y - 20) as u32,
                    title_text,
                    Rgba([255, 255, 255, 255]),
                    Rgba([50, 50, 50, 255]),
                    2, // larger scale
                );
            }

            // Star information with better formatting
            let info_y = y_offset + panel_size + 50;

            // Draw star number with color
            let star_label = format!("Star #{}", star_idx + 1);
            draw_text_with_bg(
                &mut img,
                x_offset as u32,
                info_y as u32,
                &star_label,
                Rgba([255, 220, 0, 255]), // Golden yellow for star number
                Rgba([40, 40, 40, 255]),
                2, // larger scale
            );

            // Draw metrics on the next line with more spacing for larger text
            let metrics_y = info_y + 35;
            let metrics_text = format!(
                "HFR: {:.2}  FWHM: {:.2}  R²: {:.3}",
                star.hfr, psf_model.fwhm, psf_model.r_squared
            );

            // Color code based on R² value
            let text_color = if psf_model.r_squared > 0.95 {
                Rgba([0, 255, 0, 255]) // Green for excellent fit
            } else if psf_model.r_squared > 0.90 {
                Rgba([255, 255, 0, 255]) // Yellow for good fit
            } else if psf_model.r_squared > 0.85 {
                Rgba([255, 165, 0, 255]) // Orange for acceptable fit
            } else {
                Rgba([255, 100, 100, 255]) // Light red for poor fit
            };

            draw_text_with_bg(
                &mut img,
                x_offset as u32,
                metrics_y as u32,
                &metrics_text,
                text_color,
                Rgba([30, 30, 30, 240]), // Dark semi-transparent background
                2,                       // larger scale for better readability
            );
        }
    }

    // Draw location map at bottom
    let map_y_offset = 20 + num_rows * (star_panel_height + panel_spacing);
    let map_x_offset = (final_width - map_width) / 2;

    // Create minimap with a simplified view of the image
    // Calculate proper statistics for visualization
    let stats = fits.calculate_basic_statistics();

    // Apply MTF stretch for better visibility
    use crate::mtf_stretch::{stretch_image, StretchParameters};
    let stretch_params = StretchParameters {
        factor: 0.25,         // Stronger stretch for better minimap visibility
        black_clipping: -2.0, // Less aggressive black clipping
    };
    let stretched = stretch_image(
        &fits.data,
        &stats,
        stretch_params.factor,
        stretch_params.black_clipping,
    );

    // Draw downsampled image as minimap background
    for y in 0..map_height {
        for x in 0..map_width {
            // Map minimap coordinates to image coordinates
            let img_x = (x as f64 * width as f64 / map_width as f64) as usize;
            let img_y = (y as f64 * height as f64 / map_height as f64) as usize;

            if img_x < width && img_y < height {
                let idx = img_y * width + img_x;
                let value = stretched[idx];
                // Convert 16-bit stretched value to 8-bit with better visibility
                let gray = ((value >> 8) as f64 * 0.8).min(200.0) as u8; // Scale to 80% brightness, cap at 200
                img.put_pixel(
                    (map_x_offset + x) as u32,
                    (map_y_offset + y) as u32,
                    Rgba([gray, gray, gray, 255]),
                );
            }
        }
    }

    // Draw minimap border
    draw_hollow_rect_mut(
        &mut img,
        Rect::at(map_x_offset as i32 - 1, map_y_offset as i32 - 1)
            .of_size((map_width + 2) as u32, (map_height + 2) as u32),
        Rgba([100, 100, 100, 255]),
    );

    // Draw title for map
    draw_text(
        &mut img,
        (map_x_offset + map_width / 2 - 50) as u32,
        (map_y_offset - 20) as u32,
        "Star Locations",
        Rgba([255, 255, 255, 255]),
        2, // scale
    );

    // Draw all detected stars as small dots
    let x_scale = map_width as f64 / width as f64;
    let y_scale = map_height as f64 / height as f64;

    // Draw selected stars with numbers
    for (idx, star) in stars_to_show.iter().enumerate() {
        let map_x = (star.position.0 * x_scale) as i32 + map_x_offset as i32;
        let map_y = (star.position.1 * y_scale) as i32 + map_y_offset as i32;

        // Draw star marker
        for dy in -2..=2 {
            for dx in -2..=2 {
                if dx * dx + dy * dy <= 4 {
                    let px = (map_x + dx) as u32;
                    let py = (map_y + dy) as u32;
                    if px < final_width as u32 && py < final_height as u32 {
                        img.put_pixel(px, py, Rgba([255, 255, 0, 255])); // Yellow
                    }
                }
            }
        }

        // Draw star number
        let label = format!("{}", idx + 1);
        draw_text_with_bg(
            &mut img,
            (map_x + 5) as u32,
            (map_y - 10) as u32,
            &label,
            Rgba([255, 255, 255, 255]),
            Rgba([0, 0, 0, 200]),
            1, // scale
        );
    }

    Ok(img)
}
