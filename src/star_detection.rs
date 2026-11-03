//! How the quality scan detects stars on each rig's frames.
//!
//! The scan's star count and HFR are compared with what the rig's N.I.N.A.
//! recorded, so it measures the way that rig did: with N.I.N.A.'s built-in
//! detector at the rig's star sensitivity and noise reduction, measured as
//! the N.I.N.A. release that wrote the frame measures, or with Hocus Focus
//! when the rig ran it. Each rig, by the calibration library's rig UUID
//! ([`crate::calibration::rules::light_rig_uuid`]), may have its own
//! choices in the registry; a rig without them uses the default row.
//!
//! [`resolve`] turns a rig's choices and a frame's own facts (its header's
//! SWCREATE, and whether N.I.N.A. recorded an FWHM) into the
//! [`DetectionSettings`] one frame is measured with. The scan stores them
//! beside each measurement, and a measurement is current only while the
//! settings resolved today equal the stored ones, so a changed choice
//! measures the affected frames again on the next scan.

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

use serde::{Deserialize, Serialize};

use crate::hocus_focus_star_detection::{classify_pixel_scale, HocusFocusParams, TelescopeClass};
use crate::nina_star_detection::{MeasurementVersion, StarDetectionParams};

/// Which detector measures a rig's frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectorChoice {
    /// Hocus Focus where the frame's metadata shows N.I.N.A. used it, else
    /// N.I.N.A.'s built-in detector.
    #[default]
    Auto,
    /// N.I.N.A.'s built-in detector, measured as the frame's N.I.N.A.
    /// release measures.
    Nina,
    /// Hocus Focus, at the rig's Hocus Focus version.
    HocusFocus,
}

/// N.I.N.A.'s star sensitivity (Options › Imaging › Star detection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Normal,
    /// N.I.N.A.'s default since October 2023.
    #[default]
    High,
    Highest,
}

/// N.I.N.A.'s noise reduction before star detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseReduction {
    /// N.I.N.A.'s default.
    #[default]
    None,
    Normal,
    High,
    Highest,
    Median,
}

/// The Hocus Focus release whose measurement to reproduce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HocusFocusVersion {
    V3,
    /// Released 2026-07-02.
    #[default]
    V4,
}

/// One rig's star detection choices, or the default row's. A field left out
/// of the registry takes its default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RigStarDetection {
    pub detector: DetectorChoice,
    pub sensitivity: Sensitivity,
    pub noise_reduction: NoiseReduction,
    pub hocus_focus_version: HocusFocusVersion,
}

/// The N.I.N.A. release whose star measurement to reproduce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum NinaMeasurement {
    #[serde(rename = "3.1")]
    Nina31,
    #[default]
    #[serde(rename = "3.2")]
    Nina32,
    #[serde(rename = "3.3")]
    Nina33,
}

impl NinaMeasurement {
    fn seiza(self) -> MeasurementVersion {
        match self {
            Self::Nina31 => MeasurementVersion::Nina31,
            Self::Nina32 => MeasurementVersion::Nina32,
            Self::Nina33 => MeasurementVersion::Nina33,
        }
    }

    /// The measurement of a N.I.N.A. release, by major and minor version:
    /// 3.3 and later measure as 3.3, and releases before 3.2 as 3.1, the
    /// oldest the port reproduces.
    fn of_release((major, minor): (u32, u32)) -> Self {
        match (major, minor) {
            release if release >= (3, 3) => Self::Nina33,
            (3, 2) => Self::Nina32,
            _ => Self::Nina31,
        }
    }
}

/// How one frame is measured: what [`resolve`] decided for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "detector", rename_all = "snake_case")]
pub enum DetectionSettings {
    /// N.I.N.A.'s built-in detector.
    Nina {
        measurement: NinaMeasurement,
        sensitivity: Sensitivity,
        noise_reduction: NoiseReduction,
    },
    /// Hocus Focus as the frame's primary detector.
    HocusFocus { version: HocusFocusVersion },
}

impl Default for DetectionSettings {
    fn default() -> Self {
        resolve(&RigStarDetection::default(), &FrameFacts::default(), false)
    }
}

impl DetectionSettings {
    /// A short description for logs and the analysis CLI.
    pub fn label(&self) -> String {
        match self {
            Self::Nina {
                measurement,
                sensitivity,
                noise_reduction,
            } => format!(
                "N.I.N.A. {} measurement, {sensitivity:?} sensitivity, {noise_reduction:?} noise reduction",
                serde_json::to_value(measurement)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_default()
            ),
            Self::HocusFocus { version } => format!("Hocus Focus {version:?}"),
        }
    }

    /// N.I.N.A. detector parameters, or `None` for Hocus Focus.
    /// `image_scale_arcsec` drives High sensitivity's resize, as N.I.N.A.'s
    /// profile does.
    pub fn nina_params(&self, image_scale_arcsec: Option<f64>) -> Option<StarDetectionParams> {
        let Self::Nina {
            measurement,
            sensitivity,
            noise_reduction,
        } = *self
        else {
            return None;
        };
        use crate::nina_star_detection as nina;
        Some(StarDetectionParams {
            sensitivity: match sensitivity {
                Sensitivity::Normal => nina::StarSensitivity::Normal,
                Sensitivity::High => nina::StarSensitivity::High,
                Sensitivity::Highest => nina::StarSensitivity::Highest,
            },
            noise_reduction: match noise_reduction {
                NoiseReduction::None => nina::NoiseReduction::None,
                NoiseReduction::Normal => nina::NoiseReduction::Normal,
                NoiseReduction::High => nina::NoiseReduction::High,
                NoiseReduction::Highest => nina::NoiseReduction::Highest,
                NoiseReduction::Median => nina::NoiseReduction::Median,
            },
            image_scale_arcsec: image_scale_arcsec
                .filter(|scale| scale.is_finite() && *scale > 0.0),
            measurement: measurement.seiza(),
            ..StarDetectionParams::default()
        })
    }

    /// Hocus Focus parameters, or `None` for N.I.N.A.'s detector: the
    /// version's Simple-mode preset for the rig class the image scale gives.
    pub fn hocus_focus_params(&self, image_scale_arcsec: Option<f64>) -> Option<HocusFocusParams> {
        let Self::HocusFocus { version } = *self else {
            return None;
        };
        let class = image_scale_arcsec
            .filter(|scale| scale.is_finite() && *scale > 0.0)
            .map(classify_pixel_scale)
            .unwrap_or(TelescopeClass::Standard);
        Some(match version {
            HocusFocusVersion::V3 => HocusFocusParams::for_telescope_class(class),
            HocusFocusVersion::V4 => HocusFocusParams::v4_for_telescope_class(class),
        })
    }
}

/// What a frame's own file says about how its rig measured it, kept beside
/// the scan's measurement so its settings can be resolved again without
/// reading the file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FrameFacts {
    /// The calibration library's rig UUID; `None` when the header could not
    /// be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rig_uuid: Option<String>,
    /// SWCREATE, such as `N.I.N.A. 3.2.0.9001 (x64)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub software: Option<String>,
}

impl FrameFacts {
    pub fn of_frame(meta: &crate::commands::import::headers::FrameMeta) -> Self {
        if !meta.readable {
            return Self::default();
        }
        Self {
            rig_uuid: Some(crate::calibration::rules::light_rig_uuid(meta)),
            software: meta.software.clone(),
        }
    }
}

/// The N.I.N.A. release that wrote a frame, as (major, minor), from its
/// SWCREATE. `None` for other software or an unreadable version.
pub fn nina_release(software: Option<&str>) -> Option<(u32, u32)> {
    let software = software?.trim();
    let rest = software
        .strip_prefix("N.I.N.A.")
        .or_else(|| software.strip_prefix("NINA"))?;
    let version = rest.split_whitespace().next()?;
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Whether N.I.N.A. recorded an FWHM for the frame. Its built-in detector
/// through 3.2 leaves FWHM as `"NaN"`, so a number there means Hocus Focus
/// measured the frame (in arcseconds); 3.3 fills it itself (in pixels).
pub fn metadata_has_fwhm(metadata_json: &str) -> bool {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str(metadata_json) else {
        return false;
    };
    let fwhm = map
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("FWHM"))
        .map(|(_, value)| value);
    match fwhm {
        Some(serde_json::Value::Number(number)) => number.as_f64(),
        Some(serde_json::Value::String(text)) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
    .is_some_and(|fwhm| fwhm.is_finite() && fwhm > 0.0)
}

/// How to measure one frame of a rig with `rig`'s choices.
///
/// Auto picks Hocus Focus when N.I.N.A. recorded an FWHM and the frame was
/// written by N.I.N.A. 3.2 or earlier, whose built-in detector records
/// none. Otherwise, and with Nina chosen, N.I.N.A.'s detector measures as
/// the frame's release does: 3.1, 3.2, or 3.3 for 3.3 and later. A frame
/// from other software, or of an unknown release, is measured as 3.2.
pub fn resolve(
    rig: &RigStarDetection,
    facts: &FrameFacts,
    metadata_fwhm: bool,
) -> DetectionSettings {
    let release = nina_release(facts.software.as_deref());
    let hocus_focus = match rig.detector {
        DetectorChoice::Auto => metadata_fwhm && release.is_some_and(|release| release <= (3, 2)),
        DetectorChoice::Nina => false,
        DetectorChoice::HocusFocus => true,
    };
    if hocus_focus {
        DetectionSettings::HocusFocus {
            version: rig.hocus_focus_version,
        }
    } else {
        DetectionSettings::Nina {
            measurement: release.map(NinaMeasurement::of_release).unwrap_or_default(),
            sensitivity: rig.sensitivity,
            noise_reduction: rig.noise_reduction,
        }
    }
}

/// The registry's star detection block: a default row, and each rig's own.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StarDetectionSettings {
    /// What a rig without a row of its own uses.
    #[serde(default)]
    pub default: RigStarDetection,
    /// Each rig's own choices, by the calibration library's rig UUID.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rigs: BTreeMap<String, RigStarDetectionEntry>,
}

/// One rig's row in the registry.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RigStarDetectionEntry {
    /// What the rig was called when its row was saved, to name it when no
    /// open catalog holds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(flatten)]
    pub settings: RigStarDetection,
}

impl StarDetectionSettings {
    /// The choices for a rig: its own row, else the default row.
    pub fn for_rig(&self, rig_uuid: Option<&str>) -> &RigStarDetection {
        rig_uuid
            .and_then(|uuid| self.rigs.get(uuid))
            .map_or(&self.default, |entry| &entry.settings)
    }

    /// [`resolve`] with the frame's rig's choices.
    pub fn resolve(&self, facts: &FrameFacts, metadata_fwhm: bool) -> DetectionSettings {
        resolve(
            self.for_rig(facts.rig_uuid.as_deref()),
            facts,
            metadata_fwhm,
        )
    }
}

static CURRENT: LazyLock<RwLock<Arc<StarDetectionSettings>>> =
    LazyLock::new(|| RwLock::new(Arc::new(StarDetectionSettings::default())));

#[cfg(test)]
thread_local! {
    /// Settings one test put in force for itself, seen only on its own
    /// thread, so the tests running beside it keep the defaults.
    static TEST_SETTINGS: std::cell::RefCell<Option<Arc<StarDetectionSettings>>> =
        const { std::cell::RefCell::new(None) };
}

/// Put settings in force for the calling test's thread alone, or `None` to
/// go back to the process's.
#[cfg(test)]
pub(crate) fn set_test_settings(settings: Option<StarDetectionSettings>) {
    TEST_SETTINGS.with(|slot| *slot.borrow_mut() = settings.map(Arc::new));
}

/// Put the registry's settings in force for every scan and check after this.
pub fn configure(settings: Option<&StarDetectionSettings>) {
    *CURRENT.write().unwrap_or_else(PoisonError::into_inner) =
        Arc::new(settings.cloned().unwrap_or_default());
}

/// The settings in force.
pub fn current() -> Arc<StarDetectionSettings> {
    #[cfg(test)]
    if let Some(settings) = TEST_SETTINGS.with(|slot| slot.borrow().clone()) {
        return settings;
    }
    Arc::clone(&CURRENT.read().unwrap_or_else(PoisonError::into_inner))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(software: &str) -> FrameFacts {
        FrameFacts {
            rig_uuid: Some("rig".into()),
            software: Some(software.into()),
        }
    }

    #[test]
    fn swcreate_gives_the_nina_release() {
        assert_eq!(
            nina_release(Some("N.I.N.A. 3.2.0.9001 (x64)")),
            Some((3, 2))
        );
        assert_eq!(
            nina_release(Some("N.I.N.A. 3.3.0.1022-nightly")),
            Some((3, 3))
        );
        assert_eq!(nina_release(Some("N.I.N.A. 2.3.0.2084")), Some((2, 3)));
        assert_eq!(nina_release(Some("PixInsight 1.9.3")), None);
        assert_eq!(nina_release(Some("N.I.N.A.")), None);
        assert_eq!(nina_release(None), None);
    }

    #[test]
    fn a_numeric_fwhm_is_hocus_focus_and_nan_is_not() {
        assert!(metadata_has_fwhm(r#"{"FWHM": 2.7, "HFR": 1.9}"#));
        assert!(metadata_has_fwhm(r#"{"FWHM": "3.1"}"#));
        assert!(!metadata_has_fwhm(r#"{"FWHM": "NaN", "HFR": 1.9}"#));
        assert!(!metadata_has_fwhm(r#"{"FWHM": null}"#));
        assert!(!metadata_has_fwhm(r#"{"FWHM": 0}"#));
        assert!(!metadata_has_fwhm(r#"{"HFR": 1.9}"#));
        assert!(!metadata_has_fwhm("not json"));
    }

    #[test]
    fn auto_follows_the_frames_metadata_and_release() {
        let auto = RigStarDetection::default();
        assert_eq!(
            resolve(&auto, &facts("N.I.N.A. 3.2.0.9001 (x64)"), true),
            DetectionSettings::HocusFocus {
                version: HocusFocusVersion::V4
            }
        );
        let nina = |measurement| DetectionSettings::Nina {
            measurement,
            sensitivity: Sensitivity::High,
            noise_reduction: NoiseReduction::None,
        };
        assert_eq!(
            resolve(&auto, &facts("N.I.N.A. 3.1.2.9001"), true),
            DetectionSettings::HocusFocus {
                version: HocusFocusVersion::V4
            }
        );
        assert_eq!(
            resolve(&auto, &facts("N.I.N.A. 3.2.0.9001 (x64)"), false),
            nina(NinaMeasurement::Nina32)
        );
        assert_eq!(
            resolve(&auto, &facts("N.I.N.A. 3.1.2.9001"), false),
            nina(NinaMeasurement::Nina31)
        );
        // 3.3 fills FWHM itself: a number there is N.I.N.A.'s own.
        assert_eq!(
            resolve(&auto, &facts("N.I.N.A. 3.3.0.1022"), true),
            nina(NinaMeasurement::Nina33)
        );
        assert_eq!(
            resolve(&auto, &facts("N.I.N.A. 4.0.0.1"), false),
            nina(NinaMeasurement::Nina33)
        );
        // Unknown software measures as 3.2, and its FWHM proves nothing.
        assert_eq!(
            resolve(&auto, &facts("PixInsight 1.9.3"), true),
            nina(NinaMeasurement::Nina32)
        );
        assert_eq!(
            resolve(&auto, &FrameFacts::default(), false),
            nina(NinaMeasurement::Nina32)
        );
    }

    #[test]
    fn a_forced_detector_ignores_the_metadata() {
        let forced_nina = RigStarDetection {
            detector: DetectorChoice::Nina,
            sensitivity: Sensitivity::Highest,
            noise_reduction: NoiseReduction::Median,
            ..Default::default()
        };
        assert_eq!(
            resolve(&forced_nina, &facts("N.I.N.A. 3.2.0.9001 (x64)"), true),
            DetectionSettings::Nina {
                measurement: NinaMeasurement::Nina32,
                sensitivity: Sensitivity::Highest,
                noise_reduction: NoiseReduction::Median,
            }
        );
        let forced_hocus_focus = RigStarDetection {
            detector: DetectorChoice::HocusFocus,
            hocus_focus_version: HocusFocusVersion::V3,
            ..Default::default()
        };
        assert_eq!(
            resolve(&forced_hocus_focus, &facts("N.I.N.A. 3.3.0.1022"), false),
            DetectionSettings::HocusFocus {
                version: HocusFocusVersion::V3
            }
        );
    }

    #[test]
    fn a_rig_row_beats_the_default_row() {
        let mut settings = StarDetectionSettings::default();
        settings.rigs.insert(
            "rig".into(),
            RigStarDetectionEntry {
                name: Some("C925".into()),
                settings: RigStarDetection {
                    sensitivity: Sensitivity::Normal,
                    ..Default::default()
                },
            },
        );
        let frame = facts("N.I.N.A. 3.2.0.9001 (x64)");
        assert_eq!(
            settings.resolve(&frame, false),
            DetectionSettings::Nina {
                measurement: NinaMeasurement::Nina32,
                sensitivity: Sensitivity::Normal,
                noise_reduction: NoiseReduction::None,
            }
        );
        let other = FrameFacts {
            rig_uuid: Some("other".into()),
            ..frame
        };
        assert_eq!(
            settings.resolve(&other, false),
            DetectionSettings::Nina {
                measurement: NinaMeasurement::Nina32,
                sensitivity: Sensitivity::High,
                noise_reduction: NoiseReduction::None,
            }
        );
    }

    #[test]
    fn settings_round_trip_and_fill_left_out_fields() {
        let mut settings = StarDetectionSettings {
            default: RigStarDetection {
                detector: DetectorChoice::Nina,
                ..Default::default()
            },
            ..Default::default()
        };
        settings.rigs.insert(
            "5a1e0000-0000-5000-8000-000000000001".into(),
            RigStarDetectionEntry {
                name: Some("Askar".into()),
                settings: RigStarDetection {
                    detector: DetectorChoice::HocusFocus,
                    hocus_focus_version: HocusFocusVersion::V3,
                    ..Default::default()
                },
            },
        );
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["default"]["detector"], "nina");
        let rig = &json["rigs"]["5a1e0000-0000-5000-8000-000000000001"];
        assert_eq!(rig["name"], "Askar");
        assert_eq!(rig["detector"], "hocus_focus");
        assert_eq!(rig["hocus_focus_version"], "v3");
        assert_eq!(
            serde_json::from_value::<StarDetectionSettings>(json).unwrap(),
            settings
        );
        let sparse: StarDetectionSettings =
            serde_json::from_str(r#"{"default": {"sensitivity": "normal"}}"#).unwrap();
        assert_eq!(sparse.default.sensitivity, Sensitivity::Normal);
        assert_eq!(sparse.default.detector, DetectorChoice::Auto);
        assert_eq!(sparse.default.hocus_focus_version, HocusFocusVersion::V4);
    }

    #[test]
    fn resolved_settings_serialize_with_their_detector() {
        let settings = DetectionSettings::Nina {
            measurement: NinaMeasurement::Nina33,
            sensitivity: Sensitivity::High,
            noise_reduction: NoiseReduction::None,
        };
        let json = serde_json::to_value(settings).unwrap();
        assert_eq!(json["detector"], "nina");
        assert_eq!(json["measurement"], "3.3");
        assert_eq!(
            serde_json::from_value::<DetectionSettings>(json).unwrap(),
            settings
        );
    }

    #[test]
    fn parameters_follow_the_resolved_settings() {
        let nina = DetectionSettings::default();
        let params = nina.nina_params(Some(1.2)).unwrap();
        assert_eq!(params.measurement, MeasurementVersion::Nina32);
        assert_eq!(params.image_scale_arcsec, Some(1.2));
        assert!(nina.hocus_focus_params(None).is_none());
        let v4 = DetectionSettings::HocusFocus {
            version: HocusFocusVersion::V4,
        };
        assert_eq!(
            v4.hocus_focus_params(None).unwrap().version,
            crate::hocus_focus_star_detection::HocusFocusVersion::V4
        );
        let v3 = DetectionSettings::HocusFocus {
            version: HocusFocusVersion::V3,
        };
        assert_eq!(
            v3.hocus_focus_params(Some(3.5)).unwrap().version,
            crate::hocus_focus_star_detection::HocusFocusVersion::V3
        );
    }
}
