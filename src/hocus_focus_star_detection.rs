use crate::psf_fitting::{PSFModel, PSFType};
/// HocusFocus star detection algorithm
/// Based on the HocusFocus plugin for N.I.N.A. by George Hilios
/// Original: https://github.com/ghilios/joko.nina.plugins
///
/// This implementation uses more sophisticated detection than standard NINA:
/// - Wavelet decomposition to remove large structures (nebulae)
/// - Kappa-Sigma noise estimation for adaptive thresholding
/// - Hot pixel filtering
/// - Multi-criteria star validation
use seiza_imgproc::morphology::{KernelShape, MorphBorder, StructuringElement};
use seiza_imgproc::wavelets::StructureRemover;

/// Star detection parameters for HocusFocus algorithm
#[derive(Debug, Clone)]
pub struct HocusFocusParams {
    // Preprocessing
    pub hotpixel_filtering: bool,
    pub hotpixel_threshold: f64, // Percent of max ADU for hot pixel threshold
    pub noise_reduction_radius: usize, // Half-size of Gaussian kernel

    // Image processing runs on seiza-imgproc (pure Rust, OpenCV-verified)

    // Structure detection
    pub structure_layers: usize, // Number of wavelet layers for large structure removal
    pub noise_clipping_multiplier: f64, // Sigma multiplier for noise threshold
    pub star_clipping_multiplier: f64, // Sigma multiplier for star pixel filtering

    // Star validation criteria
    pub min_star_size: usize,
    pub max_star_size: usize,
    pub sensitivity: f64,    // Minimum (signal - background)/noise ratio
    pub peak_response: f64,  // Reject if median >= peak_response * peak
    pub max_distortion: f64, // Min pixel density (pixels/area)
    pub background_box_expansion: usize, // Pixels to expand for background estimation
    pub star_center_tolerance: f64, // Fraction of box size for center tolerance
    pub saturation_threshold: f64, // ADU value for saturation
    pub min_hfr: f64,        // Minimum HFR threshold

    // PSF fitting
    pub psf_type: PSFType, // PSF model type to fit (None, Gaussian, Moffat4)
}

impl Default for HocusFocusParams {
    fn default() -> Self {
        Self {
            hotpixel_filtering: true,
            hotpixel_threshold: 0.001, // 0.1% of max ADU
            noise_reduction_radius: 4, // Actual default from user

            // seiza-imgproc structure removal and morphology
            structure_layers: 4,
            noise_clipping_multiplier: 4.0,
            star_clipping_multiplier: 2.0,
            min_star_size: 5, // Minimum bounding box size - actual default
            max_star_size: 150,
            sensitivity: 10.0,                    // Brightness sensitivity
            peak_response: 0.75,                  // 75% - actual default
            max_distortion: 0.5,                  // Actual default
            background_box_expansion: 3,          // Actual default
            star_center_tolerance: 0.3,           // 30% - actual default
            saturation_threshold: 65535.0 * 0.99, // 99% of max
            min_hfr: 1.5,                         // Actual default
            psf_type: PSFType::None,              // No PSF fitting by default
        }
    }
}

/// Detected star information
#[derive(Debug, Clone)]
pub struct HocusFocusStar {
    pub position: (f64, f64),
    pub hfr: f64,
    pub fwhm: f64,
    pub brightness: f64,
    pub background: f64,
    pub snr: f64, // Signal-to-noise ratio
    pub flux: f64,
    pub pixel_count: usize,
    pub psf_model: Option<PSFModel>, // PSF fitting results
}

/// Star detection result
#[derive(Debug, Clone)]
pub struct HocusFocusDetectionResult {
    pub stars: Vec<HocusFocusStar>,
    pub average_hfr: f64,
    pub average_fwhm: f64,
    pub noise_sigma: f64,
    pub background_mean: f64,
}

/// Kappa-Sigma noise estimation result
#[derive(Debug, Clone)]
struct KappaSigmaResult {
    pub sigma: f64,
    pub background_mean: f64,
}

/// Main star detection function using HocusFocus algorithm
pub fn detect_stars_hocus_focus(
    data: &[u16],
    width: usize,
    height: usize,
    params: &HocusFocusParams,
) -> HocusFocusDetectionResult {
    // Step 1: Apply hot pixel filtering if enabled
    let mut working_data = if params.hotpixel_filtering {
        apply_hotpixel_filter(data, width, height, params.hotpixel_threshold)
    } else {
        data.to_vec()
    };

    // Step 2: Apply noise reduction if configured
    if params.noise_reduction_radius > 0 {
        // HocusFocus uses kernel_size = radius * 2 + 1
        let kernel_size = params.noise_reduction_radius * 2 + 1;
        working_data = apply_gaussian_blur(&working_data, width, height, kernel_size);
    }

    // Step 3: Create structure map by removing large structures
    let structure_map = match create_structure_map(&working_data, width, height, params) {
        Ok(map) => map,
        Err(e) => {
            eprintln!("Error creating structure map: {}", e);
            return HocusFocusDetectionResult {
                stars: vec![],
                average_hfr: 0.0,
                average_fwhm: 0.0,
                noise_sigma: 0.0,
                background_mean: 0.0,
            };
        }
    };

    // Step 4: Estimate noise using Kappa-Sigma method
    let noise_estimate = kappa_sigma_noise_estimate(
        &structure_map,
        width,
        height,
        params.noise_clipping_multiplier,
    );

    // Debug output
    crate::debug_detection!(
        "Debug HocusFocus: noise_sigma: {:.3}, background_mean: {:.3}",
        noise_estimate.sigma,
        noise_estimate.background_mean
    );

    // Step 5: Binarize structure map using noise threshold
    let median = calculate_median(&structure_map);
    let threshold = median + params.noise_clipping_multiplier * noise_estimate.sigma;

    crate::debug_detection!(
        "Debug HocusFocus: median: {:.3}, threshold: {:.3}",
        median,
        threshold
    );
    let mut binary_map = binarize(&structure_map, threshold);

    // Debug: Count non-zero pixels in binary map
    let non_zero = binary_map.iter().filter(|&&x| x).count();
    crate::debug_detection!(
        "Debug HocusFocus: Binary map has {} non-zero pixels ({:.2}%)",
        non_zero,
        non_zero as f64 / binary_map.len() as f64 * 100.0
    );

    // Apply erosion to break up connected components
    if non_zero > structure_map.len() / 100 {
        // If more than 1% of pixels are set
        binary_map = match apply_erosion(&binary_map, width, height) {
            Ok(map) => map,
            Err(e) => {
                eprintln!("Error applying erosion: {}", e);
                return HocusFocusDetectionResult {
                    stars: vec![],
                    average_hfr: 0.0,
                    average_fwhm: 0.0,
                    noise_sigma: 0.0,
                    background_mean: 0.0,
                };
            }
        };
        if crate::debug::is_debug_enabled() {
            let eroded_count = binary_map.iter().filter(|&&x| x).count();
            crate::debug_detection!(
                "Debug HocusFocus: After erosion: {} non-zero pixels ({:.2}%)",
                eroded_count,
                eroded_count as f64 / binary_map.len() as f64 * 100.0
            );
        }
    }

    // Step 6: Find star candidates
    let candidates = find_star_candidates(&binary_map, width, height, params);
    crate::debug_detection!(
        "Debug HocusFocus: Found {} star candidates",
        candidates.len()
    );

    // Step 7: Measure and validate stars
    let stars = measure_stars(
        &working_data,
        width,
        height,
        candidates,
        params,
        &noise_estimate,
    );
    crate::debug_detection!("Debug HocusFocus: {} stars passed validation", stars.len());

    // Calculate statistics
    let average_hfr = if !stars.is_empty() {
        stars.iter().map(|s| s.hfr).sum::<f64>() / stars.len() as f64
    } else {
        0.0
    };

    let average_fwhm = if !stars.is_empty() {
        stars.iter().map(|s| s.fwhm).sum::<f64>() / stars.len() as f64
    } else {
        0.0
    };

    HocusFocusDetectionResult {
        stars,
        average_hfr,
        average_fwhm,
        noise_sigma: noise_estimate.sigma,
        background_mean: noise_estimate.background_mean,
    }
}

/// Apply hot pixel filtering using 3x3 median filter
fn apply_hotpixel_filter(
    data: &[u16],
    width: usize,
    height: usize,
    threshold_percent: f64,
) -> Vec<u16> {
    let mut result = data.to_vec();
    let max_adu = 65535.0;
    let threshold = threshold_percent * max_adu;
    if width < 3 || height < 3 {
        return result;
    }

    // Median of each interior 3x3 neighborhood via Devillard's 19-op
    // sorting network, applied elementwise over shifted row slices so the
    // whole row's min/max ops vectorize. Same median values as sorting each
    // neighborhood, without a per-pixel allocation and sort.
    let ilen = width - 2;
    let mut p: Vec<Vec<u16>> = vec![vec![0u16; ilen]; 9];
    const NET: [(usize, usize); 19] = [
        (1, 2),
        (4, 5),
        (7, 8),
        (0, 1),
        (3, 4),
        (6, 7),
        (1, 2),
        (4, 5),
        (7, 8),
        (0, 3),
        (5, 8),
        (4, 7),
        (3, 6),
        (1, 4),
        (2, 5),
        (4, 7),
        (4, 2),
        (6, 4),
        (4, 2),
    ];
    for y in 1..(height - 1) {
        for (r, sy) in [y - 1, y, y + 1].into_iter().enumerate() {
            let srow = &data[sy * width..(sy + 1) * width];
            p[r * 3].copy_from_slice(&srow[..ilen]);
            p[r * 3 + 1].copy_from_slice(&srow[1..1 + ilen]);
            p[r * 3 + 2].copy_from_slice(&srow[2..2 + ilen]);
        }
        for &(a, b) in NET.iter() {
            let (lo, hi) = (a.min(b), a.max(b));
            let (head, tail) = p.split_at_mut(hi);
            let (pa, pb) = (&mut head[lo], &mut tail[0]);
            if a < b {
                for (x, y) in pa.iter_mut().zip(pb.iter_mut()) {
                    let (mn, mx) = ((*x).min(*y), (*x).max(*y));
                    *x = mn;
                    *y = mx;
                }
            } else {
                for (y, x) in pa.iter_mut().zip(pb.iter_mut()) {
                    let (mn, mx) = ((*x).min(*y), (*x).max(*y));
                    *x = mn;
                    *y = mx;
                }
            }
        }
        let out = &mut result[y * width + 1..y * width + 1 + ilen];
        let center = &data[y * width + 1..y * width + 1 + ilen];
        for ((o, &c), &m) in out.iter_mut().zip(center.iter()).zip(p[4].iter()) {
            if (c as f64 - m as f64).abs() > threshold {
                *o = m;
            }
        }
    }

    result
}

/// Apply Gaussian blur for noise reduction
fn apply_gaussian_blur(data: &[u16], width: usize, height: usize, kernel_size: usize) -> Vec<u16> {
    // Generate Gaussian kernel
    let radius = kernel_size / 2;
    let mut kernel = vec![0.0; kernel_size * kernel_size];
    let sigma = radius as f64 / 2.0;
    let two_sigma_sq = 2.0 * sigma * sigma;
    let mut sum = 0.0;

    for y in 0..kernel_size {
        for x in 0..kernel_size {
            let dx = x as f64 - radius as f64;
            let dy = y as f64 - radius as f64;
            let value = (-((dx * dx + dy * dy) / two_sigma_sq)).exp();
            kernel[y * kernel_size + x] = value;
            sum += value;
        }
    }

    // Normalize kernel
    for k in kernel.iter_mut() {
        *k /= sum;
    }

    // Apply convolution. HocusFocus uses the full 2D kernel (not a
    // separable pair), so keep it — but accumulate axis-swapped: for each
    // tap, add tap * shifted-row elementwise into a row accumulator. Every
    // output pixel still receives its terms in the same (ky, kx) order as
    // the naive per-pixel loop, so the result is bit-identical, and the
    // inner loop vectorizes instead of doing k*k scalar ops per pixel.
    let mut result = vec![0u16; width * height];
    if width < kernel_size || height < kernel_size {
        return result;
    }
    let ilen = width - 2 * radius;
    let mut acc = vec![0f64; ilen];
    for y in radius..(height - radius) {
        acc.fill(0.0);
        for ky in 0..kernel_size {
            let srow = &data[(y + ky - radius) * width..(y + ky - radius + 1) * width];
            for kx in 0..kernel_size {
                let kv = kernel[ky * kernel_size + kx];
                let s = &srow[kx..kx + ilen];
                for (a, &v) in acc.iter_mut().zip(s.iter()) {
                    *a += v as f64 * kv;
                }
            }
        }
        let orow = &mut result[y * width + radius..y * width + radius + ilen];
        for (o, &a) in orow.iter_mut().zip(acc.iter()) {
            *o = a as u16;
        }
    }

    result
}

/// Create structure map by subtracting wavelet residual layer
fn create_structure_map(
    data: &[u16],
    width: usize,
    height: usize,
    params: &HocusFocusParams,
) -> Result<Vec<f64>, Box<dyn std::error::Error>> {
    // Multi-scale structure removal (Gaussian + domain-transform layers,
    // reproducing the former OpenCV filter pipeline bit-for-bit in intent).
    // The pipeline is f32 internally and u16 camera values are exact in
    // f32, so the f32 entry point skips two full-image f64 conversions
    // while producing bit-identical residuals; the subtraction below is
    // done in f64 exactly as before (each operand widens exactly).
    let float_data: Vec<f32> = data.iter().map(|&v| v as f32).collect();
    let wavelet_remover = StructureRemover::new(params.structure_layers);
    let residual = wavelet_remover.remove_structures_filtered_f32(&float_data, width, height);

    // Subtract residual from original to remove large structures
    let mut structure_map = vec![0f64; data.len()];
    for i in 0..structure_map.len() {
        structure_map[i] = (data[i] as f64 - residual[i] as f64).max(0.0);
    }

    // Debug statistics cost six full passes over the map — only compute
    // them when debug output is actually enabled.
    if crate::debug::is_debug_enabled() {
        let min = structure_map.iter().fold(f64::INFINITY, |a, &b| a.min(b));
        let max = structure_map
            .iter()
            .fold(f64::NEG_INFINITY, |a, &b| a.max(b));
        let non_zero = structure_map.iter().filter(|&&v| v > 0.0).count();
        let above_10 = structure_map.iter().filter(|&&v| v > 10.0).count();
        let above_50 = structure_map.iter().filter(|&&v| v > 50.0).count();
        let above_100 = structure_map.iter().filter(|&&v| v > 100.0).count();

        crate::debug_detection!(
            "Debug structure_map: min={:.1}, max={:.1}, non_zero={} ({:.1}%)",
            min,
            max,
            non_zero,
            non_zero as f64 / structure_map.len() as f64 * 100.0
        );
        crate::debug_detection!(
            "  Above 10: {} ({:.1}%), Above 50: {} ({:.1}%), Above 100: {} ({:.1}%)",
            above_10,
            above_10 as f64 / structure_map.len() as f64 * 100.0,
            above_50,
            above_50 as f64 / structure_map.len() as f64 * 100.0,
            above_100,
            above_100 as f64 / structure_map.len() as f64 * 100.0
        );
    }

    // Apply smoothing to blend edges
    let kernel_size = params.structure_layers * 2 + 1;
    smooth_gaussian(&mut structure_map, width, height, kernel_size);

    Ok(structure_map)
}

/// Smooth with Gaussian kernel
fn smooth_gaussian(data: &mut [f64], width: usize, height: usize, kernel_size: usize) {
    let sigma = kernel_size as f64 / 3.0;
    let radius = kernel_size / 2;

    // Generate kernel
    let mut kernel = vec![0.0; kernel_size * kernel_size];
    let mut sum = 0.0;
    for y in 0..kernel_size {
        for x in 0..kernel_size {
            let dx = x as f64 - radius as f64;
            let dy = y as f64 - radius as f64;
            let value = (-(dx * dx + dy * dy) / (2.0 * sigma * sigma)).exp();
            kernel[y * kernel_size + x] = value;
            sum += value;
        }
    }
    for k in kernel.iter_mut() {
        *k /= sum;
    }

    // Apply convolution, axis-swapped like apply_gaussian_blur: per tap,
    // add tap * shifted-row into a row accumulator. Each output pixel
    // receives its terms in the same (ky, kx) order as the naive loop, so
    // the sums are bit-identical, and the inner loop vectorizes.
    if width < kernel_size || height < kernel_size {
        return;
    }
    let original = data.to_vec();
    let ilen = width - 2 * radius;
    let mut acc = vec![0f64; ilen];
    for y in radius..(height - radius) {
        acc.fill(0.0);
        for ky in 0..kernel_size {
            let srow = &original[(y + ky - radius) * width..(y + ky - radius + 1) * width];
            for kx in 0..kernel_size {
                let kv = kernel[ky * kernel_size + kx];
                let sr = &srow[kx..kx + ilen];
                for (a, &v) in acc.iter_mut().zip(sr.iter()) {
                    *a += v * kv;
                }
            }
        }
        data[y * width + radius..y * width + radius + ilen].copy_from_slice(&acc);
    }
}

/// Kappa-Sigma noise estimation matching HocusFocus implementation
fn kappa_sigma_noise_estimate(
    data: &[f64],
    _width: usize,
    _height: usize,
    clipping_multiplier: f64,
) -> KappaSigmaResult {
    let allowed_error = 0.00001;
    let max_iterations = 5;
    let mut threshold = f64::MAX;
    let mut last_sigma = 1.0;
    let mut last_mean = 1.0;
    let mut num_iterations = 0;

    while num_iterations < max_iterations {
        // Stream the clipped subset instead of collecting it: the filter
        // preserves element order, and the mean and variance passes visit
        // the same values in the same order as they did over the collected
        // mask, so the sums are bit-identical — without allocating up to
        // the full map size on every iteration.
        let keep = |x: &&f64| -> bool {
            num_iterations == 0 || (**x > f64::EPSILON && **x < threshold - f64::EPSILON)
        };

        let mut count = 0usize;
        let mut sum = 0.0f64;
        for x in data.iter().filter(keep) {
            sum += *x;
            count += 1;
        }
        if count == 0 {
            break;
        }

        // Calculate mean and standard deviation
        let mean = sum / count as f64;
        let variance = data
            .iter()
            .filter(keep)
            .map(|&x| (x - mean).powi(2))
            .sum::<f64>()
            / count as f64;
        let sigma = variance.sqrt();

        num_iterations += 1;

        // Check convergence (absolute difference, not relative)
        if num_iterations > 1 {
            let sigma_convergence_error = (sigma - last_sigma).abs();
            if sigma_convergence_error <= allowed_error {
                last_sigma = sigma;
                last_mean = mean;
                break;
            }
        }

        threshold = mean + clipping_multiplier * sigma;
        last_sigma = sigma;
        last_mean = mean;
    }

    KappaSigmaResult {
        sigma: last_sigma,
        background_mean: last_mean,
    }
}

/// Calculate median of data
fn calculate_median(data: &[f64]) -> f64 {
    // Selection instead of a full sort: same median value, O(n) not
    // O(n log n) — this runs over the entire structure map.
    let mut values: Vec<f64> = data.to_vec();
    let len = values.len();
    let (lower, upper, _) =
        values.select_nth_unstable_by(len / 2, |a, b| a.partial_cmp(b).unwrap());
    if len.is_multiple_of(2) {
        let below = lower.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (below + *upper) / 2.0
    } else {
        *upper
    }
}

/// Binarize data using threshold
fn binarize(data: &[f64], threshold: f64) -> Vec<bool> {
    data.iter().map(|&v| v > threshold).collect()
}

/// Convert bool vector to u8 vector for image processing
fn bool_to_u8(binary_map: &[bool]) -> Vec<u8> {
    binary_map
        .iter()
        .map(|&b| if b { 255u8 } else { 0u8 })
        .collect()
}

/// Convert u8 vector back to bool vector
fn u8_to_bool(data: &[u8]) -> Vec<bool> {
    data.iter().map(|&v| v > 127).collect()
}

/// Apply morphological erosion to break up connected components
fn apply_erosion(
    binary_map: &[bool],
    width: usize,
    height: usize,
) -> Result<Vec<bool>, Box<dyn std::error::Error>> {
    let u8_data = bool_to_u8(binary_map);
    // Ellipse is better for breaking up components; reflected border like
    // the former OpenCV call.
    let se = StructuringElement::new(KernelShape::Ellipse, 3);
    let eroded =
        seiza_imgproc::morphology::erode(&u8_data, width, height, &se, MorphBorder::Reflect);
    Ok(u8_to_bool(&eroded))
}

/// Find star candidates from binary map using HocusFocus-style scanning
fn find_star_candidates(
    binary_map: &[bool],
    width: usize,
    height: usize,
    params: &HocusFocusParams,
) -> Vec<StarCandidate> {
    let mut candidates = Vec::new();
    let mut structure_map = binary_map.to_vec();

    let mut total_structures = 0;
    let mut too_small = 0;
    let mut too_large = 0;

    // Scan the image from top-left, expanding rightward and downward
    for y_top in 0..(height - 1) {
        for x_left in 0..(width - 1) {
            let idx = y_top * width + x_left;

            // Skip background pixels and already processed pixels
            if !structure_map[idx] {
                continue;
            }

            total_structures += 1;

            let mut star_pixels = Vec::new();
            let mut star_bounds = (x_left, y_top, 1, 1); // x, y, width, height

            // Grow the star bounding box downward and rightward
            let mut y = y_top;
            loop {
                let mut row_points_added = 0;
                let row_start = y * width;

                // Check if starting pixel is part of star
                let x = x_left;
                if x < width && structure_map[row_start + x] {
                    star_pixels.push((x, y));
                    row_points_added += 1;
                }

                // Expand leftward from starting position
                let mut row_start_x = x;
                if row_points_added > 0 {
                    while row_start_x > 0 && structure_map[row_start + row_start_x - 1] {
                        row_start_x -= 1;
                        star_pixels.push((row_start_x, y));
                        row_points_added += 1;
                    }
                }

                // Expand rightward from starting position
                let mut row_end_x = x;
                while row_end_x < width - 1 {
                    if !structure_map[row_start + row_end_x + 1] {
                        if row_points_added > 0 || row_end_x >= star_bounds.0 + star_bounds.2 {
                            break;
                        }
                        row_end_x += 1;
                    } else {
                        row_end_x += 1;
                        star_pixels.push((row_end_x, y));
                        row_points_added += 1;
                    }
                }

                // Update bounding box
                if row_start_x < star_bounds.0 {
                    star_bounds.2 += star_bounds.0 - row_start_x;
                    star_bounds.0 = row_start_x;
                }
                if row_end_x >= star_bounds.0 + star_bounds.2 {
                    star_bounds.2 = row_end_x - star_bounds.0 + 1;
                }

                // No points added on this row, we're done
                if row_points_added == 0 {
                    star_bounds.3 = y - y_top;
                    break;
                }

                // Reached bottom of image
                if y >= height - 1 {
                    star_bounds.3 = y - y_top + 1;
                    break;
                }

                y += 1;
            }

            // Check size constraints BEFORE clearing the map
            if star_bounds.2 < params.min_star_size || star_bounds.3 < params.min_star_size {
                too_small += 1;
                crate::debug_detection!(
                    "  Structure too small: {}x{} at ({},{})",
                    star_bounds.2,
                    star_bounds.3,
                    star_bounds.0,
                    star_bounds.1
                );
                // Still need to clear to avoid re-processing
                for sy in star_bounds.1..(star_bounds.1 + star_bounds.3).min(height) {
                    for sx in star_bounds.0..(star_bounds.0 + star_bounds.2).min(width) {
                        structure_map[sy * width + sx] = false;
                    }
                }
                continue;
            }

            if star_bounds.2 > params.max_star_size || star_bounds.3 > params.max_star_size {
                too_large += 1;
                crate::debug_detection!(
                    "  Structure too large: {}x{} at ({},{})",
                    star_bounds.2,
                    star_bounds.3,
                    star_bounds.0,
                    star_bounds.1
                );
                // Still need to clear to avoid re-processing
                for sy in star_bounds.1..(star_bounds.1 + star_bounds.3).min(height) {
                    for sx in star_bounds.0..(star_bounds.0 + star_bounds.2).min(width) {
                        structure_map[sy * width + sx] = false;
                    }
                }
                continue;
            }

            // Clear pixels now that we know it's a valid size
            for sy in star_bounds.1..(star_bounds.1 + star_bounds.3).min(height) {
                for sx in star_bounds.0..(star_bounds.0 + star_bounds.2).min(width) {
                    structure_map[sy * width + sx] = false;
                }
            }

            // Calculate centroid
            let center_x =
                star_pixels.iter().map(|&(x, _)| x as f64).sum::<f64>() / star_pixels.len() as f64;
            let center_y =
                star_pixels.iter().map(|&(_, y)| y as f64).sum::<f64>() / star_pixels.len() as f64;

            candidates.push(StarCandidate {
                pixels: star_pixels,
                center: (center_x, center_y),
                bounding_box: star_bounds,
            });
        }
    }

    crate::debug_detection!(
        "Debug star scanning: total_structures={}, too_small={}, too_large={}, candidates={}",
        total_structures,
        too_small,
        too_large,
        candidates.len()
    );

    candidates
}

#[derive(Debug, Clone)]
struct StarCandidate {
    pixels: Vec<(usize, usize)>,
    center: (f64, f64),
    bounding_box: (usize, usize, usize, usize), // x, y, width, height
}

/// Measure and validate star candidates
fn measure_stars(
    data: &[u16],
    width: usize,
    height: usize,
    candidates: Vec<StarCandidate>,
    params: &HocusFocusParams,
    noise_estimate: &KappaSigmaResult,
) -> Vec<HocusFocusStar> {
    let mut stars = Vec::new();

    for candidate in candidates {
        // Measure star properties
        let (hfr, fwhm, peak, median, background, flux) = measure_star_properties(
            data,
            width,
            height,
            &candidate,
            params.background_box_expansion,
        );

        // Calculate SNR (signal - background) / noise
        let signal = peak - background;
        let snr = signal / noise_estimate.sigma.max(0.001);

        // Validate star based on multiple criteria
        if !validate_star(
            &candidate, peak, median, background, hfr, snr, params, width, height,
        ) {
            continue;
        }

        // PSF fitting if requested
        let psf_model = if params.psf_type != PSFType::None {
            use crate::psf_fitting::PSFFitter;
            let fitter = PSFFitter::new(params.psf_type);
            fitter.fit_star(
                data,
                width,
                height,
                candidate.center.0,
                candidate.center.1,
                candidate.bounding_box.2 as f64,
                candidate.bounding_box.3 as f64,
                background,
                peak,
            )
        } else {
            None
        };

        // Use PSF-derived FWHM if available
        let final_fwhm = if let Some(ref psf) = psf_model {
            psf.fwhm
        } else {
            fwhm
        };

        stars.push(HocusFocusStar {
            position: candidate.center,
            hfr,
            fwhm: final_fwhm,
            brightness: peak,
            background,
            snr,
            flux,
            pixel_count: candidate.pixels.len(),
            psf_model,
        });
    }

    stars
}

/// Measure star properties including median for flatness check
fn measure_star_properties(
    data: &[u16],
    width: usize,
    height: usize,
    candidate: &StarCandidate,
    background_expansion: usize,
) -> (f64, f64, f64, f64, f64, f64) {
    let (cx, cy) = candidate.center;
    let (bx, by, bw, bh) = candidate.bounding_box;

    // Calculate background from expanded region
    let expanded_width = bw + background_expansion * 2;
    let expanded_height = bh + background_expansion * 2;
    let expanded_x = bx.saturating_sub(background_expansion);
    let expanded_y = by.saturating_sub(background_expansion);

    let mut background_pixels = Vec::new();
    let mut star_pixel_values = Vec::new();

    // Collect background pixels (outside star box but inside expanded box)
    for y in expanded_y..(expanded_y + expanded_height).min(height) {
        for x in expanded_x..(expanded_x + expanded_width).min(width) {
            // Check if outside star bounding box
            if x < bx || x >= bx + bw || y < by || y >= by + bh {
                background_pixels.push(data[y * width + x] as f64);
            }
        }
    }

    // Calculate background median
    background_pixels.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let background = if !background_pixels.is_empty() {
        if background_pixels.len().is_multiple_of(2) {
            (background_pixels[background_pixels.len() / 2 - 1]
                + background_pixels[background_pixels.len() / 2])
                / 2.0
        } else {
            background_pixels[background_pixels.len() / 2]
        }
    } else {
        0.0
    };

    // Calculate star properties
    let mut weighted_distance = 0.0;
    let mut total_weight = 0.0;
    let mut peak = 0.0f64;
    let mut flux = 0.0;

    for &(px, py) in &candidate.pixels {
        let raw_value = data[py * width + px] as f64;
        let value = (raw_value - background).max(0.0);

        star_pixel_values.push(raw_value);

        if value > 0.0 {
            let distance = ((px as f64 - cx).powi(2) + (py as f64 - cy).powi(2)).sqrt();
            weighted_distance += value * distance;
            total_weight += value;
            peak = peak.max(raw_value);
            flux += value;
        }
    }

    // Calculate star median for flatness check
    star_pixel_values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let star_median = if !star_pixel_values.is_empty() {
        if star_pixel_values.len().is_multiple_of(2) {
            (star_pixel_values[star_pixel_values.len() / 2 - 1]
                + star_pixel_values[star_pixel_values.len() / 2])
                / 2.0
        } else {
            star_pixel_values[star_pixel_values.len() / 2]
        }
    } else {
        0.0
    };

    let hfr = if total_weight > 0.0 {
        weighted_distance / total_weight
    } else {
        0.0
    };

    // Estimate FWHM from HFR (approximate conversion)
    let fwhm = hfr * 2.0 * 1.177; // 2*sqrt(2*ln(2))

    (hfr, fwhm, peak, star_median - background, background, flux)
}

/// Validate star based on HocusFocus criteria
#[allow(clippy::too_many_arguments)]
fn validate_star(
    candidate: &StarCandidate,
    peak: f64,
    median: f64,
    background: f64,
    hfr: f64,
    snr: f64,
    params: &HocusFocusParams,
    src_width: usize,
    src_height: usize,
) -> bool {
    let (bx, by, bw, bh) = candidate.bounding_box;

    // Too small
    if bw < params.min_star_size || bh < params.min_star_size {
        return false;
    }

    // Touching the border
    if bx == 0 || by == 0 || bx + bw >= src_width || by + bh >= src_height {
        return false;
    }

    // Too distorted (pixel density check)
    let max_dim = bw.max(bh) as f64;
    let pixel_density = candidate.pixels.len() as f64 / (max_dim * max_dim);
    if pixel_density < params.max_distortion {
        return false;
    }

    // Fully saturated
    if (background + peak) >= params.saturation_threshold {
        return false;
    }

    // Not bright enough relative to noise (sensitivity check)
    if snr <= params.sensitivity {
        return false;
    }

    // Star center too far from bounding box center
    let box_center_x = bx as f64 + bw as f64 / 2.0;
    let box_center_y = by as f64 + bh as f64 / 2.0;
    let center_threshold_x = bw as f64 * params.star_center_tolerance / 2.0;
    let center_threshold_y = bh as f64 * params.star_center_tolerance / 2.0;

    if (candidate.center.0 - box_center_x).abs() > center_threshold_x
        || (candidate.center.1 - box_center_y).abs() > center_threshold_y
    {
        return false;
    }

    // Too flat (median too close to peak)
    if median >= params.peak_response * peak {
        return false;
    }

    // HFR below minimum threshold
    if hfr <= params.min_hfr {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg_u16(len: usize, mut state: u64) -> Vec<u16> {
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (state >> 48) as u16
            })
            .collect()
    }

    /// The retired per-pixel implementation, kept as the reference the
    /// vectorized network must match exactly.
    fn hotpixel_reference(
        data: &[u16],
        width: usize,
        height: usize,
        threshold_percent: f64,
    ) -> Vec<u16> {
        let mut result = data.to_vec();
        let threshold = threshold_percent * 65535.0;
        for y in 1..(height - 1) {
            for x in 1..(width - 1) {
                let idx = y * width + x;
                let center = data[idx] as f64;
                let mut neighbors = Vec::with_capacity(9);
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let ny = (y as i32 + dy) as usize;
                        let nx = (x as i32 + dx) as usize;
                        neighbors.push(data[ny * width + nx] as f64);
                    }
                }
                neighbors.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let median = neighbors[4];
                if (center - median).abs() > threshold {
                    result[idx] = median as u16;
                }
            }
        }
        result
    }

    #[test]
    fn hotpixel_network_matches_per_pixel_sort() {
        for (w, h, seed) in [(17usize, 11usize, 3u64), (32, 8, 99), (5, 5, 7)] {
            let data = lcg_u16(w * h, seed);
            for threshold in [0.0, 0.001, 0.1, 0.5] {
                assert_eq!(
                    apply_hotpixel_filter(&data, w, h, threshold),
                    hotpixel_reference(&data, w, h, threshold),
                    "w={w} h={h} threshold={threshold}"
                );
            }
        }
    }

    /// The retired per-pixel convolution, kept as the reference the
    /// axis-swapped accumulation must match bit for bit.
    fn blur_reference(data: &[u16], width: usize, height: usize, kernel_size: usize) -> Vec<u16> {
        let radius = kernel_size / 2;
        let mut kernel = vec![0.0; kernel_size * kernel_size];
        let sigma = radius as f64 / 2.0;
        let two_sigma_sq = 2.0 * sigma * sigma;
        let mut sum = 0.0;
        for y in 0..kernel_size {
            for x in 0..kernel_size {
                let dx = x as f64 - radius as f64;
                let dy = y as f64 - radius as f64;
                let value = (-((dx * dx + dy * dy) / two_sigma_sq)).exp();
                kernel[y * kernel_size + x] = value;
                sum += value;
            }
        }
        for k in kernel.iter_mut() {
            *k /= sum;
        }
        let mut result = vec![0u16; width * height];
        for y in radius..(height - radius) {
            for x in radius..(width - radius) {
                let mut sum = 0.0;
                for ky in 0..kernel_size {
                    for kx in 0..kernel_size {
                        let sy = y + ky - radius;
                        let sx = x + kx - radius;
                        sum += data[sy * width + sx] as f64 * kernel[ky * kernel_size + kx];
                    }
                }
                result[y * width + x] = sum as u16;
            }
        }
        result
    }

    #[test]
    fn axis_swapped_blur_is_bit_identical() {
        for (w, h, k, seed) in [
            (24usize, 13usize, 9usize, 5u64),
            (16, 16, 3, 11),
            (31, 9, 5, 42),
        ] {
            let data = lcg_u16(w * h, seed);
            assert_eq!(
                apply_gaussian_blur(&data, w, h, k),
                blur_reference(&data, w, h, k),
                "w={w} h={h} k={k}"
            );
        }
    }

    #[test]
    fn median_selection_matches_full_sort() {
        let mut state = 12345u64;
        for len in [1usize, 2, 3, 100, 101, 10_000] {
            let data: Vec<f64> = (0..len)
                .map(|_| {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    ((state >> 40) as f64) / 256.0
                })
                .collect();
            let mut sorted = data.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let expected = if len.is_multiple_of(2) {
                (sorted[len / 2 - 1] + sorted[len / 2]) / 2.0
            } else {
                sorted[len / 2]
            };
            assert_eq!(calculate_median(&data), expected, "len={len}");
        }
    }
}
