//! Hocus Focus's options, as a profile stores them, turned into the
//! detector settings Hocus Focus runs with.
//!
//! Hocus Focus reads each option with its own default when the profile does
//! not store it. In Simple mode (`UseAdvanced` off) its preset then derives
//! most of them from `Simple_NoiseLevel`, `Simple_PixelScale` and
//! `Simple_FocusRange`, and an optimized snapshot (`UseOptimizedSettings`)
//! goes over the preset. In Advanced mode the stored values apply as they
//! are. [`HocusFocusKnobs::from_profile`] follows `StarDetectionOptions.cs`
//! of Hocus Focus 4 (`InitializeOptions`, `DerivePresetSettings`,
//! `ApplyOptimizedSnapshotToLiveProperties`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::nina_profile::ProfileSettings;
use super::HocusFocusVersion;
use crate::hocus_focus_star_detection::{HocusFocusParams, MeasurementAverage};

/// How a frame's HFR summarizes its stars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Average {
    #[default]
    Median,
    MeanOutliers,
}

/// Every Hocus Focus setting that changes a measurement, resolved from a
/// profile. Two frames measured with equal knobs are measured alike, so the
/// knobs are part of the scan's cache key.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HocusFocusKnobs {
    pub hotpixel_filtering: bool,
    /// Off, hot-pixel filtering replaces every pixel by its 3 × 3 median.
    pub hotpixel_thresholding: bool,
    pub hotpixel_threshold: f64,
    pub noise_reduction_radius: u32,
    pub measurement_noise_reduction: bool,
    pub noise_clipping_multiplier: f64,
    pub star_clipping_multiplier: f64,
    pub structure_layers: u32,
    pub brightness_sensitivity: f64,
    pub star_peak_response: f64,
    pub max_distortion: f64,
    pub star_center_tolerance: f64,
    pub background_box_expansion: u32,
    pub min_star_bounding_box: u32,
    pub min_hfr: f64,
    pub pixel_sample_size: f64,
    /// A fraction of full scale.
    pub saturation_threshold: f64,
    pub exclude_saturated_from_hfr: bool,
    pub measurement_average: Average,
    pub adaptive_binarization: bool,
    pub adaptive_block_size: u32,
    /// 0 when Hocus Focus keeps contaminated stars.
    pub contamination_sensitivity: f64,
    pub detection_binning: u32,
    /// N.I.N.A.'s autofocus crop, 1 when Hocus Focus's "Use AutoFocus Crop"
    /// is off or the profile sets none.
    pub autofocus_inner_crop_ratio: f64,
    pub autofocus_outer_crop_ratio: f64,
    /// N.I.N.A.'s brightest-star count for the frame's HFR; 0 uses all.
    pub autofocus_brightest_stars: u32,
}

/// A stored option, read the way Hocus Focus's options accessor reads it.
struct Options<'a>(&'a std::collections::BTreeMap<String, Value>);

impl Options<'_> {
    fn bool(&self, key: &str, default: bool) -> bool {
        match self.0.get(key) {
            Some(Value::Bool(value)) => *value,
            Some(Value::String(text)) => text.eq_ignore_ascii_case("true"),
            _ => default,
        }
    }

    fn f64(&self, key: &str, default: f64) -> f64 {
        match self.0.get(key) {
            Some(Value::Number(number)) => number.as_f64().filter(|v| v.is_finite()),
            Some(Value::String(text)) => text.trim().parse().ok().filter(|v: &f64| v.is_finite()),
            _ => None,
        }
        .unwrap_or(default)
    }

    fn u32(&self, key: &str, default: u32) -> u32 {
        let value = self.f64(key, default as f64);
        if value.is_finite() && value >= 0.0 {
            value.round().min(10_000.0) as u32
        } else {
            default
        }
    }

    fn text(&self, key: &str) -> Option<&str> {
        match self.0.get(key) {
            Some(Value::String(text)) => Some(text.trim()),
            Some(Value::Number(_)) | Some(Value::Bool(_)) => None,
            _ => None,
        }
    }
}

/// `DetectionBinning` is stored as its enum name (`Bin2`) or number.
fn binning(options: &Options<'_>) -> u32 {
    match options.0.get("DetectionBinning") {
        Some(Value::String(text)) => text
            .trim()
            .trim_start_matches("Bin")
            .parse::<u32>()
            .unwrap_or(1),
        Some(Value::Number(number)) => number.as_u64().unwrap_or(1) as u32,
        _ => 1,
    }
    .clamp(1, 4)
}

impl HocusFocusKnobs {
    /// The knobs a profile gives, and notes on stored options PSF Guard
    /// cannot reproduce.
    pub fn from_profile(profile: &ProfileSettings) -> (Self, Vec<String>) {
        let options = Options(&profile.hocus_focus);
        let mut notes = Vec::new();
        // Hocus Focus's own defaults (InitializeOptions).
        let mut knobs = Self {
            hotpixel_filtering: options.bool("HotpixelFiltering", true),
            hotpixel_thresholding: options.bool("HotpixelThresholdingEnabled", true),
            hotpixel_threshold: options.f64("HotpixelThreshold", 0.001),
            noise_reduction_radius: options.u32("NoiseReductionRadius", 3),
            measurement_noise_reduction: options
                .bool("StarMeasurementNoiseReductionEnabled", false),
            noise_clipping_multiplier: options.f64("NoiseClippingMultiplier", 4.0),
            star_clipping_multiplier: options.f64("StarClippingMultiplier", 2.0),
            structure_layers: options.u32("StructureLayers", 4),
            brightness_sensitivity: options.f64("BrightnessSensitivity", 10.0),
            star_peak_response: options.f64("StarPeakResponse", 0.75),
            max_distortion: options.f64("MaxDistortion", 0.5),
            star_center_tolerance: options.f64("StarCenterTolerance", 0.3),
            background_box_expansion: options.u32("StarBackgroundBoxExpansion", 3),
            min_star_bounding_box: options.u32("MinStarBoundingBoxSize", 5),
            min_hfr: options.f64("MinHFR", 1.2),
            pixel_sample_size: options.f64("PixelSampleSize", 1.0),
            saturation_threshold: options.f64("SaturationThreshold", 0.99),
            exclude_saturated_from_hfr: options.bool("ExcludeSaturatedStarsFromHFR", true),
            measurement_average: match options.text("MeasurementAverage") {
                Some("MeanOutliers") => Average::MeanOutliers,
                _ => Average::Median,
            },
            adaptive_binarization: options.bool("LocallyAdaptiveBinarization", true),
            adaptive_block_size: options.u32("AdaptiveNoiseBlockSize", 128),
            contamination_sensitivity: if options.bool("RejectContaminatedStars", true) {
                options.f64("ContaminationSensitivity", 5.0)
            } else {
                0.0
            },
            detection_binning: binning(&options),
            autofocus_inner_crop_ratio: 1.0,
            autofocus_outer_crop_ratio: 1.0,
            autofocus_brightest_stars: profile.autofocus_brightest_stars,
        };
        if !options.bool("UseAdvanced", false) {
            knobs.derive_preset(&options);
            if options.bool("UseOptimizedSettings", false)
                && let Some(snapshot) = options.text("OptimizedSettingsJson")
            {
                match serde_json::from_str::<serde_json::Map<String, Value>>(snapshot) {
                    Ok(snapshot) => knobs.apply_optimized(&snapshot),
                    Err(_) => notes.push(
                        "Hocus Focus's optimized settings could not be read; PSF Guard uses its \
                         Simple preset."
                            .into(),
                    ),
                }
            }
        }
        if options.bool("UseAutoFocusCrop", true) {
            knobs.autofocus_inner_crop_ratio = profile.autofocus_inner_crop_ratio;
            knobs.autofocus_outer_crop_ratio = profile.autofocus_outer_crop_ratio;
        }
        let dilation = options.u32("StructureDilationCount", 0);
        if dilation != 0 {
            notes.push(format!(
                "Hocus Focus dilates structures {dilation} times, which PSF Guard does not \
                 reproduce."
            ));
        }
        if options.bool("DefocusAwareDonutDetection", false) {
            notes.push(
                "Hocus Focus's defocus-aware donut detection is on, which PSF Guard does not \
                 reproduce."
                    .into(),
            );
        }
        (knobs, notes)
    }

    /// Simple mode's preset (`DerivePresetSettings`).
    fn derive_preset(&mut self, options: &Options<'_>) {
        let noise_level = options.text("Simple_NoiseLevel").unwrap_or("Typical");
        self.hotpixel_filtering = noise_level != "None";
        (
            self.measurement_noise_reduction,
            self.noise_reduction_radius,
        ) = match noise_level {
            "None" => (false, 0),
            "High" => (true, 5),
            _ => (false, 3),
        };
        self.noise_clipping_multiplier = 4.0;
        self.star_clipping_multiplier = 2.0;
        self.structure_layers = 4;
        self.brightness_sensitivity = 10.0;
        if options.text("Simple_FocusRange") == Some("WideRange") {
            self.structure_layers += 1;
            self.brightness_sensitivity += 2.0;
        }
        self.min_star_bounding_box = 5;
        self.pixel_sample_size = 1.0;
        match options.text("Simple_PixelScale") {
            Some("WideField") => {
                self.structure_layers -= 1;
                self.min_star_bounding_box -= 1;
                self.pixel_sample_size = 0.5;
            }
            Some("LongFocalLength") => {
                self.structure_layers += 1;
                self.min_star_bounding_box += 1;
                self.brightness_sensitivity += 2.0;
            }
            _ => {}
        }
        if self.hotpixel_thresholding && self.hotpixel_filtering {
            self.noise_reduction_radius += 1;
        }
        self.star_peak_response = 0.75;
        self.max_distortion = 0.5;
        self.star_center_tolerance = 0.3;
        self.background_box_expansion = 3;
        self.min_hfr = 1.2;
        self.hotpixel_threshold = 0.001;
    }

    /// An optimized snapshot over the preset: the curated knobs win.
    fn apply_optimized(&mut self, snapshot: &serde_json::Map<String, Value>) {
        let number = |key: &str| snapshot.get(key).and_then(Value::as_f64);
        let flag = |key: &str| snapshot.get(key).and_then(Value::as_bool);
        let count = |key: &str| number(key).filter(|v| *v >= 0.0).map(|v| v.round() as u32);
        if let Some(v) = number("BrightnessSensitivity") {
            self.brightness_sensitivity = v;
        }
        if let Some(v) = number("StarClippingMultiplier") {
            self.star_clipping_multiplier = v;
        }
        if let Some(v) = number("NoiseClippingMultiplier") {
            self.noise_clipping_multiplier = v;
        }
        if let Some(v) = number("StarPeakResponse") {
            self.star_peak_response = v;
        }
        if let Some(v) = number("MaxDistortion") {
            self.max_distortion = v;
        }
        if let Some(v) = number("MinHFR") {
            self.min_hfr = v;
        }
        if let Some(v) = number("StarCenterTolerance") {
            self.star_center_tolerance = v;
        }
        if let Some(v) = count("StructureLayers") {
            self.structure_layers = v;
        }
        if let Some(v) = count("NoiseReductionRadius") {
            self.noise_reduction_radius = v;
        }
        if let Some(v) = count("MinStarBoundingBoxSize") {
            self.min_star_bounding_box = v;
        }
        if let Some(v) = flag("HotpixelThresholdingEnabled") {
            self.hotpixel_thresholding = v;
        }
        if let Some(v) = number("HotpixelThreshold") {
            self.hotpixel_threshold = v;
        }
        if let Some(v) = flag("LocallyAdaptiveBinarization") {
            self.adaptive_binarization = v;
        }
        if let Some(v) = count("AdaptiveNoiseBlockSize") {
            self.adaptive_block_size = v;
        }
    }

    /// Seiza's parameters for these knobs at a Hocus Focus version.
    pub fn params(&self, version: HocusFocusVersion) -> HocusFocusParams {
        let mut params = match version {
            HocusFocusVersion::V3 => HocusFocusParams::default(),
            HocusFocusVersion::V4 => HocusFocusParams::v4(),
        };
        params.hotpixel_filtering = self.hotpixel_filtering;
        // Seiza replaces a pixel by its median when they differ by more
        // than the threshold, so 0 replaces every one, as Hocus Focus does
        // without thresholding.
        params.hotpixel_threshold = if self.hotpixel_thresholding {
            self.hotpixel_threshold
        } else {
            0.0
        };
        params.noise_reduction_radius = self.noise_reduction_radius as usize;
        params.measurement_noise_reduction = self.measurement_noise_reduction;
        params.noise_clipping_multiplier = self.noise_clipping_multiplier;
        params.star_clipping_multiplier = self.star_clipping_multiplier;
        params.structure_layers = self.structure_layers as usize;
        params.sensitivity = self.brightness_sensitivity;
        params.peak_response = self.star_peak_response;
        params.max_distortion = self.max_distortion;
        params.star_center_tolerance = self.star_center_tolerance;
        params.background_box_expansion = self.background_box_expansion as usize;
        params.min_star_size = self.min_star_bounding_box as usize;
        params.min_hfr = self.min_hfr;
        params.pixel_sample_size = self.pixel_sample_size;
        params.saturation_threshold = self.saturation_threshold * 65535.0;
        params.keep_saturated_stars = self.exclude_saturated_from_hfr;
        params.measurement_average = match self.measurement_average {
            Average::Median => MeasurementAverage::Median,
            Average::MeanOutliers => MeasurementAverage::MeanOutliers,
        };
        params.adaptive_binarization = self.adaptive_binarization;
        params.adaptive_block_size = self.adaptive_block_size.max(1) as usize;
        params.contamination_sensitivity = self.contamination_sensitivity;
        params.detection_binning = self.detection_binning.max(1) as usize;
        params.autofocus_inner_crop_ratio = self.autofocus_inner_crop_ratio;
        params.autofocus_outer_crop_ratio = self.autofocus_outer_crop_ratio;
        params.autofocus_brightest_stars = self.autofocus_brightest_stars as usize;
        params
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hocus_focus_star_detection::TelescopeClass;
    use serde_json::json;

    fn profile(options: Value) -> ProfileSettings {
        ProfileSettings {
            hocus_focus: serde_json::from_value(options).unwrap(),
            ..Default::default()
        }
    }

    #[test]
    fn simple_mode_matches_seizas_presets() {
        for (scale, class) in [
            ("Typical", TelescopeClass::Standard),
            ("WideField", TelescopeClass::WideField),
            ("LongFocalLength", TelescopeClass::LongFocalLength),
        ] {
            let (knobs, notes) =
                HocusFocusKnobs::from_profile(&profile(json!({"Simple_PixelScale": scale})));
            assert!(notes.is_empty());
            let ours = knobs.params(HocusFocusVersion::V4);
            let seiza = HocusFocusParams::v4_for_telescope_class(class);
            assert_eq!(ours.structure_layers, seiza.structure_layers, "{scale}");
            assert_eq!(ours.min_star_size, seiza.min_star_size, "{scale}");
            assert_eq!(ours.pixel_sample_size, seiza.pixel_sample_size, "{scale}");
            assert_eq!(ours.sensitivity, seiza.sensitivity, "{scale}");
            assert_eq!(
                ours.noise_reduction_radius, seiza.noise_reduction_radius,
                "{scale}"
            );
            assert_eq!(ours.min_hfr, seiza.min_hfr, "{scale}");
            assert_eq!(
                ours.keep_saturated_stars, seiza.keep_saturated_stars,
                "{scale}"
            );
            assert_eq!(
                ours.saturation_threshold, seiza.saturation_threshold,
                "{scale}"
            );
            assert_eq!(
                ours.contamination_sensitivity,
                seiza.contamination_sensitivity
            );
        }
    }

    #[test]
    fn simple_mode_ignores_stored_advanced_values() {
        // A profile switched back to Simple keeps its Advanced values stored.
        let (knobs, _) = HocusFocusKnobs::from_profile(&profile(json!({
            "UseAdvanced": false,
            "Simple_PixelScale": "WideField",
            "Simple_FocusRange": "WideRange",
            "Simple_NoiseLevel": "High",
            "StructureLayers": 7,
            "BrightnessSensitivity": 3.0,
        })));
        assert_eq!(knobs.structure_layers, 4);
        assert_eq!(knobs.brightness_sensitivity, 12.0);
        assert_eq!(knobs.noise_reduction_radius, 6);
        assert!(knobs.measurement_noise_reduction);
        assert_eq!(knobs.min_star_bounding_box, 4);
    }

    #[test]
    fn advanced_mode_takes_stored_values_over_hocus_focus_defaults() {
        let c925 = ProfileSettings {
            autofocus_brightest_stars: 30,
            autofocus_inner_crop_ratio: 0.6,
            hocus_focus: serde_json::from_value(json!({
                "UseAdvanced": true,
                "Simple_PixelScale": "LongFocalLength",
                "NoiseReductionRadius": 4,
                "StructureLayers": 6,
                "BrightnessSensitivity": 1.2000000000000002,
                "MinStarBoundingBoxSize": 6,
            }))
            .unwrap(),
            ..Default::default()
        };
        let (knobs, _) = HocusFocusKnobs::from_profile(&c925);
        assert_eq!(knobs.noise_reduction_radius, 4);
        assert_eq!(knobs.structure_layers, 6);
        assert_eq!(knobs.brightness_sensitivity, 1.2000000000000002);
        assert_eq!(knobs.min_star_bounding_box, 6);
        // Unstored: Hocus Focus's defaults, not the preset's.
        assert_eq!(knobs.pixel_sample_size, 1.0);
        assert_eq!(knobs.min_hfr, 1.2);
        assert_eq!(knobs.autofocus_brightest_stars, 30);
        assert_eq!(knobs.autofocus_inner_crop_ratio, 0.6);
        let params = knobs.params(HocusFocusVersion::V4);
        assert_eq!(params.autofocus_brightest_stars, 30);
        assert_eq!(params.sensitivity, 1.2000000000000002);
    }

    #[test]
    fn the_crop_follows_use_autofocus_crop_and_optimized_settings_apply() {
        let mut settings = profile(json!({
            "UseAutoFocusCrop": false,
            "UseOptimizedSettings": true,
            "OptimizedSettingsJson": "{\"BrightnessSensitivity\": 7.5, \"StructureLayers\": 5}",
            "RejectContaminatedStars": false,
            "DetectionBinning": "Bin2",
            "HotpixelThresholdingEnabled": false,
        }));
        settings.autofocus_inner_crop_ratio = 0.5;
        let (knobs, _) = HocusFocusKnobs::from_profile(&settings);
        assert_eq!(knobs.autofocus_inner_crop_ratio, 1.0);
        assert_eq!(knobs.brightness_sensitivity, 7.5);
        assert_eq!(knobs.structure_layers, 5);
        assert_eq!(knobs.contamination_sensitivity, 0.0);
        assert_eq!(knobs.detection_binning, 2);
        assert_eq!(knobs.params(HocusFocusVersion::V4).hotpixel_threshold, 0.0);
        let (_, notes) = HocusFocusKnobs::from_profile(&profile(json!({
            "UseOptimizedSettings": true,
            "OptimizedSettingsJson": "not json",
            "StructureDilationCount": 2,
        })));
        assert_eq!(notes.len(), 2);
    }
}
