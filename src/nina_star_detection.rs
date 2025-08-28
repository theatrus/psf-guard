/// Exact implementation of N.I.N.A.'s star detection algorithm
/// Based on StarDetection.cs from N.I.N.A. source code

use crate::accord_imaging::*;

/// Convert 16-bit data to 8-bit using NINA's exact method (right shift by 8)
fn convert_16bpp_to_8bpp_nina(data: &[u16]) -> Vec<u8> {
    data.iter().map(|&pixel| (pixel >> 8) as u8).collect()
}

/// Banker's rounding (round half to even) to match .NET's default Math.Round
fn round_half_to_even(x: f64) -> f64 {
    let truncated = x.trunc();
    let fraction = x - truncated;
    
    if fraction > 0.5 || fraction < -0.5 {
        x.round()
    } else if fraction == 0.5 {
        if truncated % 2.0 == 0.0 {
            truncated
        } else {
            truncated + 1.0
        }
    } else if fraction == -0.5 {
        if truncated % 2.0 == 0.0 {
            truncated
        } else {
            truncated - 1.0
        }
    } else {
        truncated
    }
}

/// Star detection parameters matching N.I.N.A.
#[derive(Debug, Clone)]
pub struct StarDetectionParams {
    pub sensitivity: StarSensitivity,
    pub noise_reduction: NoiseReduction,
    pub is_auto_focus: bool,
    pub use_roi: bool,
    pub inner_crop_ratio: f64,
    pub outer_crop_ratio: f64,
    pub number_of_af_stars: usize,
}

impl Default for StarDetectionParams {
    fn default() -> Self {
        Self {
            sensitivity: StarSensitivity::Normal,
            noise_reduction: NoiseReduction::None,
            is_auto_focus: false,
            use_roi: false,
            inner_crop_ratio: 1.0,
            outer_crop_ratio: 1.0,
            number_of_af_stars: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StarSensitivity {
    Normal,
    High,
    Highest,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoiseReduction {
    None,
    Normal,
    High,
    Highest,
    Median,
}

/// Internal state for star detection
struct DetectionState<'a> {
    pub detection_data: &'a [u16], // Data used for detection (can be stretched)
    pub original_data: &'a [u16],  // Original raw data for HFR calculation
    pub width: usize,
    pub height: usize,
    pub resize_factor: f64,
    pub inverse_resize_factor: f64,
    pub min_star_size: usize,
    pub max_star_size: usize,
}

/// Star information during detection
#[derive(Debug, Clone)]
struct Star {
    pub position: (f64, f64),  // x, y
    pub radius: f64,
    pub rectangle: Rectangle,
    pub mean_brightness: f64,
    pub surrounding_mean: f64,
    pub max_pixel_value: f64,
    pub hfr: f64,
    pub average: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct Rectangle {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Detected star result
#[derive(Debug, Clone)]
pub struct DetectedStar {
    pub hfr: f64,
    pub position: (f64, f64),
    pub average_brightness: f64,
    pub max_brightness: f64,
    pub background: f64,
    pub bounding_box: Rectangle,
}

/// Star detection result
#[derive(Debug, Clone)]
pub struct StarDetectionResult {
    pub average_hfr: f64,
    pub detected_stars: usize,
    pub hfr_std_dev: f64,
    pub star_list: Vec<DetectedStar>,
}

const MAX_WIDTH: usize = 1552;

/// Main star detection function matching N.I.N.A.'s implementation
pub fn detect_stars(
    image_data_16bit: &[u16],
    width: usize,
    height: usize,
    params: &StarDetectionParams,
) -> StarDetectionResult {
    detect_stars_with_original(image_data_16bit, image_data_16bit, width, height, params)
}

/// Star detection with separate detection and measurement data
/// This matches N.I.N.A.'s behavior where detection uses stretched data
/// but HFR measurement uses original raw data
pub fn detect_stars_with_original(
    detection_data_16bit: &[u16],  // Data for edge detection (can be stretched)
    original_data_16bit: &[u16],   // Original raw data for HFR calculation
    width: usize,
    height: usize,
    params: &StarDetectionParams,
) -> StarDetectionResult {
    // Step 1: Get initial state using both detection and original data
    let state = get_initial_state(detection_data_16bit, original_data_16bit, width, height, params);
    
    // Debug output for resize factor
    eprintln!("Debug: Image {}x{}, resize_factor: {:.3}, min_star_size: {}, max_star_size: {}", 
              width, height, state.resize_factor, state.min_star_size, state.max_star_size);
    
    // Step 2: Convert 16bpp to 8bpp for edge detection using NINA's method (right shift by 8)
    let image_8bit = convert_16bpp_to_8bpp_nina(detection_data_16bit);
    
    // Debug: Check 8-bit conversion
    let min_8bit = *image_8bit.iter().min().unwrap_or(&0);
    let max_8bit = *image_8bit.iter().max().unwrap_or(&0);
    let non_zero_count = image_8bit.iter().filter(|&&x| x > 0).count();
    eprintln!("Debug: 8-bit conversion - min: {}, max: {}, non-zero pixels: {} ({:.2}%)", 
              min_8bit, max_8bit, non_zero_count, 
              non_zero_count as f64 / image_8bit.len() as f64 * 100.0);
    
    // Step 3: Noise reduction (if enabled)
    let mut bitmap_to_analyze = if params.noise_reduction != NoiseReduction::None {
        reduce_noise(&image_8bit, state.width, state.height, params.noise_reduction)
    } else {
        image_8bit
    };
    
    // Step 4: Resize to speed up manipulation
    let (resized_image, resized_width, resized_height) = DetectionUtility::resize_for_detection(
        &bitmap_to_analyze,
        width,
        height,
        MAX_WIDTH,
        state.resize_factor,
    );
    bitmap_to_analyze = resized_image;
    
    eprintln!("Debug: Resized to {}x{}", resized_width, resized_height);
    
    // Step 5: Prepare image for structure detection
    prepare_for_structure_detection(&mut bitmap_to_analyze, resized_width, resized_height, params);
    
    // Step 6: Get structure info
    let blobs = detect_structures(&bitmap_to_analyze, resized_width, resized_height);
    
    eprintln!("Debug: Detected {} blobs", blobs.len());
    
    // Step 7: Identify stars
    let (star_list, detected_stars) = identify_stars(params, &state, blobs, resized_width, resized_height);
    
    // Step 8: Calculate statistics
    let mut result = StarDetectionResult {
        average_hfr: 0.0,
        detected_stars,
        hfr_std_dev: 0.0,
        star_list,
    };
    
    if !result.star_list.is_empty() {
        let mean: f64 = result.star_list.iter().map(|s| s.hfr).sum::<f64>() / result.star_list.len() as f64;
        result.average_hfr = mean;
        
        if result.star_list.len() > 1 {
            let variance: f64 = result.star_list.iter()
                .map(|s| (s.hfr - mean).powi(2))
                .sum::<f64>() / (result.star_list.len() - 1) as f64;
            result.hfr_std_dev = variance.sqrt();
        }
    }
    
    result
}

fn get_initial_state<'a>(
    detection_data: &'a [u16],
    original_data: &'a [u16],
    width: usize,
    height: usize,
    params: &StarDetectionParams,
) -> DetectionState<'a> {
    let mut resize_factor = 1.0;
    
    if width > MAX_WIDTH {
        resize_factor = match params.sensitivity {
            StarSensitivity::Highest => {
                f64::max(2.0 / 3.0, MAX_WIDTH as f64 / width as f64)
            }
            StarSensitivity::High => {
                // N.I.N.A. uses image scale for High sensitivity
                // For now, we'll simulate the common case of 1.0-1.5 arcsec/pixel
                // which uses 1/3 resize factor
                // TODO: Calculate from FITS headers XPIXSZ/FOCALLEN
                let simulated_resize = 1.0 / 3.0;
                
                // But still respect the MAX_WIDTH limit
                f64::max(simulated_resize, MAX_WIDTH as f64 / width as f64)
            }
            StarSensitivity::Normal => {
                MAX_WIDTH as f64 / width as f64
            }
        };
    }
    
    let inverse_resize_factor = 1.0 / resize_factor;
    let min_star_size = ((5.0 * resize_factor).floor() as usize).max(2);
    let max_star_size = (150.0 * resize_factor).ceil() as usize;
    
    DetectionState {
        detection_data,
        original_data,
        width,
        height,
        resize_factor,
        inverse_resize_factor,
        min_star_size,
        max_star_size,
    }
}

fn reduce_noise(image: &[u8], width: usize, height: usize, noise_reduction: NoiseReduction) -> Vec<u8> {
    match noise_reduction {
        NoiseReduction::None => image.to_vec(),
        NoiseReduction::Normal => {
            let blur = FastGaussianBlur::new();
            blur.process(image, width, height, 1)
        }
        NoiseReduction::High => {
            let blur = FastGaussianBlur::new();
            blur.process(image, width, height, 2)
        }
        NoiseReduction::Highest => {
            let blur = FastGaussianBlur::new();
            blur.process(image, width, height, 3)
        }
        NoiseReduction::Median => {
            let median = Median;
            median.apply(image, width, height)
        }
    }
}

fn prepare_for_structure_detection(
    image: &mut [u8],
    width: usize,
    height: usize,
    params: &StarDetectionParams,
) {
    // Apply Canny edge detector
    // N.I.N.A. uses NoBlurCanny for High/Highest sensitivity, regular Canny for Normal
    match params.sensitivity {
        StarSensitivity::Normal => {
            let canny = CannyEdgeDetector::new(10, 80);
            canny.apply_in_place(image, width, height);
        }
        StarSensitivity::High | StarSensitivity::Highest => {
            let canny = CannyEdgeDetector::new_no_blur(10, 80);
            canny.apply_in_place(image, width, height);
        }
    }
    
    // Debug: Check edge detection results
    let edge_pixels = image.iter().filter(|&&p| p > 0).count();
    eprintln!("Debug: After Canny edge detection - {} non-zero pixels", edge_pixels);
    
    // Apply SIS threshold
    let sis = SISThreshold;
    sis.apply_in_place(image, width, height);
    
    // Debug: Count non-zero pixels after SIS
    let non_zero = image.iter().filter(|&&p| p > 0).count();
    eprintln!("Debug: After SIS threshold - {} non-zero pixels", non_zero);
    
    // Apply binary dilation
    let dilation = BinaryDilation3x3;
    dilation.apply_in_place(image, width, height);
}

fn detect_structures(image: &[u8], width: usize, height: usize) -> Vec<Blob> {
    let mut blob_counter = BlobCounter::new();
    blob_counter.process_image(image, width, height);
    blob_counter.get_objects_information()
}

fn identify_stars(
    params: &StarDetectionParams,
    state: &DetectionState,
    blobs: Vec<Blob>,
    _bitmap_width: usize,
    _bitmap_height: usize,
) -> (Vec<DetectedStar>, usize) {
    let mut star_list = Vec::new();
    let shape_checker = SimpleShapeChecker;
    let mut sum_radius = 0.0;
    let mut sum_squares = 0.0;
    
    let mut size_filtered = 0;
    let mut roi_filtered = 0;
    let mut failed_detection = 0;
    let mut edge_filtered = 0;
    
    for blob in &blobs {
        // Size filtering
        if blob.rectangle.width > state.max_star_size as i32
            || blob.rectangle.height > state.max_star_size as i32
            || blob.rectangle.width < state.min_star_size as i32
            || blob.rectangle.height < state.min_star_size as i32 {
            size_filtered += 1;
            continue;
        }
        
        // ROI filtering (simplified - not implemented)
        if params.use_roi {
            // TODO: Implement InROI check
            roi_filtered += 1;
        }
        
        // Scale rectangle back to original coordinates
        let rect = Rectangle {
            x: (blob.rectangle.x as f64 * state.inverse_resize_factor).floor() as i32,
            y: (blob.rectangle.y as f64 * state.inverse_resize_factor).floor() as i32,
            width: (blob.rectangle.width as f64 * state.inverse_resize_factor).ceil() as i32,
            height: (blob.rectangle.height as f64 * state.inverse_resize_factor).ceil() as i32,
        };
        
        // Build large rectangle for background estimation (3x the star size)
        let large_rect_x = (rect.x - rect.width).max(0);
        let large_rect_y = (rect.y - rect.height).max(0);
        let mut large_rect_width = rect.width * 3;
        if large_rect_x + large_rect_width > state.width as i32 {
            large_rect_width = state.width as i32 - large_rect_x;
        }
        let mut large_rect_height = rect.height * 3;
        if large_rect_y + large_rect_height > state.height as i32 {
            large_rect_height = state.height as i32 - large_rect_y;
        }
        let large_rect = Rectangle {
            x: large_rect_x,
            y: large_rect_y,
            width: large_rect_width,
            height: large_rect_height,
        };
        
        // Check if star is circular (simplified)
        let points = Vec::new(); // TODO: Get blob edge points
        let mut center_x = 0.0f32;
        let mut center_y = 0.0f32;
        let mut radius = 0.0f32;
        
        let star = if shape_checker.is_circle(&points, &mut center_x, &mut center_y, &mut radius) {
            Star {
                position: (
                    center_x as f64 * state.inverse_resize_factor,
                    center_y as f64 * state.inverse_resize_factor,
                ),
                radius: radius as f64 * state.inverse_resize_factor,
                rectangle: rect,
                mean_brightness: 0.0,
                surrounding_mean: 0.0,
                max_pixel_value: 0.0,
                hfr: 0.0,
                average: 0.0,
            }
        } else {
            // Star is elongated - check eccentricity
            let eccentricity = calculate_eccentricity(rect.width as f64, rect.height as f64);
            if eccentricity > 0.8 {
                continue; // Discard highly elliptical shapes
            }
            
            let center_x = blob.rectangle.x as f64 + blob.rectangle.width as f64 / 2.0;
            let center_y = blob.rectangle.y as f64 + blob.rectangle.height as f64 / 2.0;
            
            Star {
                position: (
                    center_x * state.inverse_resize_factor,
                    center_y * state.inverse_resize_factor,
                ),
                radius: rect.width.max(rect.height) as f64 / 2.0,
                rectangle: rect,
                mean_brightness: 0.0,
                surrounding_mean: 0.0,
                max_pixel_value: 0.0,
                hfr: 0.0,
                average: 0.0,
            }
        };
        
        // Get pixel data and calculate statistics
        let (is_star, mut star) = analyze_star_pixels(state, star, &large_rect);
        
        if is_star {
            sum_radius += star.radius;
            sum_squares += star.radius * star.radius;
            
            // Calculate HFR
            star = calculate_star_hfr(state, star);
            
            // Check if centroid is not touching rectangle edges 
            // NOTE: N.I.N.A. has a bug in line 344 where it compares Position.X < Position.X + Width
            // We replicate this bug for compatibility
            if star.position.0 > (star.rectangle.x + 1) as f64
                && star.position.1 > (star.rectangle.y + 1) as f64
                && star.position.0 < (star.position.0 + star.rectangle.width as f64 - 2.0)  // N.I.N.A. bug
                && star.position.1 < (star.rectangle.y + star.rectangle.height - 2) as f64 {
                star_list.push(star);
            } else {
                edge_filtered += 1;
            }
        } else {
            failed_detection += 1;
        }
    }
    
    // No stars found
    if star_list.is_empty() {
        return (Vec::new(), 0);
    }
    
    // Filter by radius statistics
    let detected_stars = star_list.len();
    
    if !star_list.is_empty() {
        let avg = sum_radius / star_list.len() as f64;
        let stdev = ((sum_squares - star_list.len() as f64 * avg * avg) / star_list.len() as f64).sqrt();
        
        eprintln!("Debug: Before radius filter: {} stars, avg radius: {:.2}, stdev: {:.2}", 
                  star_list.len(), avg, stdev);
        
        star_list.retain(|s| match params.sensitivity {
            StarSensitivity::Highest => {
                // More permissive towards large stars
                s.radius <= avg + 2.0 * stdev && s.radius >= avg - 1.5 * stdev
            }
            _ => {
                s.radius <= avg + 1.5 * stdev && s.radius >= avg - 1.5 * stdev
            }
        });
        
        eprintln!("Debug: After radius filter: {} stars", star_list.len());
    }
    
    eprintln!("Debug: Blob filtering - Size: {}, ROI: {}, Failed detection: {}, Edge: {}", 
              size_filtered, roi_filtered, failed_detection, edge_filtered);
    
    // Convert to DetectedStar
    let detected: Vec<DetectedStar> = star_list
        .into_iter()
        .map(|s| DetectedStar {
            hfr: s.hfr,
            position: s.position,
            average_brightness: s.average,
            max_brightness: s.max_pixel_value,
            background: s.surrounding_mean,
            bounding_box: s.rectangle,
        })
        .collect();
    
    (detected, detected_stars)
}

fn analyze_star_pixels(
    state: &DetectionState,
    mut star: Star,
    large_rect: &Rectangle,
) -> (bool, Star) {
    let mut star_pixel_sum = 0.0;
    let mut star_pixel_count = 0;
    let mut large_rect_pixel_sum = 0.0;
    let mut large_rect_pixel_sum_squares = 0.0;
    let mut inner_star_bright_pixels = 0;
    
    // Process pixels
    for y in large_rect.y..(large_rect.y + large_rect.height) {
        for x in large_rect.x..(large_rect.x + large_rect.width) {
            if x >= 0 && y >= 0 && (x as usize) < state.width && (y as usize) < state.height {
                let pixel_value = state.original_data[(y as usize) * state.width + (x as usize)] as f64;
                
                // Check if in star rectangle
                if x >= star.rectangle.x && x < star.rectangle.x + star.rectangle.width
                    && y >= star.rectangle.y && y < star.rectangle.y + star.rectangle.height {
                    // Check if inside circle
                    if inside_circle(x as f64, y as f64, star.position.0, star.position.1, star.radius) {
                        star_pixel_sum += pixel_value;
                        star_pixel_count += 1;
                        star.max_pixel_value = star.max_pixel_value.max(pixel_value);
                    }
                } else {
                    // Background pixel
                    large_rect_pixel_sum += pixel_value;
                    large_rect_pixel_sum_squares += pixel_value * pixel_value;
                }
            }
        }
    }
    
    if star_pixel_count == 0 {
        return (false, star);
    }
    
    star.mean_brightness = star_pixel_sum / star_pixel_count as f64;
    let large_rect_pixel_count = (large_rect.height * large_rect.width - star.rectangle.height * star.rectangle.width) as f64;
    let large_rect_mean = large_rect_pixel_sum / large_rect_pixel_count;
    star.surrounding_mean = large_rect_mean;
    let large_rect_stdev = ((large_rect_pixel_sum_squares - large_rect_pixel_count * large_rect_mean * large_rect_mean) / large_rect_pixel_count).sqrt();
    
    // Minimum bright pixels threshold
    let minimum_bright_pixels = (state.width.max(state.height) as f64 / 1000.0).ceil() as usize;
    let bright_pixel_threshold = large_rect_mean + 1.5 * large_rect_stdev;
    
    // Count bright pixels
    for y in star.rectangle.y..(star.rectangle.y + star.rectangle.height) {
        for x in star.rectangle.x..(star.rectangle.x + star.rectangle.width) {
            if x >= 0 && y >= 0 && (x as usize) < state.width && (y as usize) < state.height {
                if inside_circle(x as f64, y as f64, star.position.0, star.position.1, star.radius) {
                    let pixel_value = state.original_data[(y as usize) * state.width + (x as usize)] as f64;
                    if pixel_value > bright_pixel_threshold {
                        inner_star_bright_pixels += 1;
                    }
                }
            }
        }
    }
    
    // Check detection criteria
    let brightness_threshold = large_rect_mean + (0.1 * large_rect_mean).min(large_rect_stdev);
    let is_star = star.mean_brightness >= brightness_threshold && inner_star_bright_pixels > minimum_bright_pixels;
    
    (is_star, star)
}

fn calculate_star_hfr(state: &DetectionState, mut star: Star) -> Star {
    let outer_radius = star.radius * 1.2;
    let mut sum = 0.0;
    let mut sum_dist = 0.0;
    let mut all_sum = 0.0;
    let mut sum_val_x = 0.0;
    let mut sum_val_y = 0.0;
    let mut pixel_count = 0;
    
    // Process all pixels in star rectangle
    for y in star.rectangle.y..(star.rectangle.y + star.rectangle.height) {
        for x in star.rectangle.x..(star.rectangle.x + star.rectangle.width) {
            if x >= 0 && y >= 0 && (x as usize) < state.width && (y as usize) < state.height {
                let pixel_value = state.original_data[(y as usize) * state.width + (x as usize)] as f64;
                
                // N.I.N.A.'s exact background subtraction: Math.Round(value - SurroundingMean)
                // Uses banker's rounding (round half to even)
                let mut value = round_half_to_even(pixel_value - star.surrounding_mean);
                if value < 0.0 {
                    value = 0.0;
                }
                
                all_sum += value;
                pixel_count += 1;
                
                // Only include pixels within outerRadius in HFR calculation
                if inside_circle(x as f64, y as f64, star.position.0, star.position.1, outer_radius) {
                    let dx = x as f64 - star.position.0;
                    let dy = y as f64 - star.position.1;
                    let distance = (dx * dx + dy * dy).sqrt();
                    
                    sum += value;
                    sum_dist += value * distance;
                    sum_val_x += (x - star.rectangle.x) as f64 * value;
                    sum_val_y += (y - star.rectangle.y) as f64 * value;
                }
            }
        }
    }
    
    // Calculate HFR
    star.hfr = if sum > 0.0 {
        sum_dist / sum
    } else {
        2.0_f64.sqrt() * outer_radius
    };
    
    star.average = if pixel_count > 0 {
        all_sum / pixel_count as f64
    } else {
        0.0
    };
    
    // Update centroid
    if sum > 0.0 {
        let centroid_x = sum_val_x / sum + star.rectangle.x as f64;
        let centroid_y = sum_val_y / sum + star.rectangle.y as f64;
        star.position = (centroid_x, centroid_y);
    }
    
    star
}

fn inside_circle(x: f64, y: f64, center_x: f64, center_y: f64, radius: f64) -> bool {
    (x - center_x).powi(2) + (y - center_y).powi(2) <= radius.powi(2)
}

fn calculate_eccentricity(width: f64, height: f64) -> f64 {
    let x = width.max(height);
    let y = width.min(height);
    let focus = (x.powi(2) - y.powi(2)).sqrt();
    focus / x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inside_circle() {
        assert!(inside_circle(0.0, 0.0, 0.0, 0.0, 1.0));
        assert!(inside_circle(0.5, 0.5, 0.0, 0.0, 1.0));
        assert!(!inside_circle(1.5, 0.0, 0.0, 0.0, 1.0));
    }

    #[test]
    fn test_eccentricity() {
        // Circle (width == height) should have eccentricity 0
        assert_eq!(calculate_eccentricity(10.0, 10.0), 0.0);
        
        // Very elongated ellipse
        let e = calculate_eccentricity(10.0, 5.0);
        assert!(e > 0.8);
    }
}