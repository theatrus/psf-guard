//! The N.I.N.A. profile settings that decide a frame's recorded star count
//! and HFR, and reading them from a `.profile` file.
//!
//! A profile is N.I.N.A.'s DataContract XML. The settings that matter sit in
//! four places: `ImageSettings` (star sensitivity, noise reduction and the
//! autostretch the built-in detector finds edges on), `FocuserSettings` (the
//! autofocus crop and brightest-star count, which N.I.N.A. passes to every
//! star detection, not only autofocus), `ApplicationSettings` (which star
//! detector is selected) and `PluginSettings` (Hocus Focus's options, by
//! its plugin GUID). [`ProfileSettings`] is the snapshot of all of them, as
//! an import reads it and the PSF Guard plugin sends it.

use std::collections::BTreeMap;

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};

use super::{NoiseReduction, Sensitivity};

/// Hocus Focus's plugin GUID, which keys its options in `PluginSettings`.
pub const HOCUS_FOCUS_PLUGIN: &str = "0f1d10b6-d306-4168-b751-d454cbac9670";
/// N.I.N.A.'s pluggable-behaviour key for the star detector.
const STAR_DETECTION_BEHAVIOUR: &str = "NINA.Image.ImageAnalysis.IStarDetection";
/// Hocus Focus's star detector, as N.I.N.A. records the selection.
const HOCUS_FOCUS_STAR_DETECTION: &str =
    "NINA.Joko.Plugins.HocusFocus.StarDetection.HocusFocusStarDetection";

/// The largest profile an import reads. Real profiles are 30–50 KB.
pub const MAX_PROFILE_BYTES: usize = 2 * 1024 * 1024;
/// The deepest element nesting a profile may have; real ones reach 8.
const MAX_DEPTH: usize = 32;
/// The most elements a profile may hold; real ones have about 1,500.
const MAX_ELEMENTS: usize = 50_000;
/// The longest Hocus Focus value kept, which bounds `OptimizedSettingsJson`.
const MAX_VALUE_CHARS: usize = 16 * 1024;

/// The Hocus Focus options that change which stars it finds or how it
/// measures them. Others (autofocus timing, paths, PSF modelling) are not
/// kept.
pub const HOCUS_FOCUS_KEYS: &[&str] = &[
    "UseAdvanced",
    "Simple_NoiseLevel",
    "Simple_PixelScale",
    "Simple_FocusRange",
    "UseOptimizedSettings",
    "OptimizedSettingsJson",
    "UseAutoFocusCrop",
    "HotpixelFiltering",
    "HotpixelThresholdingEnabled",
    "HotpixelThreshold",
    "NoiseReductionRadius",
    "StarMeasurementNoiseReductionEnabled",
    "NoiseClippingMultiplier",
    "StarClippingMultiplier",
    "StructureLayers",
    "StructureDilationSize",
    "StructureDilationCount",
    "BrightnessSensitivity",
    "StarPeakResponse",
    "MaxDistortion",
    "StarCenterTolerance",
    "StarBackgroundBoxExpansion",
    "MinStarBoundingBoxSize",
    "MinHFR",
    "PixelSampleSize",
    "SaturationThreshold",
    "ExcludeSaturatedStarsFromHFR",
    "MeasurementAverage",
    "LocallyAdaptiveBinarization",
    "AdaptiveNoiseBlockSize",
    "ContaminationSensitivity",
    "RejectContaminatedStars",
    "DetectionBinning",
    "DefocusAwareDonutDetection",
    "UsePerFilterSettings",
];

/// Which star detector a profile selects.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum StarDetectorSelection {
    /// N.I.N.A.'s built-in detector: no selection recorded.
    #[default]
    Nina,
    HocusFocus,
    /// Another plugin's detector, by its type name.
    Other(String),
}

impl From<String> for StarDetectorSelection {
    fn from(value: String) -> Self {
        match value.as_str() {
            "nina" | "" | "NINA" => Self::Nina,
            "hocus_focus" | HOCUS_FOCUS_STAR_DETECTION => Self::HocusFocus,
            _ => Self::Other(value),
        }
    }
}

impl From<StarDetectorSelection> for String {
    fn from(value: StarDetectorSelection) -> Self {
        match value {
            StarDetectorSelection::Nina => "nina".into(),
            StarDetectorSelection::HocusFocus => "hocus_focus".into(),
            StarDetectorSelection::Other(name) => name,
        }
    }
}

fn one() -> f64 {
    1.0
}

fn default_stretch_factor() -> f64 {
    0.2
}

fn default_black_clipping() -> f64 {
    -2.8
}

/// The profile settings that decide a frame's recorded star count and HFR:
/// one snapshot, as N.I.N.A. held them while it took the frame. Stored in
/// the registry, so a field this build does not know is ignored there; a
/// plugin's request is held to the known fields by [`strict_settings`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileSettings {
    #[serde(default)]
    pub star_detector: StarDetectorSelection,
    #[serde(default)]
    pub star_sensitivity: Sensitivity,
    #[serde(default)]
    pub noise_reduction: NoiseReduction,
    /// The autostretch's target median, which the built-in detector finds
    /// edges on.
    #[serde(default = "default_stretch_factor")]
    pub auto_stretch_factor: f64,
    /// The autostretch's shadow clipping, in MADs.
    #[serde(default = "default_black_clipping")]
    pub black_clipping: f64,
    /// Below 1, stars are found only in the centred region this fraction of
    /// the frame covers.
    #[serde(default = "one")]
    pub autofocus_inner_crop_ratio: f64,
    /// With the inner ratio below 1 as well, the region outside the inner
    /// one and inside this one.
    #[serde(default = "one")]
    pub autofocus_outer_crop_ratio: f64,
    /// Above 0, the frame's HFR comes from this many of the brightest stars.
    #[serde(default)]
    pub autofocus_brightest_stars: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focal_length_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel_size_um: Option<f64>,
    /// Hocus Focus's stored options among [`HOCUS_FOCUS_KEYS`], by its own
    /// names. A key absent here takes Hocus Focus's default.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hocus_focus: BTreeMap<String, serde_json::Value>,
}

/// The fields [`ProfileSettings`] reads.
const PROFILE_FIELDS: &[&str] = &[
    "star_detector",
    "star_sensitivity",
    "noise_reduction",
    "auto_stretch_factor",
    "black_clipping",
    "autofocus_inner_crop_ratio",
    "autofocus_outer_crop_ratio",
    "autofocus_brightest_stars",
    "focal_length_mm",
    "pixel_size_um",
    "hocus_focus",
];

/// [`ProfileSettings`] as a plugin's request sends them: a field this build
/// does not read is refused, so a misspelt one cannot pass as a default.
pub fn strict_settings<'de, D>(deserializer: D) -> Result<ProfileSettings, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let value = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
    if let Some(unknown) = value
        .keys()
        .find(|key| !PROFILE_FIELDS.contains(&key.as_str()))
    {
        return Err(D::Error::custom(format!(
            "unknown settings field `{unknown}`"
        )));
    }
    serde_json::from_value(serde_json::Value::Object(value)).map_err(D::Error::custom)
}

impl Default for ProfileSettings {
    fn default() -> Self {
        Self {
            star_detector: StarDetectorSelection::default(),
            star_sensitivity: Sensitivity::default(),
            noise_reduction: NoiseReduction::default(),
            auto_stretch_factor: default_stretch_factor(),
            black_clipping: default_black_clipping(),
            autofocus_inner_crop_ratio: 1.0,
            autofocus_outer_crop_ratio: 1.0,
            autofocus_brightest_stars: 0,
            focal_length_mm: None,
            pixel_size_um: None,
            hocus_focus: BTreeMap::new(),
        }
    }
}

impl ProfileSettings {
    /// Refuse values N.I.N.A. could not have held: a crop ratio outside
    /// (0, 1], a non-finite stretch, an unknown or oversized Hocus Focus
    /// value.
    pub fn validate(&self) -> Result<(), String> {
        for (name, ratio) in [
            (
                "autofocus_inner_crop_ratio",
                self.autofocus_inner_crop_ratio,
            ),
            (
                "autofocus_outer_crop_ratio",
                self.autofocus_outer_crop_ratio,
            ),
        ] {
            if !(ratio.is_finite() && ratio > 0.0 && ratio <= 1.0) {
                return Err(format!("{name} must be above 0 and at most 1"));
            }
        }
        if !(self.auto_stretch_factor.is_finite() && self.black_clipping.is_finite()) {
            return Err("the autostretch values must be finite numbers".into());
        }
        if self.autofocus_brightest_stars > 100_000 {
            return Err("autofocus_brightest_stars is at most 100000".into());
        }
        for value in [self.focal_length_mm, self.pixel_size_um]
            .into_iter()
            .flatten()
        {
            if !(value.is_finite() && value > 0.0) {
                return Err("focal length and pixel size must be positive".into());
            }
        }
        if let StarDetectorSelection::Other(name) = &self.star_detector
            && name.chars().count() > 256
        {
            return Err("star_detector is at most 256 characters".into());
        }
        for (key, value) in &self.hocus_focus {
            if !HOCUS_FOCUS_KEYS.contains(&key.as_str()) {
                return Err(format!(
                    "`{key}` is not a Hocus Focus option PSF Guard reads"
                ));
            }
            let fits = match value {
                serde_json::Value::Bool(_) => true,
                serde_json::Value::Number(number) => number.as_f64().is_some_and(f64::is_finite),
                serde_json::Value::String(text) => text.chars().count() <= MAX_VALUE_CHARS,
                _ => false,
            };
            if !fits {
                return Err(format!(
                    "Hocus Focus option `{key}` has an unreadable value"
                ));
            }
        }
        Ok(())
    }
}

/// What an import read from a profile.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParsedProfile {
    /// N.I.N.A.'s profile GUID, lower case.
    pub profile_id: String,
    pub name: Option<String>,
    pub settings: ProfileSettings,
    /// Settings PSF Guard read but cannot reproduce, such as Hocus Focus's
    /// per-filter options.
    pub warnings: Vec<String>,
}

/// One element of the profile, with its local name.
#[derive(Debug, Default)]
struct Element {
    name: String,
    /// The `i:type` attribute's value, such as `b:double`.
    value_type: Option<String>,
    text: String,
    children: Vec<Element>,
}

impl Element {
    fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.name == name)
    }

    fn path(&self, names: &[&str]) -> Option<&Element> {
        names
            .iter()
            .try_fold(self, |element, name| element.child(name))
    }

    fn text_at(&self, names: &[&str]) -> Option<&str> {
        self.path(names)
            .map(|element| element.text.trim())
            .filter(|text| !text.is_empty())
    }
}

fn local_name(raw: &str) -> String {
    raw.rsplit(':').next().unwrap_or_default().to_string()
}

/// Read the XML into a tree, refusing DTDs, entities other than XML's own
/// and character references, and documents too large or too deep.
fn read_tree(xml: &[u8]) -> Result<Element, String> {
    if xml.len() > MAX_PROFILE_BYTES {
        return Err(format!(
            "the file is larger than {} MB, too large for a N.I.N.A. profile",
            MAX_PROFILE_BYTES / (1024 * 1024)
        ));
    }
    let text = std::str::from_utf8(xml).map_err(|_| "the file is not UTF-8 text".to_string())?;
    let text = text.trim_start_matches('\u{feff}');
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Element> = vec![Element::default()];
    let mut elements = 0usize;
    let open = |start: &quick_xml::events::BytesStart<'_>| -> Element {
        let value_type = start
            .attributes()
            .flatten()
            .find(|attribute| local_name(attribute.key.as_ref()) == "type")
            .map(|attribute| attribute.value.to_string());
        Element {
            name: local_name(start.name().as_ref()),
            value_type,
            ..Default::default()
        }
    };
    loop {
        let event = reader
            .read_event()
            .map_err(|error| format!("the file is not valid XML: {error}"))?;
        match event {
            Event::DocType(_) => {
                return Err("the file declares a DTD, which a N.I.N.A. profile never does".into());
            }
            Event::Start(start) => {
                elements += 1;
                if elements > MAX_ELEMENTS || stack.len() > MAX_DEPTH {
                    return Err("the file is too large or too deeply nested for a profile".into());
                }
                stack.push(open(&start));
            }
            Event::Empty(start) => {
                elements += 1;
                if elements > MAX_ELEMENTS {
                    return Err("the file is too large for a profile".into());
                }
                let element = open(&start);
                stack.last_mut().expect("root").children.push(element);
            }
            Event::End(_) => {
                let element = stack.pop().expect("balanced");
                let Some(parent) = stack.last_mut() else {
                    return Err("the file is not valid XML".into());
                };
                parent.children.push(element);
            }
            Event::Text(text) => {
                stack
                    .last_mut()
                    .expect("root")
                    .text
                    .push_str(&text.xml10_content());
            }
            Event::CData(data) => {
                stack.last_mut().expect("root").text.push_str(&data);
            }
            Event::GeneralRef(reference) => {
                let resolved = if reference.is_char_ref() {
                    reference.resolve_char_ref().ok().flatten().ok_or_else(|| {
                        "the file holds an invalid character reference".to_string()
                    })?
                } else {
                    match &*reference {
                        "amp" => '&',
                        "lt" => '<',
                        "gt" => '>',
                        "quot" => '"',
                        "apos" => '\'',
                        other => {
                            return Err(format!(
                                "the file uses the entity `&{other};`, which a N.I.N.A. profile never does"
                            ));
                        }
                    }
                };
                stack.last_mut().expect("root").text.push(resolved);
            }
            Event::Eof => break,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) => {}
        }
    }
    if stack.len() != 1 {
        return Err("the file ends inside an element".into());
    }
    let mut document = stack.pop().expect("root");
    match document.children.len() {
        1 => Ok(document.children.pop().expect("one")),
        _ => Err("the file is not a single XML document".into()),
    }
}

/// A Hocus Focus option's value as its `i:type` declares it.
fn typed_value(element: &Element) -> Option<serde_json::Value> {
    let text = element.text.trim();
    match element.value_type.as_deref().map(local_name).as_deref() {
        Some("boolean") => text.parse::<bool>().ok().map(serde_json::Value::Bool),
        Some("int" | "long" | "short") => text.parse::<i64>().ok().map(serde_json::Value::from),
        Some("double" | "float" | "decimal") => text
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(serde_json::Value::from),
        _ => Some(serde_json::Value::String(
            element.text.chars().take(MAX_VALUE_CHARS).collect(),
        )),
    }
}

fn sensitivity(text: Option<&str>) -> Sensitivity {
    match text {
        Some("Normal") => Sensitivity::Normal,
        Some("Highest") => Sensitivity::Highest,
        _ => Sensitivity::High,
    }
}

fn noise_reduction(text: Option<&str>) -> NoiseReduction {
    match text {
        Some("Normal") => NoiseReduction::Normal,
        Some("High") => NoiseReduction::High,
        Some("Highest") => NoiseReduction::Highest,
        Some("Median") => NoiseReduction::Median,
        _ => NoiseReduction::None,
    }
}

/// Read a N.I.N.A. `.profile` file's star detection settings.
pub fn parse_profile(xml: &[u8]) -> Result<ParsedProfile, String> {
    let root = read_tree(xml)?;
    if root.name != "Profile" {
        return Err(format!(
            "the file holds `{}`, not a N.I.N.A. profile",
            root.name
        ));
    }
    let profile_id = root
        .text_at(&["Id"])
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
        .map(|id| id.hyphenated().to_string())
        .ok_or_else(|| "the profile has no valid Id".to_string())?;
    let number = |path: &[&str]| {
        root.text_at(path)
            .and_then(|text| text.parse::<f64>().ok())
            .filter(|value| value.is_finite())
    };
    let mut warnings = Vec::new();

    let star_detector = root
        .path(&["ApplicationSettings", "SelectedPluggableBehaviors"])
        .into_iter()
        .flat_map(|behaviours| behaviours.children.iter())
        .find(|pair| pair.text_at(&["key"]) == Some(STAR_DETECTION_BEHAVIOUR))
        .and_then(|pair| pair.text_at(&["value"]))
        .map(|value| StarDetectorSelection::from(value.to_string()))
        .unwrap_or_default();
    if let StarDetectorSelection::Other(name) = &star_detector {
        warnings.push(format!(
            "Not supported: star detector {name}; N.I.N.A.'s detector is used"
        ));
    }

    let mut hocus_focus = BTreeMap::new();
    let plugins = root
        .path(&["PluginSettings", "pluginStorage"])
        .into_iter()
        .flat_map(|storage| storage.children.iter());
    for plugin in plugins {
        if plugin.text_at(&["Key"]) != Some(HOCUS_FOCUS_PLUGIN) {
            continue;
        }
        let options = plugin
            .child("Value")
            .into_iter()
            .flat_map(|values| values.children.iter());
        for option in options {
            let Some(key) = option.text_at(&["Key"]) else {
                continue;
            };
            if !HOCUS_FOCUS_KEYS.contains(&key) {
                continue;
            }
            if let Some(value) = option.child("Value").and_then(typed_value) {
                hocus_focus.insert(key.to_string(), value);
            }
        }
    }
    if hocus_focus.get("UsePerFilterSettings") == Some(&serde_json::Value::Bool(true)) {
        warnings.push(
            "Not reproduced: Hocus Focus per-filter settings; the shared settings are used".into(),
        );
    }

    let ratio = |name: &str| {
        number(&["FocuserSettings", name])
            .filter(|ratio| *ratio > 0.0 && *ratio <= 1.0)
            .unwrap_or(1.0)
    };
    let settings = ProfileSettings {
        star_detector,
        star_sensitivity: sensitivity(root.text_at(&["ImageSettings", "StarSensitivity"])),
        noise_reduction: noise_reduction(root.text_at(&["ImageSettings", "NoiseReduction"])),
        auto_stretch_factor: number(&["ImageSettings", "AutoStretchFactor"])
            .unwrap_or_else(default_stretch_factor),
        black_clipping: number(&["ImageSettings", "BlackClipping"])
            .unwrap_or_else(default_black_clipping),
        autofocus_inner_crop_ratio: ratio("AutoFocusInnerCropRatio"),
        autofocus_outer_crop_ratio: ratio("AutoFocusOuterCropRatio"),
        autofocus_brightest_stars: number(&["FocuserSettings", "AutoFocusUseBrightestStars"])
            .filter(|count| *count >= 0.0)
            .map(|count| count.min(100_000.0) as u32)
            .unwrap_or(0),
        focal_length_mm: number(&["TelescopeSettings", "FocalLength"]).filter(|v| *v > 0.0),
        pixel_size_um: number(&["CameraSettings", "PixelSize"]).filter(|v| *v > 0.0),
        hocus_focus,
    };
    settings.validate()?;
    Ok(ParsedProfile {
        profile_id,
        name: root
            .text_at(&["Name"])
            .map(|name| name.chars().take(200).collect()),
        settings,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/nina-profiles")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    #[test]
    fn reads_each_real_profile() {
        let c925 = parse_profile(&fixture("c925.profile")).unwrap();
        assert_eq!(c925.profile_id, "8f8e9c44-ad9c-44cc-a872-d319f98cae59");
        assert_eq!(c925.name.as_deref(), Some("C925"));
        let settings = &c925.settings;
        assert_eq!(settings.star_detector, StarDetectorSelection::HocusFocus);
        assert_eq!(settings.star_sensitivity, Sensitivity::High);
        assert_eq!(settings.noise_reduction, NoiseReduction::None);
        assert_eq!(settings.autofocus_brightest_stars, 30);
        assert_eq!(settings.autofocus_inner_crop_ratio, 1.0);
        assert_eq!(settings.focal_length_mm, Some(2350.0));
        assert_eq!(settings.pixel_size_um, Some(3.76));
        assert_eq!(settings.hocus_focus["UseAdvanced"], json!(true));
        assert_eq!(settings.hocus_focus["StructureLayers"], json!(6));
        assert_eq!(
            settings.hocus_focus["BrightnessSensitivity"],
            json!(1.2000000000000002)
        );
        assert_eq!(settings.hocus_focus["MinStarBoundingBoxSize"], json!(6));
        assert_eq!(
            settings.hocus_focus["Simple_PixelScale"],
            json!("LongFocalLength")
        );
        // Autofocus timing is not a detection setting.
        assert!(!settings.hocus_focus.contains_key("AutoFocusTimeoutSeconds"));
        assert!(c925.warnings.is_empty());

        let redcat = parse_profile(&fixture("starfront-mini.profile")).unwrap();
        assert_eq!(redcat.profile_id, "46d7a6e3-3ae8-48a9-8d29-598e3647f131");
        assert_eq!(redcat.settings.autofocus_inner_crop_ratio, 0.7);
        assert_eq!(redcat.settings.autofocus_outer_crop_ratio, 1.0);
        assert!(!redcat.settings.hocus_focus.contains_key("UseAdvanced"));
        assert_eq!(
            redcat.settings.hocus_focus["Simple_PixelScale"],
            json!("WideField")
        );

        let ultracat = parse_profile(&fixture("ultracat.profile")).unwrap();
        assert_eq!(ultracat.profile_id, "713cd923-3ca5-4273-b454-bb063788045a");
        assert_eq!(ultracat.settings.hocus_focus["PixelSampleSize"], json!(0.5));
        assert_eq!(ultracat.settings.hocus_focus["UseAdvanced"], json!(true));

        for name in ["askar.profile", "astropc1.profile", "starfront-std.profile"] {
            let parsed = parse_profile(&fixture(name)).unwrap();
            assert_eq!(
                parsed.settings.star_detector,
                StarDetectorSelection::HocusFocus,
                "{name}"
            );
            assert_eq!(
                parsed.settings.hocus_focus["NoiseReductionRadius"],
                json!(4)
            );
        }
    }

    #[test]
    fn a_hostile_or_foreign_file_is_refused() {
        let doctype = br#"<?xml version="1.0"?><!DOCTYPE p [<!ENTITY x SYSTEM "file:///etc/passwd">]><Profile><Id>&x;</Id></Profile>"#;
        assert!(parse_profile(doctype).unwrap_err().contains("DTD"));
        let entity = br#"<Profile><Id>&x;</Id></Profile>"#;
        assert!(parse_profile(entity).unwrap_err().contains("entity"));
        assert!(parse_profile(b"<Other/>")
            .unwrap_err()
            .contains("not a N.I.N.A. profile"));
        assert!(parse_profile(b"<Profile><Id>nope</Id></Profile>")
            .unwrap_err()
            .contains("Id"));
        assert!(parse_profile(b"<Profile><Id>").is_err());
        assert!(parse_profile(&vec![b' '; MAX_PROFILE_BYTES + 1])
            .unwrap_err()
            .contains("too large"));
        let deep = format!(
            "<Profile>{}{}</Profile>",
            "<a>".repeat(MAX_DEPTH + 1),
            "</a>".repeat(MAX_DEPTH + 1)
        );
        assert!(parse_profile(deep.as_bytes())
            .unwrap_err()
            .contains("nested"));
        assert!(parse_profile(&[0xff, 0xfe, 0x00]).is_err());
    }

    #[test]
    fn a_bare_profile_reads_as_ninas_defaults() {
        let parsed = parse_profile(
            "\u{feff}<Profile><Id>DAACEA0C-6D1D-4163-B582-78B2BB178FD6</Id><Name>A &amp; B</Name></Profile>"
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(parsed.profile_id, "daacea0c-6d1d-4163-b582-78b2bb178fd6");
        assert_eq!(parsed.name.as_deref(), Some("A & B"));
        assert_eq!(parsed.settings, ProfileSettings::default());
    }

    #[test]
    fn settings_round_trip_and_unknown_values_are_refused() {
        let parsed = parse_profile(&fixture("c925.profile")).unwrap();
        let json = serde_json::to_value(&parsed.settings).unwrap();
        assert_eq!(json["star_detector"], "hocus_focus");
        assert_eq!(
            serde_json::from_value::<ProfileSettings>(json).unwrap(),
            parsed.settings
        );
        let mut bad = parsed.settings.clone();
        bad.autofocus_inner_crop_ratio = 1.5;
        assert!(bad.validate().is_err());
        let mut unknown = parsed.settings.clone();
        unknown
            .hocus_focus
            .insert("IntermediateSavePath".into(), json!("C:\\"));
        assert!(unknown.validate().is_err());
        let mut nested = parsed.settings;
        nested.hocus_focus.insert("MinHFR".into(), json!({"a": 1}));
        assert!(nested.validate().is_err());
    }
}
