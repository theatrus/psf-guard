/// HocusFocus star detection algorithm
/// Based on the HocusFocus plugin for N.I.N.A. by George Hilios
/// Original: https://github.com/ghilios/joko.nina.plugins
///
/// This implementation uses more sophisticated detection than standard NINA:
/// - Wavelet decomposition to remove large structures (nebulae)
/// - Kappa-Sigma noise estimation for adaptive thresholding
/// - Hot pixel filtering
/// - Multi-criteria star validation
use crate::opencv_morphology::OpenCVMorphology;
use crate::opencv_wavelets::WaveletStructureRemover;
use crate::psf_fitting::{PSFModel, PSFType};

/// Star detection parameters for HocusFocus algorithm
#[derive(Debug, Clone)]
pub struct HocusFocusParams {
    // Preprocessing
    pub hotpixel_filtering: bool,
    pub hotpixel_threshold: f64, // Percent of max ADU for hot pixel threshold
    pub noise_reduction_radius: usize, // Half-size of Gaussian kernel

    // Note: OpenCV operations are always attempted first with automatic fallback

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

            // OpenCV operations always attempted with automatic fallback
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
    eprintln!(
        "Debug HocusFocus: noise_sigma: {:.3}, background_mean: {:.3}",
        noise_estimate.sigma, noise_estimate.background_mean
    );

    // Step 5: Binarize structure map using noise threshold
    let median = calculate_median(&structure_map);
    let threshold = median + params.noise_clipping_multiplier * noise_estimate.sigma;

    eprintln!(
        "Debug HocusFocus: median: {:.3}, threshold: {:.3}",
        median, threshold
    );
    let mut binary_map = binarize(&structure_map, threshold);

    // Debug: Count non-zero pixels in binary map
    let non_zero = binary_map.iter().filter(|&&x| x).count();
    eprintln!(
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
        let eroded_count = binary_map.iter().filter(|&&x| x).count();
        eprintln!(
            "Debug HocusFocus: After erosion: {} non-zero pixels ({:.2}%)",
            eroded_count,
            eroded_count as f64 / binary_map.len() as f64 * 100.0
        );
    }

    // Step 6: Find star candidates
    let candidates = find_star_candidates(&binary_map, width, height, params);
    eprintln!(
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
    eprintln!("Debug HocusFocus: {} stars passed validation", stars.len());

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

    for y in 1..(height - 1) {
        for x in 1..(width - 1) {
            let idx = y * width + x;
            let center = data[idx] as f64;

            // Get 3x3 neighborhood including center for median
            let mut neighbors = Vec::with_capacity(9);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let ny = (y as i32 + dy) as usize;
                    let nx = (x as i32 + dx) as usize;
                    neighbors.push(data[ny * width + nx] as f64);
                }
            }

            // Calculate median of 3x3 region
            neighbors.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = neighbors[4]; // Middle of 9 elements

            // Apply thresholding if enabled
            if (center - median).abs() > threshold {
                result[idx] = median as u16;
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

    // Apply convolution
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

/// Create structure map by subtracting wavelet residual layer
fn create_structure_map(
    data: &[u16],
    width: usize,
    height: usize,
    params: &HocusFocusParams,
) -> Result<Vec<f64>, Box<dyn std::error::Error>> {
    let float_data: Vec<f64> = data.iter().map(|&v| v as f64).collect();

    // Compute wavelet decomposition using OpenCV enhanced version
    let wavelet_remover = WaveletStructureRemover::new(params.structure_layers);
    let residual = wavelet_remover
        .remove_structures(&float_data, width, height)
        .map_err(|e| format!("OpenCV wavelet removal failed: {}", e))?;

    // Subtract residual from original to remove large structures
    let mut structure_map = float_data.clone();
    for i in 0..structure_map.len() {
        structure_map[i] = (structure_map[i] - residual[i]).max(0.0);
    }

    // Debug: Check structure map statistics
    let min = structure_map.iter().fold(f64::INFINITY, |a, &b| a.min(b));
    let max = structure_map
        .iter()
        .fold(f64::NEG_INFINITY, |a, &b| a.max(b));
    let non_zero = structure_map.iter().filter(|&&v| v > 0.0).count();

    // Check how many pixels are above various thresholds
    let above_10 = structure_map.iter().filter(|&&v| v > 10.0).count();
    let above_50 = structure_map.iter().filter(|&&v| v > 50.0).count();
    let above_100 = structure_map.iter().filter(|&&v| v > 100.0).count();

    eprintln!(
        "Debug structure_map: min={:.1}, max={:.1}, non_zero={} ({:.1}%)",
        min,
        max,
        non_zero,
        non_zero as f64 / structure_map.len() as f64 * 100.0
    );
    eprintln!(
        "  Above 10: {} ({:.1}%), Above 50: {} ({:.1}%), Above 100: {} ({:.1}%)",
        above_10,
        above_10 as f64 / structure_map.len() as f64 * 100.0,
        above_50,
        above_50 as f64 / structure_map.len() as f64 * 100.0,
        above_100,
        above_100 as f64 / structure_map.len() as f64 * 100.0
    );

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

    // Apply convolution
    let original = data.to_vec();
    for y in radius..(height - radius) {
        for x in radius..(width - radius) {
            let mut sum = 0.0;
            for ky in 0..kernel_size {
                for kx in 0..kernel_size {
                    let sy = y + ky - radius;
                    let sx = x + kx - radius;
                    sum += original[sy * width + sx] * kernel[ky * kernel_size + kx];
                }
            }
            data[y * width + x] = sum;
        }
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

    // Work with a copy of the data
    let data_vec: Vec<f64> = data.to_vec();

    while num_iterations < max_iterations {
        // Create mask for values below threshold
        let mask: Vec<f64> = if num_iterations > 0 {
            data_vec
                .iter()
                .filter(|&&x| x > f64::EPSILON && x < threshold - f64::EPSILON)
                .copied()
                .collect()
        } else {
            data_vec.clone()
        };

        if mask.is_empty() {
            break;
        }

        // Calculate mean and standard deviation
        let mean = mask.iter().sum::<f64>() / mask.len() as f64;
        let variance = mask.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / mask.len() as f64;
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
    let mut sorted: Vec<f64> = data.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let len = sorted.len();
    if len.is_multiple_of(2) {
        (sorted[len / 2 - 1] + sorted[len / 2]) / 2.0
    } else {
        sorted[len / 2]
    }
}

/// Binarize data using threshold
fn binarize(data: &[f64], threshold: f64) -> Vec<bool> {
    data.iter().map(|&v| v > threshold).collect()
}

/// Convert bool vector to u8 vector for OpenCV processing
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
    // Try OpenCV erosion first
    let mut u8_data = bool_to_u8(binary_map);
    let morphology = OpenCVMorphology::new_ellipse(3); // Ellipse is better for breaking up components

    morphology
        .erode_in_place(&mut u8_data, width, height)
        .map_err(|e| format!("OpenCV erosion failed: {}", e))?;

    Ok(u8_to_bool(&u8_data))
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
                eprintln!(
                    "  Structure too small: {}x{} at ({},{})",
                    star_bounds.2, star_bounds.3, star_bounds.0, star_bounds.1
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
                eprintln!(
                    "  Structure too large: {}x{} at ({},{})",
                    star_bounds.2, star_bounds.3, star_bounds.0, star_bounds.1
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

    eprintln!(
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
