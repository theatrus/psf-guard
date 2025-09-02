use anyhow::Result;
use bumpalo::Bump;
use std::path::Path;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ImageStatistics {
    pub width: usize,
    pub height: usize,
    pub mean: f64,
    pub median: f64,
    pub std_dev: f64,
    pub min: f64,
    pub max: f64,
    pub star_count: Option<usize>,
    pub hfr: Option<f64>,
    pub fwhm: Option<f64>,
    pub mad: Option<f64>,
}

/// FITS image data structure
pub struct FitsImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u16>, // Keep as 16-bit unsigned integers
}

impl FitsImage {
    /// Extract temperature from FITS headers
    pub fn extract_temperature(path: &Path) -> Option<f64> {
        use fitrs::Fits;

        if let Ok(fits) = Fits::open(path) {
            if let Some(hdu) = fits.get(0) {
                // Try common temperature header keywords
                let temp_keywords = [
                    "CCD-TEMP", "TEMP", "SET-TEMP", "CCD_TEMP", "TEMPERAT", "CCDTEMP",
                ];

                // Compile regex once outside the loop
                let number_regex =
                    regex::Regex::new(r"(?:FloatingPoint|Integer|RealFloatingNumber)\(([^)]+)\)")
                        .unwrap();

                for keyword in &temp_keywords {
                    if let Some(value) = hdu.value(keyword) {
                        // Parse the value as a number
                        let value_str = format!("{:?}", value);
                        // Extract numeric value from debug string like "RealFloatingNumber(-10.1)" or "Integer(-10)"
                        if let Some(captures) = number_regex.captures(&value_str) {
                            if let Ok(temp) = captures[1].parse::<f64>() {
                                return Some(temp);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Extract camera model from FITS headers
    pub fn extract_camera_model(path: &Path) -> Option<String> {
        use fitrs::Fits;

        if let Ok(fits) = Fits::open(path) {
            if let Some(hdu) = fits.get(0) {
                // Try common camera/instrument header keywords
                let camera_keywords = ["INSTRUME", "CAMERA", "DETECTOR", "CCD_NAME", "CCDNAME"];

                // Compile regex once outside the loop - handle both CharacterString and String formats
                let string_regex =
                    regex::Regex::new(r#"(?:CharacterString|String)\("([^"]+)"\)"#).unwrap();

                for keyword in &camera_keywords {
                    if let Some(value) = hdu.value(keyword) {
                        // Extract string value from debug format
                        let value_str = format!("{:?}", value);
                        // Extract string from debug format like "CharacterString(\"ZWO ASI294MC Pro\")"
                        if let Some(captures) = string_regex.captures(&value_str) {
                            return Some(captures[1].to_string());
                        }
                    }
                }
            }
        }
        None
    }

    /// Load FITS image data from file using fitrs
    pub fn from_file(path: &Path) -> Result<Self> {
        use fitrs::Fits;

        let fits = Fits::open(path)
            .map_err(|e| anyhow::anyhow!("Failed to open FITS file {}: {}", path.display(), e))?;

        // Get the primary HDU
        let hdu = fits
            .get(0)
            .ok_or_else(|| anyhow::anyhow!("No HDU found in FITS file"))?;

        // Read the image data using pattern matching
        let (data_f64, width, height) = match hdu.read_data() {
            fitrs::FitsData::FloatingPoint32(array) => {
                let shape = &array.shape;
                if shape.len() >= 2 {
                    let width = shape[0];
                    let height = shape[1];
                    let data: Vec<f64> = array.data.into_iter().map(|x| x as f64).collect();
                    (data, width, height)
                } else {
                    return Err(anyhow::anyhow!("FITS file does not contain 2D image data"));
                }
            }
            fitrs::FitsData::FloatingPoint64(array) => {
                let shape = &array.shape;
                if shape.len() >= 2 {
                    let width = shape[0];
                    let height = shape[1];
                    (array.data, width, height)
                } else {
                    return Err(anyhow::anyhow!("FITS file does not contain 2D image data"));
                }
            }
            fitrs::FitsData::IntegersI32(array) => {
                let shape = &array.shape;
                if shape.len() >= 2 {
                    let width = shape[0];
                    let height = shape[1];
                    let data: Vec<f64> = array
                        .data
                        .into_iter()
                        .map(|opt| opt.unwrap_or(0) as f64)
                        .collect();
                    (data, width, height)
                } else {
                    return Err(anyhow::anyhow!("FITS file does not contain 2D image data"));
                }
            }
            fitrs::FitsData::IntegersU32(array) => {
                let shape = &array.shape;
                if shape.len() >= 2 {
                    let width = shape[0];
                    let height = shape[1];
                    let data: Vec<f64> = array
                        .data
                        .into_iter()
                        .map(|opt| opt.unwrap_or(0) as f64)
                        .collect();
                    (data, width, height)
                } else {
                    return Err(anyhow::anyhow!("FITS file does not contain 2D image data"));
                }
            }
            _ => {
                return Err(anyhow::anyhow!("Unsupported FITS data type"));
            }
        };

        // Get total pixels
        let total_pixels = data_f64.len();
        if total_pixels == 0 {
            return Err(anyhow::anyhow!("FITS file contains no image data"));
        }

        // Verify dimensions match data length
        if width * height != total_pixels {
            return Err(anyhow::anyhow!(
                "Image dimensions {}x{} don't match data length {}",
                width,
                height,
                total_pixels
            ));
        }

        // Convert f64 data to u16, scaling to 0-65535 range
        let min = data_f64.iter().fold(f64::INFINITY, |a, &b| a.min(b));
        let max = data_f64.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));

        let data_u16 = if max > min {
            let scale = 65535.0 / (max - min);
            data_f64
                .into_iter()
                .map(|v| ((v - min) * scale).clamp(0.0, 65535.0) as u16)
                .collect()
        } else {
            vec![0u16; total_pixels]
        };

        Ok(FitsImage {
            width,
            height,
            data: data_u16,
        })
    }

    /// Calculate basic statistics without star detection  
    pub fn calculate_basic_statistics(&self) -> ImageStatistics {
        self.calculate_statistics_with_mad()
    }

    /// Calculate statistics including MAD
    pub fn calculate_statistics_with_mad(&self) -> ImageStatistics {
        // Use arena for temporary allocation
        let arena = Bump::new();
        let mut sorted_data = bumpalo::vec![in &arena];
        sorted_data.extend_from_slice(&self.data);
        sorted_data.sort();

        let sum: u64 = self.data.iter().map(|&x| x as u64).sum();
        let mean = sum as f64 / self.data.len() as f64;

        let median = if self.data.len() % 2 == 0 {
            let mid = self.data.len() / 2;
            (sorted_data[mid - 1] as f64 + sorted_data[mid] as f64) / 2.0
        } else {
            sorted_data[self.data.len() / 2] as f64
        };

        let variance: f64 = self
            .data
            .iter()
            .map(|&x| {
                let diff = x as f64 - mean;
                diff * diff
            })
            .sum::<f64>()
            / self.data.len() as f64;
        let std_dev = variance.sqrt();

        let min = *sorted_data.first().unwrap_or(&0) as f64;
        let max = *sorted_data.last().unwrap_or(&65535) as f64;

        // Calculate MAD using N.I.N.A.'s histogram-based approach
        let mad = self.calculate_mad_histogram(median);

        ImageStatistics {
            width: self.width,
            height: self.height,
            mean,
            median,
            std_dev,
            min,
            max,
            star_count: None,
            hfr: None,
            fwhm: None,
            mad: Some(mad),
        }
    }

    /// Calculate basic image statistics
    pub fn calculate_statistics(&self) -> ImageStatistics {
        let stats = self.calculate_basic_statistics();

        // Return statistics without star detection
        // (star detection is now handled by dedicated modules)
        ImageStatistics {
            width: self.width,
            height: self.height,
            mean: stats.mean,
            median: stats.median,
            std_dev: stats.std_dev,
            min: stats.min,
            max: stats.max,
            star_count: None,
            hfr: None,
            fwhm: None,
            mad: stats.mad,
        }
    }

    /// Calculate MAD using N.I.N.A.'s histogram-based approach
    fn calculate_mad_histogram(&self, median: f64) -> f64 {
        // Build histogram of pixel values
        let mut pixel_counts = vec![0u32; 65536];
        for &val in self.data.iter() {
            pixel_counts[val as usize] += 1;
        }

        // Find median values (handling even vs odd length arrays)
        let median1 = median.floor() as i32;
        let median2 = median.ceil() as i32;

        // Calculate MAD using N.I.N.A.'s algorithm
        // MAD = median(|x_i - median|)
        // Since we're looking for the median of absolute deviations,
        // we start from the median and step outward symmetrically
        let mut occurrences = 0u32;
        let medianlength = self.data.len() as f64 / 2.0;
        let mut idx_down = median1;
        let mut idx_up = median2;

        loop {
            // Count pixels at current deviation distance
            if idx_down >= 0 && idx_down != idx_up {
                occurrences += pixel_counts[idx_down as usize] + pixel_counts[idx_up as usize];
            } else if idx_up < 65536 {
                occurrences += pixel_counts[idx_up as usize];
            }

            // Check if we've found the median of deviations
            if occurrences as f64 > medianlength {
                // The median absolute deviation is the current distance from median
                return (idx_up as f64 - median).abs();
            }

            // Step outward
            idx_down -= 1;
            idx_up += 1;

            // Safety check
            if idx_down < 0 && idx_up >= 65536 {
                break;
            }
        }

        // Fallback to simple MAD calculation
        let mut deviations: Vec<f64> = self
            .data
            .iter()
            .map(|&x| (x as f64 - median).abs())
            .collect();
        deviations.sort_by(|a, b| a.partial_cmp(b).unwrap());

        if deviations.len() % 2 == 0 {
            let mid = deviations.len() / 2;
            (deviations[mid - 1] + deviations[mid]) / 2.0
        } else {
            deviations[deviations.len() / 2]
        }
    }
}
