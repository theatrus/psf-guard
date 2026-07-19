/// Star selection strategies for PSF visualization
use crate::hocus_focus_star_detection::HocusFocusStar;

#[allow(dead_code)]
pub enum SelectionStrategy {
    /// Top N stars by specified metric
    TopN { n: usize, metric: SortMetric },
    /// Stars from five regions (four quadrants + center)
    FiveRegions { per_region: usize },
    /// Stars with diverse quality scores
    QualityRange { per_tier: usize },
    /// Stars from corners and edges (9 positions: 4 corners + 4 edges + center)
    Corners,
    /// Custom selection based on criteria
    Custom {
        min_hfr: Option<f64>,
        max_hfr: Option<f64>,
        min_r2: Option<f64>,
    },
}

pub enum SortMetric {
    Hfr,
    R2,
    Brightness,
}

/// Select stars based on the given strategy
pub fn select_stars(
    stars: Vec<HocusFocusStar>,
    strategy: &SelectionStrategy,
    image_width: usize,
    image_height: usize,
) -> Vec<HocusFocusStar> {
    match strategy {
        SelectionStrategy::TopN { n, metric } => select_top_n(stars, *n, metric),
        SelectionStrategy::FiveRegions { per_region } => {
            select_five_regions(stars, *per_region, image_width, image_height)
        }
        SelectionStrategy::QualityRange { per_tier } => select_quality_range(stars, *per_tier),
        SelectionStrategy::Corners => select_corners(stars, image_width, image_height),
        SelectionStrategy::Custom {
            min_hfr,
            max_hfr,
            min_r2,
        } => select_custom(stars, *min_hfr, *max_hfr, *min_r2),
    }
}

fn select_top_n(
    mut stars: Vec<HocusFocusStar>,
    n: usize,
    metric: &SortMetric,
) -> Vec<HocusFocusStar> {
    // Sort by the specified metric
    match metric {
        SortMetric::Hfr => stars.sort_by(|a, b| a.hfr.partial_cmp(&b.hfr).unwrap()),
        SortMetric::R2 => stars.sort_by(|a, b| {
            let r2_a = a.psf_model.as_ref().map(|m| m.r_squared).unwrap_or(0.0);
            let r2_b = b.psf_model.as_ref().map(|m| m.r_squared).unwrap_or(0.0);
            r2_b.partial_cmp(&r2_a).unwrap() // Higher R² first
        }),
        SortMetric::Brightness => {
            stars.sort_by(|a, b| b.brightness.partial_cmp(&a.brightness).unwrap())
        }
    }

    stars.into_iter().take(n).collect()
}

fn select_five_regions(
    stars: Vec<HocusFocusStar>,
    per_region: usize,
    image_width: usize,
    image_height: usize,
) -> Vec<HocusFocusStar> {
    let center_x = image_width as f64 / 2.0;
    let center_y = image_height as f64 / 2.0;

    // Define regions
    let mut top_left = Vec::new();
    let mut top_right = Vec::new();
    let mut bottom_left = Vec::new();
    let mut bottom_right = Vec::new();
    let mut center = Vec::new();

    // Center region is within 25% of image dimensions from center
    let center_radius_x = image_width as f64 * 0.25;
    let center_radius_y = image_height as f64 * 0.25;

    // Categorize stars by region
    for star in stars {
        let (x, y) = star.position;

        // Check if in center region
        if (x - center_x).abs() < center_radius_x && (y - center_y).abs() < center_radius_y {
            center.push(star);
        } else if x < center_x && y < center_y {
            top_left.push(star);
        } else if x >= center_x && y < center_y {
            top_right.push(star);
        } else if x < center_x && y >= center_y {
            bottom_left.push(star);
        } else {
            bottom_right.push(star);
        }
    }

    // Sort each region by HFR (best first)
    let sort_by_hfr = |stars: &mut Vec<HocusFocusStar>| {
        stars.sort_by(|a, b| a.hfr.partial_cmp(&b.hfr).unwrap());
    };

    sort_by_hfr(&mut top_left);
    sort_by_hfr(&mut top_right);
    sort_by_hfr(&mut bottom_left);
    sort_by_hfr(&mut bottom_right);
    sort_by_hfr(&mut center);

    // Select top stars from each region
    let mut selected = Vec::new();
    selected.extend(top_left.into_iter().take(per_region));
    selected.extend(top_right.into_iter().take(per_region));
    selected.extend(bottom_left.into_iter().take(per_region));
    selected.extend(bottom_right.into_iter().take(per_region));
    selected.extend(center.into_iter().take(per_region));

    selected
}

fn select_quality_range(stars: Vec<HocusFocusStar>, per_tier: usize) -> Vec<HocusFocusStar> {
    // Filter stars with PSF models
    let mut stars_with_psf: Vec<_> = stars
        .into_iter()
        .filter(|s| s.psf_model.is_some())
        .collect();

    if stars_with_psf.is_empty() {
        return Vec::new();
    }

    // Sort by R² value
    stars_with_psf.sort_by(|a, b| {
        let r2_a = a.psf_model.as_ref().unwrap().r_squared;
        let r2_b = b.psf_model.as_ref().unwrap().r_squared;
        r2_b.partial_cmp(&r2_a).unwrap()
    });

    let _total = stars_with_psf.len();
    let mut selected = Vec::new();

    // Define quality tiers
    let tiers = [
        (0.9, 1.0, "Excellent"), // R² > 0.9
        (0.7, 0.9, "Good"),      // 0.7 < R² <= 0.9
        (0.5, 0.7, "Fair"),      // 0.5 < R² <= 0.7
        (0.0, 0.5, "Poor"),      // R² <= 0.5
    ];

    for (min_r2, max_r2, _name) in &tiers {
        let tier_stars: Vec<_> = stars_with_psf
            .iter()
            .filter(|s| {
                let r2 = s.psf_model.as_ref().unwrap().r_squared;
                r2 > *min_r2 && r2 <= *max_r2
            })
            .take(per_tier)
            .cloned()
            .collect();

        selected.extend(tier_stars);
    }

    // If we don't have enough from tiers, add some from the top
    if selected.len() < per_tier * 4 {
        let remaining = per_tier * 4 - selected.len();
        let additional: Vec<_> = stars_with_psf
            .into_iter()
            .filter(|s| !selected.iter().any(|sel| sel.position == s.position))
            .take(remaining)
            .collect();
        selected.extend(additional);
    }

    selected
}

fn select_corners(
    stars: Vec<HocusFocusStar>,
    image_width: usize,
    image_height: usize,
) -> Vec<HocusFocusStar> {
    // Define 9 regions for 3x3 grid
    let margin = 0.15; // Use 15% margin from edges
    let x_min = image_width as f64 * margin;
    let x_max = image_width as f64 * (1.0 - margin);
    let y_min = image_height as f64 * margin;
    let y_max = image_height as f64 * (1.0 - margin);

    let x_mid = image_width as f64 / 2.0;
    let y_mid = image_height as f64 / 2.0;

    // Define 9 target regions with their centers
    let regions = [
        // Top row
        (x_min, y_min, "top-left"),
        (x_mid, y_min, "top-center"),
        (x_max, y_min, "top-right"),
        // Middle row
        (x_min, y_mid, "mid-left"),
        (x_mid, y_mid, "center"),
        (x_max, y_mid, "mid-right"),
        // Bottom row
        (x_min, y_max, "bottom-left"),
        (x_mid, y_max, "bottom-center"),
        (x_max, y_max, "bottom-right"),
    ];

    let mut selected = Vec::new();

    // For each region, find the closest star with good HFR
    for (target_x, target_y, _name) in &regions {
        // Find best star closest to this position
        let best_star = stars
            .iter()
            .filter(|s| {
                // Only consider stars with reasonable HFR
                s.hfr > 1.0 && s.hfr < 10.0
            })
            .min_by(|a, b| {
                // Calculate distances to target position
                let dist_a =
                    ((a.position.0 - target_x).powi(2) + (a.position.1 - target_y).powi(2)).sqrt();
                let dist_b =
                    ((b.position.0 - target_x).powi(2) + (b.position.1 - target_y).powi(2)).sqrt();

                // Sort by distance first, then by HFR if distances are similar
                if (dist_a - dist_b).abs() < 50.0 {
                    a.hfr.partial_cmp(&b.hfr).unwrap()
                } else {
                    dist_a.partial_cmp(&dist_b).unwrap()
                }
            });

        if let Some(star) = best_star {
            // Avoid duplicates
            if !selected
                .iter()
                .any(|s: &HocusFocusStar| s.position == star.position)
            {
                selected.push(star.clone());
            }
        }
    }

    // Sort by position (top to bottom, left to right) for consistent ordering
    selected.sort_by(|a, b| {
        if (a.position.1 - b.position.1).abs() < 100.0 {
            // Same row, sort by X
            a.position.0.partial_cmp(&b.position.0).unwrap()
        } else {
            // Different rows, sort by Y
            a.position.1.partial_cmp(&b.position.1).unwrap()
        }
    });

    selected
}

fn select_custom(
    stars: Vec<HocusFocusStar>,
    min_hfr: Option<f64>,
    max_hfr: Option<f64>,
    min_r2: Option<f64>,
) -> Vec<HocusFocusStar> {
    stars
        .into_iter()
        .filter(|s| {
            // HFR filter
            if let Some(min) = min_hfr
                && s.hfr < min
            {
                return false;
            }
            if let Some(max) = max_hfr
                && s.hfr > max
            {
                return false;
            }

            // R² filter
            if let Some(min) = min_r2 {
                if let Some(psf) = &s.psf_model {
                    if psf.r_squared < min {
                        return false;
                    }
                } else {
                    return false; // No PSF model
                }
            }

            true
        })
        .collect()
}
