use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct StatisticalGradingConfig {
    /// Enable HFR outlier detection
    pub enable_hfr_analysis: bool,
    /// Standard deviations for HFR outlier detection
    pub hfr_stddev_threshold: f64,

    /// Enable star count outlier detection
    pub enable_star_count_analysis: bool,
    /// Standard deviations for star count outlier detection
    pub star_count_stddev_threshold: f64,

    /// Enable median/mean shift detection
    pub enable_distribution_analysis: bool,
    /// Percentage threshold for median shift from mean
    pub median_shift_threshold: f64,

    /// Enable cloud detection (sudden rises in median)
    pub enable_cloud_detection: bool,
    /// Percentage threshold for cloud detection (e.g., 0.2 = 20% increase)
    pub cloud_threshold: f64,
    /// Number of images to establish baseline after cloud event
    pub cloud_baseline_count: usize,
}

impl Default for StatisticalGradingConfig {
    fn default() -> Self {
        Self {
            enable_hfr_analysis: true,
            hfr_stddev_threshold: 2.0,
            enable_star_count_analysis: true,
            star_count_stddev_threshold: 2.0,
            enable_distribution_analysis: true,
            median_shift_threshold: 0.10, // 10% shift
            enable_cloud_detection: true,
            cloud_threshold: 0.20,   // 20% increase indicates clouds
            cloud_baseline_count: 5, // Need 5 images to establish new baseline
        }
    }
}

#[derive(Debug, Deserialize)]
struct ImageMetadata {
    #[serde(rename = "FileName")]
    #[allow(dead_code)]
    filename: String,
    #[serde(rename = "FilterName")]
    #[allow(dead_code)]
    filter_name: String,
    #[serde(rename = "HFR")]
    hfr: Option<f64>,
    #[serde(rename = "DetectedStars")]
    detected_stars: Option<i32>,
    #[serde(rename = "ExposureStartTime")]
    exposure_start_time: String,
}

#[derive(Debug)]
pub struct ImageStatistics {
    pub id: i32,
    pub target_id: i32,
    pub target_name: String,
    pub filter_name: String,
    pub hfr: Option<f64>,
    pub star_count: Option<i32>,
    pub exposure_time: String,
    pub original_status: i32,
    pub metadata_json: String,
}

#[derive(Debug)]
pub struct FilterStatistics {
    pub filter_name: String,
    pub hfr_values: Vec<f64>,
    pub star_counts: Vec<i32>,
    pub hfr_mean: f64,
    pub hfr_median: f64,
    pub hfr_stddev: f64,
    pub star_count_mean: f64,
    pub star_count_median: f64,
    pub star_count_stddev: f64,
}

#[derive(Debug, Clone)]
pub struct StatisticalRejection {
    pub image_id: i32,
    pub reason: String,
    pub details: String,
}

pub struct StatisticalGrader {
    config: StatisticalGradingConfig,
}

impl StatisticalGrader {
    pub fn new(config: StatisticalGradingConfig) -> Self {
        Self { config }
    }

    /// Analyze images and return additional rejections based on statistical analysis
    pub fn analyze_images(
        &self,
        mut images: Vec<ImageStatistics>,
    ) -> Result<Vec<StatisticalRejection>> {
        let mut rejections = Vec::new();

        // Sort images by target, filter, and time to ensure proper sequence
        images.sort_by(|a, b| {
            a.target_id
                .cmp(&b.target_id)
                .then_with(|| a.filter_name.cmp(&b.filter_name))
                .then_with(|| a.exposure_time.cmp(&b.exposure_time))
        });

        // Group images by target and filter
        let mut target_filter_groups: HashMap<(i32, String), Vec<&ImageStatistics>> =
            HashMap::new();
        for image in &images {
            target_filter_groups
                .entry((image.target_id, image.filter_name.clone()))
                .or_default()
                .push(image);
        }

        // Analyze each target/filter group
        for ((_target_id, _filter_name), target_filter_images) in target_filter_groups {
            if target_filter_images.len() < 3 {
                // Not enough images for statistical analysis
                continue;
            }

            // Calculate statistics for this target/filter combination
            let stats = self.calculate_filter_statistics(&target_filter_images);

            // Check for outliers
            if self.config.enable_hfr_analysis {
                rejections.extend(self.check_hfr_outliers(&target_filter_images, &stats));
            }

            if self.config.enable_star_count_analysis {
                rejections.extend(self.check_star_count_outliers(&target_filter_images, &stats));
            }

            if self.config.enable_distribution_analysis {
                rejections.extend(self.check_distribution_quality(&target_filter_images, &stats));
            }

            // Check for cloud detection (sequence analysis)
            if self.config.enable_cloud_detection {
                rejections.extend(self.check_cloud_sequence(&target_filter_images));
            }
        }

        Ok(rejections)
    }

    fn calculate_filter_statistics(&self, images: &[&ImageStatistics]) -> FilterStatistics {
        let mut hfr_values: Vec<f64> = images.iter().filter_map(|img| img.hfr).collect();

        let mut star_counts: Vec<i32> = images.iter().filter_map(|img| img.star_count).collect();

        let hfr_mean = if !hfr_values.is_empty() {
            hfr_values.iter().sum::<f64>() / hfr_values.len() as f64
        } else {
            0.0
        };

        let star_count_mean = if !star_counts.is_empty() {
            star_counts.iter().sum::<i32>() as f64 / star_counts.len() as f64
        } else {
            0.0
        };

        // Calculate medians
        let hfr_median = self.calculate_median(&mut hfr_values);
        let star_count_median = self.calculate_median_i32(&mut star_counts);

        let hfr_stddev = self.calculate_stddev(&hfr_values, hfr_mean);
        let star_count_stddev = self.calculate_stddev_i32(&star_counts, star_count_mean);

        FilterStatistics {
            filter_name: images[0].filter_name.clone(),
            hfr_values,
            star_counts,
            hfr_mean,
            hfr_median,
            hfr_stddev,
            star_count_mean,
            star_count_median,
            star_count_stddev,
        }
    }

    fn calculate_stddev(&self, values: &[f64], mean: f64) -> f64 {
        if values.len() < 2 {
            return 0.0;
        }

        let variance =
            values.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / (values.len() - 1) as f64;

        variance.sqrt()
    }

    fn calculate_stddev_i32(&self, values: &[i32], mean: f64) -> f64 {
        if values.len() < 2 {
            return 0.0;
        }

        let variance = values
            .iter()
            .map(|&x| (x as f64 - mean).powi(2))
            .sum::<f64>()
            / (values.len() - 1) as f64;

        variance.sqrt()
    }

    fn calculate_median(&self, values: &mut [f64]) -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mid = values.len() / 2;
        if values.len().is_multiple_of(2) {
            (values[mid - 1] + values[mid]) / 2.0
        } else {
            values[mid]
        }
    }

    fn calculate_median_i32(&self, values: &mut [i32]) -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        values.sort();
        let mid = values.len() / 2;
        if values.len().is_multiple_of(2) {
            (values[mid - 1] + values[mid]) as f64 / 2.0
        } else {
            values[mid] as f64
        }
    }

    fn check_hfr_outliers(
        &self,
        images: &[&ImageStatistics],
        stats: &FilterStatistics,
    ) -> Vec<StatisticalRejection> {
        let mut rejections = Vec::new();

        if stats.hfr_stddev == 0.0 {
            return rejections;
        }

        for image in images {
            if let Some(hfr) = image.hfr {
                let z_score = (hfr - stats.hfr_mean).abs() / stats.hfr_stddev;

                if z_score > self.config.hfr_stddev_threshold {
                    rejections.push(StatisticalRejection {
                        image_id: image.id,
                        reason: "Statistical HFR".to_string(),
                        details: format!(
                            "HFR {:.3} is {:.1}σ from mean {:.3} (threshold: {:.1}σ)",
                            hfr, z_score, stats.hfr_mean, self.config.hfr_stddev_threshold
                        ),
                    });
                }
            }
        }

        rejections
    }

    fn check_star_count_outliers(
        &self,
        images: &[&ImageStatistics],
        stats: &FilterStatistics,
    ) -> Vec<StatisticalRejection> {
        let mut rejections = Vec::new();

        if stats.star_count_stddev == 0.0 {
            return rejections;
        }

        for image in images {
            if let Some(star_count) = image.star_count {
                let z_score =
                    (star_count as f64 - stats.star_count_mean).abs() / stats.star_count_stddev;

                if z_score > self.config.star_count_stddev_threshold {
                    rejections.push(StatisticalRejection {
                        image_id: image.id,
                        reason: "Statistical Stars".to_string(),
                        details: format!(
                            "Star count {} is {:.1}σ from mean {:.0} (threshold: {:.1}σ)",
                            star_count,
                            z_score,
                            stats.star_count_mean,
                            self.config.star_count_stddev_threshold
                        ),
                    });
                }
            }
        }

        rejections
    }

    fn check_distribution_quality(
        &self,
        images: &[&ImageStatistics],
        stats: &FilterStatistics,
    ) -> Vec<StatisticalRejection> {
        let mut rejections = Vec::new();

        // Check if median significantly differs from mean (indicating skewed distribution)
        if stats.hfr_stddev > 0.0 {
            let hfr_median_shift = (stats.hfr_median - stats.hfr_mean).abs() / stats.hfr_mean;

            if hfr_median_shift > self.config.median_shift_threshold {
                // The distribution is skewed, use median for outlier detection
                for image in images {
                    if let Some(hfr) = image.hfr {
                        // Use median-based rejection for skewed distributions
                        let deviation_from_median = (hfr - stats.hfr_median).abs();
                        let mad_multiplier = 1.4826; // Constant to make MAD comparable to stddev

                        // Calculate Median Absolute Deviation (MAD)
                        let mut deviations: Vec<f64> = stats
                            .hfr_values
                            .iter()
                            .map(|&v| (v - stats.hfr_median).abs())
                            .collect();
                        let mad = self.calculate_median(&mut deviations) * mad_multiplier;

                        if mad > 0.0 {
                            let z_score = deviation_from_median / mad;
                            if z_score > self.config.hfr_stddev_threshold {
                                rejections.push(StatisticalRejection {
                                    image_id: image.id,
                                    reason: "Distribution HFR".to_string(),
                                    details: format!(
                                        "HFR {:.3} deviates {:.1} MAD from median {:.3} (threshold: {:.1})",
                                        hfr, z_score, stats.hfr_median, self.config.hfr_stddev_threshold
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }

        // Similar check for star count distribution
        if stats.star_count_stddev > 0.0 {
            let star_median_shift =
                (stats.star_count_median - stats.star_count_mean).abs() / stats.star_count_mean;

            if star_median_shift > self.config.median_shift_threshold {
                // The distribution is skewed, use median for outlier detection
                for image in images {
                    if let Some(star_count) = image.star_count {
                        // Use median-based rejection for skewed distributions
                        let deviation_from_median =
                            (star_count as f64 - stats.star_count_median).abs();
                        let mad_multiplier = 1.4826; // Constant to make MAD comparable to stddev

                        // Calculate Median Absolute Deviation (MAD)
                        let mut deviations: Vec<f64> = stats
                            .star_counts
                            .iter()
                            .map(|&v| (v as f64 - stats.star_count_median).abs())
                            .collect();
                        let mad = self.calculate_median(&mut deviations) * mad_multiplier;

                        if mad > 0.0 {
                            let z_score = deviation_from_median / mad;
                            if z_score > self.config.star_count_stddev_threshold {
                                rejections.push(StatisticalRejection {
                                    image_id: image.id,
                                    reason: "Distribution Stars".to_string(),
                                    details: format!(
                                        "Star count {} deviates {:.1} MAD from median {:.0} (threshold: {:.1})",
                                        star_count, z_score, stats.star_count_median, self.config.star_count_stddev_threshold
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }

        rejections
    }

    fn check_cloud_sequence(&self, images: &[&ImageStatistics]) -> Vec<StatisticalRejection> {
        let mut rejections = Vec::new();

        if images.len() < 3 {
            return rejections;
        }

        // Track baseline establishment
        let mut baseline_established = false;
        let mut _baseline_start_idx = 0;
        let mut baseline_values: Vec<f64> = Vec::new();

        // We'll use HFR as primary indicator for clouds (higher HFR = worse seeing/clouds)
        // Calculate rolling median for each position
        for (i, image) in images.iter().enumerate() {
            if let Some(current_hfr) = image.hfr {
                // Skip if no baseline yet
                if !baseline_established {
                    baseline_values.push(current_hfr);

                    // Need enough images to establish baseline
                    if baseline_values.len() >= self.config.cloud_baseline_count {
                        baseline_established = true;
                        _baseline_start_idx = i + 1;
                    }
                    continue;
                }

                // Calculate baseline median
                let mut sorted_baseline = baseline_values.clone();
                sorted_baseline.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let baseline_median = if sorted_baseline.len().is_multiple_of(2) {
                    let mid = sorted_baseline.len() / 2;
                    (sorted_baseline[mid - 1] + sorted_baseline[mid]) / 2.0
                } else {
                    sorted_baseline[sorted_baseline.len() / 2]
                };

                // Check if current value represents a sudden rise (cloud event)
                let increase_ratio = (current_hfr - baseline_median) / baseline_median;

                if increase_ratio > self.config.cloud_threshold {
                    // Cloud detected - reject this and following images until new baseline
                    rejections.push(StatisticalRejection {
                        image_id: image.id,
                        reason: "Cloud Detection".to_string(),
                        details: format!(
                            "HFR {:.3} is {:.0}% above baseline {:.3} (threshold: {:.0}%)",
                            current_hfr,
                            increase_ratio * 100.0,
                            baseline_median,
                            self.config.cloud_threshold * 100.0
                        ),
                    });

                    // Reset baseline establishment
                    baseline_established = false;
                    baseline_values.clear();
                    baseline_values.push(current_hfr);
                } else {
                    // Update rolling baseline - remove oldest, add newest
                    if baseline_values.len() >= self.config.cloud_baseline_count {
                        baseline_values.remove(0);
                    }
                    baseline_values.push(current_hfr);
                }
            }
        }

        // Also check star count drops as secondary indicator
        if rejections.is_empty() {
            baseline_established = false;
            baseline_values.clear();

            for (i, image) in images.iter().enumerate() {
                if let Some(current_stars) = image.star_count {
                    let current_stars_f64 = current_stars as f64;

                    if !baseline_established {
                        baseline_values.push(current_stars_f64);

                        if baseline_values.len() >= self.config.cloud_baseline_count {
                            baseline_established = true;
                            _baseline_start_idx = i + 1;
                        }
                        continue;
                    }

                    // Calculate baseline median
                    let mut sorted_baseline = baseline_values.clone();
                    sorted_baseline.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let baseline_median = if sorted_baseline.len().is_multiple_of(2) {
                        let mid = sorted_baseline.len() / 2;
                        (sorted_baseline[mid - 1] + sorted_baseline[mid]) / 2.0
                    } else {
                        sorted_baseline[sorted_baseline.len() / 2]
                    };

                    // For star count, a drop indicates clouds
                    let decrease_ratio = (baseline_median - current_stars_f64) / baseline_median;

                    if decrease_ratio > self.config.cloud_threshold {
                        rejections.push(StatisticalRejection {
                            image_id: image.id,
                            reason: "Cloud Detection (Stars)".to_string(),
                            details: format!(
                                "Star count {} is {:.0}% below baseline {:.0} (threshold: {:.0}%)",
                                current_stars,
                                decrease_ratio * 100.0,
                                baseline_median,
                                self.config.cloud_threshold * 100.0
                            ),
                        });

                        // Reset baseline
                        baseline_established = false;
                        baseline_values.clear();
                        baseline_values.push(current_stars_f64);
                    } else {
                        // Update rolling baseline
                        if baseline_values.len() >= self.config.cloud_baseline_count {
                            baseline_values.remove(0);
                        }
                        baseline_values.push(current_stars_f64);
                    }
                }
            }
        }

        rejections
    }
}

/// Parse image metadata from JSON to extract HFR and star count
pub fn parse_image_metadata(
    id: i32,
    target_id: i32,
    target_name: &str,
    metadata_json: &str,
    filter_name: &str,
    original_status: i32,
) -> Result<ImageStatistics> {
    let metadata: ImageMetadata = serde_json::from_str(metadata_json)?;

    Ok(ImageStatistics {
        id,
        target_id,
        target_name: target_name.to_string(),
        filter_name: filter_name.to_string(),
        hfr: metadata.hfr,
        star_count: metadata.detected_stars,
        exposure_time: metadata.exposure_start_time,
        original_status,
        metadata_json: metadata_json.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_statistical_grading_config_default() {
        let config = StatisticalGradingConfig::default();
        assert!(config.enable_hfr_analysis);
        assert_eq!(config.hfr_stddev_threshold, 2.0);
        assert!(config.enable_star_count_analysis);
        assert_eq!(config.star_count_stddev_threshold, 2.0);
        assert!(config.enable_distribution_analysis);
        assert_eq!(config.median_shift_threshold, 0.10);
        assert!(config.enable_cloud_detection);
        assert_eq!(config.cloud_threshold, 0.20);
        assert_eq!(config.cloud_baseline_count, 5);
    }

    #[test]
    fn test_parse_image_metadata_complete() {
        let metadata_json = r#"{
            "FileName": "/path/to/image.fits",
            "FilterName": "Ha",
            "HFR": 2.5,
            "DetectedStars": 342,
            "ExposureStartTime": "2023-08-27T10:00:00Z"
        }"#;

        let result = parse_image_metadata(1, 2, "Test Target", metadata_json, "Ha", 0).unwrap();

        assert_eq!(result.id, 1);
        assert_eq!(result.target_id, 2);
        assert_eq!(result.target_name, "Test Target");
        assert_eq!(result.filter_name, "Ha");
        assert_eq!(result.hfr, Some(2.5));
        assert_eq!(result.star_count, Some(342));
        assert_eq!(result.exposure_time, "2023-08-27T10:00:00Z");
        assert_eq!(result.original_status, 0);
    }

    #[test]
    fn test_parse_image_metadata_missing_hfr() {
        let metadata_json = r#"{
            "FileName": "/path/to/image.fits",
            "FilterName": "Ha",
            "DetectedStars": 342,
            "ExposureStartTime": "2023-08-27T10:00:00Z"
        }"#;

        let result = parse_image_metadata(1, 2, "Test Target", metadata_json, "Ha", 0).unwrap();

        assert_eq!(result.hfr, None);
        assert_eq!(result.star_count, Some(342));
    }

    #[test]
    fn test_parse_image_metadata_null_values() {
        let metadata_json = r#"{
            "FileName": "/path/to/image.fits",
            "FilterName": "Ha",
            "HFR": null,
            "DetectedStars": null,
            "ExposureStartTime": "2023-08-27T10:00:00Z"
        }"#;

        let result = parse_image_metadata(1, 2, "Test Target", metadata_json, "Ha", 0).unwrap();

        assert_eq!(result.hfr, None);
        assert_eq!(result.star_count, None);
    }

    #[test]
    fn test_parse_image_metadata_invalid_json() {
        let metadata_json = "not json";

        let result = parse_image_metadata(1, 2, "Test Target", metadata_json, "Ha", 0);

        assert!(result.is_err());
    }

    #[test]
    fn test_statistical_rejection_creation() {
        let rejection = StatisticalRejection {
            image_id: 123,
            reason: "Test Reason".to_string(),
            details: "Test Details".to_string(),
        };

        assert_eq!(rejection.image_id, 123);
        assert_eq!(rejection.reason, "Test Reason");
        assert_eq!(rejection.details, "Test Details");
    }

    #[test]
    fn test_calculate_median_empty() {
        let grader = StatisticalGrader::new(StatisticalGradingConfig::default());
        let mut values: Vec<f64> = vec![];
        assert_eq!(grader.calculate_median(&mut values), 0.0);
    }

    #[test]
    fn test_calculate_median_single() {
        let grader = StatisticalGrader::new(StatisticalGradingConfig::default());
        let mut values = vec![5.0];
        assert_eq!(grader.calculate_median(&mut values), 5.0);
    }

    #[test]
    fn test_calculate_median_odd() {
        let grader = StatisticalGrader::new(StatisticalGradingConfig::default());
        let mut values = vec![3.0, 1.0, 2.0];
        assert_eq!(grader.calculate_median(&mut values), 2.0);
    }

    #[test]
    fn test_calculate_median_even() {
        let grader = StatisticalGrader::new(StatisticalGradingConfig::default());
        let mut values = vec![4.0, 2.0, 3.0, 1.0];
        assert_eq!(grader.calculate_median(&mut values), 2.5);
    }

    #[test]
    fn test_calculate_stddev_empty() {
        let grader = StatisticalGrader::new(StatisticalGradingConfig::default());
        let values: Vec<f64> = vec![];
        assert_eq!(grader.calculate_stddev(&values, 0.0), 0.0);
    }

    #[test]
    fn test_calculate_stddev_single() {
        let grader = StatisticalGrader::new(StatisticalGradingConfig::default());
        let values = vec![5.0];
        assert_eq!(grader.calculate_stddev(&values, 5.0), 0.0);
    }

    #[test]
    fn test_calculate_stddev_normal() {
        let grader = StatisticalGrader::new(StatisticalGradingConfig::default());
        let values = vec![2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let mean = 5.0;
        let stddev = grader.calculate_stddev(&values, mean);
        // Calculate expected value:
        // Mean = 5.0
        // Values: [2, 4, 4, 4, 5, 5, 7, 9]
        // Deviations from mean: [-3, -1, -1, -1, 0, 0, 2, 4]
        // Squared deviations: [9, 1, 1, 1, 0, 0, 4, 16] = 32
        // Variance = 32 / (8-1) = 32/7 ≈ 4.571
        // StdDev = sqrt(4.571) ≈ 2.138
        assert!((stddev - 2.138).abs() < 0.01);
    }

    #[test]
    fn test_calculate_median_i32_empty() {
        let grader = StatisticalGrader::new(StatisticalGradingConfig::default());
        let mut values: Vec<i32> = vec![];
        assert_eq!(grader.calculate_median_i32(&mut values), 0.0);
    }

    #[test]
    fn test_calculate_median_i32_single() {
        let grader = StatisticalGrader::new(StatisticalGradingConfig::default());
        let mut values = vec![42];
        assert_eq!(grader.calculate_median_i32(&mut values), 42.0);
    }

    #[test]
    fn test_calculate_median_i32_odd() {
        let grader = StatisticalGrader::new(StatisticalGradingConfig::default());
        let mut values = vec![3, 1, 2];
        assert_eq!(grader.calculate_median_i32(&mut values), 2.0);
    }

    #[test]
    fn test_calculate_median_i32_even() {
        let grader = StatisticalGrader::new(StatisticalGradingConfig::default());
        let mut values = vec![40, 20, 30, 10];
        assert_eq!(grader.calculate_median_i32(&mut values), 25.0);
    }

    #[test]
    fn test_calculate_stddev_i32_empty() {
        let grader = StatisticalGrader::new(StatisticalGradingConfig::default());
        let values: Vec<i32> = vec![];
        assert_eq!(grader.calculate_stddev_i32(&values, 0.0), 0.0);
    }

    #[test]
    fn test_calculate_stddev_i32_single() {
        let grader = StatisticalGrader::new(StatisticalGradingConfig::default());
        let values = vec![42];
        assert_eq!(grader.calculate_stddev_i32(&values, 42.0), 0.0);
    }

    #[test]
    fn test_calculate_stddev_i32_normal() {
        let grader = StatisticalGrader::new(StatisticalGradingConfig::default());
        let values = vec![10, 20, 30, 40, 50];
        let mean = 30.0;
        let stddev = grader.calculate_stddev_i32(&values, mean);
        // Variance = (400 + 100 + 0 + 100 + 400) / 4 = 1000/4 = 250
        // StdDev = sqrt(250) ≈ 15.811
        assert!((stddev - 15.811).abs() < 0.01);
    }

    #[test]
    fn test_filter_statistics_creation() {
        let stats = FilterStatistics {
            filter_name: "Ha".to_string(),
            hfr_values: vec![2.5, 2.6, 2.4],
            star_counts: vec![100, 110, 105],
            hfr_mean: 2.5,
            hfr_median: 2.5,
            hfr_stddev: 0.1,
            star_count_mean: 105.0,
            star_count_median: 105.0,
            star_count_stddev: 5.0,
        };

        assert_eq!(stats.filter_name, "Ha");
        assert_eq!(stats.hfr_values.len(), 3);
        assert_eq!(stats.star_counts.len(), 3);
        assert_eq!(stats.hfr_mean, 2.5);
    }

    #[test]
    fn test_statistical_grader_with_custom_config() {
        let config = StatisticalGradingConfig {
            enable_hfr_analysis: false,
            hfr_stddev_threshold: 3.0,
            enable_star_count_analysis: true,
            star_count_stddev_threshold: 1.5,
            enable_distribution_analysis: false,
            median_shift_threshold: 0.2,
            enable_cloud_detection: true,
            cloud_threshold: 0.15,
            cloud_baseline_count: 3,
        };

        let grader = StatisticalGrader::new(config.clone());
        assert!(!grader.config.enable_hfr_analysis);
        assert_eq!(grader.config.star_count_stddev_threshold, 1.5);
        assert_eq!(grader.config.cloud_threshold, 0.15);
        assert_eq!(grader.config.cloud_baseline_count, 3);
    }

    #[test]
    fn test_analyze_images_empty() {
        let config = StatisticalGradingConfig::default();
        let grader = StatisticalGrader::new(config);
        let images = vec![];
        let result = grader.analyze_images(images).unwrap();
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_analyze_images_insufficient_for_analysis() {
        let config = StatisticalGradingConfig::default();
        let grader = StatisticalGrader::new(config);
        let images = vec![
            ImageStatistics {
                id: 1,
                target_id: 1,
                target_name: "Test Target".to_string(),
                filter_name: "Ha".to_string(),
                hfr: Some(2.5),
                star_count: Some(100),
                exposure_time: "2023-08-27T10:00:00Z".to_string(),
                original_status: 0,
                metadata_json: "{}".to_string(),
            },
            ImageStatistics {
                id: 2,
                target_id: 1,
                target_name: "Test Target".to_string(),
                filter_name: "Ha".to_string(),
                hfr: Some(2.6),
                star_count: Some(95),
                exposure_time: "2023-08-27T10:05:00Z".to_string(),
                original_status: 0,
                metadata_json: "{}".to_string(),
            },
        ];
        // Less than 3 images, should not perform analysis
        let result = grader.analyze_images(images).unwrap();
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_cloud_detection() {
        let config = StatisticalGradingConfig {
            enable_hfr_analysis: false,
            hfr_stddev_threshold: 2.0,
            enable_star_count_analysis: false,
            star_count_stddev_threshold: 2.0,
            enable_distribution_analysis: false,
            median_shift_threshold: 0.1,
            enable_cloud_detection: true,
            cloud_threshold: 0.2,    // 20% threshold
            cloud_baseline_count: 3, // Need 3 images for baseline
        };
        let grader = StatisticalGrader::new(config);
        let mut images = vec![];

        // First 3 images establish baseline (HFR around 2.5)
        for i in 1..=3 {
            images.push(ImageStatistics {
                id: i,
                target_id: 1,
                target_name: "Test Target".to_string(),
                filter_name: "Ha".to_string(),
                hfr: Some(2.5),
                star_count: Some(100),
                exposure_time: format!("2023-08-27T10:{:02}:00Z", i * 5),
                original_status: 0,
                metadata_json: "{}".to_string(),
            });
        }

        // Image 4: Cloud event - HFR jumps by 30%
        images.push(ImageStatistics {
            id: 4,
            target_id: 1,
            target_name: "Test Target".to_string(),
            filter_name: "Ha".to_string(),
            hfr: Some(3.25), // 30% increase from 2.5
            star_count: Some(100),
            exposure_time: "2023-08-27T10:20:00Z".to_string(),
            original_status: 0,
            metadata_json: "{}".to_string(),
        });

        let result = grader.analyze_images(images).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].image_id, 4);
        assert_eq!(result[0].reason, "Cloud Detection");
        assert!(result[0].details.contains("30%"));
    }
}
