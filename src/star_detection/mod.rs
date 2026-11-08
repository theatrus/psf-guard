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
//! A N.I.N.A. profile may also have recorded settings ([`ProfileRecord`]):
//! full snapshots of the profile's star detection settings, sent by the PSF
//! Guard plugin as it captures or imported from a `.profile` file, each from
//! a date on. A frame taken with the profile is measured with the newest
//! record in effect when it was captured. A manual row for the profile or
//! the frame's rig beats the records.
//!
//! [`StarDetectionSettings::resolve`] turns all this and a frame's own facts
//! (its header's SWCREATE, its profile and capture time, and whether
//! N.I.N.A. recorded an FWHM) into the [`DetectionSettings`] one frame is
//! measured with. The scan stores them beside each measurement, and a
//! measurement is current only while the settings resolved today equal the
//! stored ones, so a changed choice or a new record measures the affected
//! frames again on the next scan.

pub mod hocus_focus;
pub mod nina_profile;

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::hocus_focus_star_detection::{classify_pixel_scale, HocusFocusParams, TelescopeClass};
use crate::nina_star_detection::{MeasurementVersion, StarDetectionParams};
pub use hocus_focus::HocusFocusKnobs;
pub use nina_profile::{ProfileSettings, StarDetectorSelection};

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
/// of the registry takes its default, and one this build does not know is
/// ignored: the registry must load whatever a later release wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
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

/// The profile settings N.I.N.A.'s built-in detector takes beyond its
/// sensitivity and noise reduction: the autofocus crop it passes to every
/// detection, and the autostretch it finds edges on.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NinaProfileOptions {
    pub inner_crop_ratio: f64,
    pub outer_crop_ratio: f64,
    pub stretch_factor: f64,
    pub black_clipping: f64,
}

impl NinaProfileOptions {
    /// The options a profile gives, or `None` when they are N.I.N.A.'s
    /// defaults, so a record that only repeats them measures as no record.
    fn of_profile(settings: &ProfileSettings) -> Option<Self> {
        let options = Self {
            inner_crop_ratio: settings.autofocus_inner_crop_ratio,
            outer_crop_ratio: settings.autofocus_outer_crop_ratio,
            stretch_factor: settings.auto_stretch_factor,
            black_clipping: settings.black_clipping,
        };
        let defaults = Self {
            inner_crop_ratio: 1.0,
            outer_crop_ratio: 1.0,
            stretch_factor: 0.2,
            black_clipping: -2.8,
        };
        (options != defaults).then_some(options)
    }
}

/// How one frame is measured: what [`resolve`] decided for it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "detector", rename_all = "snake_case")]
pub enum DetectionSettings {
    /// N.I.N.A.'s built-in detector.
    Nina {
        measurement: NinaMeasurement,
        sensitivity: Sensitivity,
        noise_reduction: NoiseReduction,
        /// The profile's crop and autostretch; `None` for N.I.N.A.'s
        /// defaults.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<NinaProfileOptions>,
    },
    /// Hocus Focus as the frame's primary detector.
    HocusFocus {
        version: HocusFocusVersion,
        /// Every option a recorded profile gives; `None` uses the version's
        /// Simple preset for the frame's image scale.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        knobs: Option<HocusFocusKnobs>,
    },
}

impl Default for DetectionSettings {
    fn default() -> Self {
        resolve(
            &RigStarDetection::default(),
            &FrameFacts::default(),
            FwhmEvidence::Absent,
        )
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
                profile,
            } => {
                format!(
                "N.I.N.A. {} measurement, {sensitivity:?} sensitivity, {noise_reduction:?} noise \
                 reduction{}",
                serde_json::to_value(measurement)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_default(),
                if profile.is_some() { ", profile options" } else { "" }
            )
            }
            Self::HocusFocus { version, knobs } => format!(
                "Hocus Focus {version:?}{}",
                if knobs.is_some() {
                    ", profile options"
                } else {
                    ""
                }
            ),
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
            profile,
        } = *self
        else {
            return None;
        };
        use crate::nina_star_detection as nina;
        // N.I.N.A. crops only when a ratio is below 1 (RenderedImage.DetectStars).
        let (inner, outer) = profile
            .map(|profile| (profile.inner_crop_ratio, profile.outer_crop_ratio))
            .unwrap_or((1.0, 1.0));
        Some(StarDetectionParams {
            use_roi: inner < 1.0 || outer < 1.0,
            inner_crop_ratio: inner,
            outer_crop_ratio: outer,
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
        })
    }

    /// The autostretch N.I.N.A.'s detector finds edges on.
    pub fn stretch_params(&self) -> seiza_stretch::StretchParams {
        match self {
            Self::Nina {
                profile: Some(profile),
                ..
            } => seiza_stretch::StretchParams {
                target_median: profile.stretch_factor,
                shadows_clip: profile.black_clipping,
            },
            _ => seiza_stretch::StretchParams::default(),
        }
    }

    /// Hocus Focus parameters, or `None` for N.I.N.A.'s detector: a
    /// recorded profile's options, else the version's Simple-mode preset for
    /// the rig class the image scale gives.
    pub fn hocus_focus_params(&self, image_scale_arcsec: Option<f64>) -> Option<HocusFocusParams> {
        let Self::HocusFocus { version, knobs } = *self else {
            return None;
        };
        if let Some(knobs) = knobs {
            return Some(knobs.params(version));
        }
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
    /// The N.I.N.A. profile the catalog row records, normalized by
    /// [`profile_key`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    /// When the catalog row says the frame was taken, in epoch seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<i64>,
}

impl FrameFacts {
    /// The header's facts, with the catalog row's profile and capture time.
    pub fn of_frame(
        meta: &crate::commands::import::headers::FrameMeta,
        profile_id: Option<&str>,
        captured_at: Option<i64>,
    ) -> Self {
        let mut facts = Self {
            profile_id: profile_id.and_then(profile_key),
            captured_at,
            ..Self::default()
        };
        if meta.readable {
            facts.rig_uuid = Some(crate::calibration::rules::light_rig_uuid(meta));
            facts.software = meta.software.clone();
        }
        facts
    }
}

/// A N.I.N.A. profile id as the registry keys it: a GUID in lower-case
/// hyphenated form. `None` for an empty id.
pub fn profile_key(profile_id: &str) -> Option<String> {
    let trimmed = profile_id.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(
        uuid::Uuid::parse_str(trimmed)
            .map(|id| id.hyphenated().to_string())
            .unwrap_or_else(|_| trimmed.to_lowercase()),
    )
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

/// What the catalog row's FWHM says. N.I.N.A.'s built-in detector through
/// 3.2 leaves FWHM as `"NaN"`, so on those releases a number means Hocus
/// Focus measured the frame (in arcseconds) and `NaN` means it did not;
/// 3.3 fills FWHM itself (in pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FwhmEvidence {
    /// A positive number.
    Measured,
    /// `NaN`, zero or another non-number.
    NotMeasured,
    /// No FWHM at all, as in an imported catalog's rows.
    Absent,
}

impl FwhmEvidence {
    pub fn of_metadata(metadata_json: &str) -> Self {
        serde_json::from_str::<serde_json::Value>(metadata_json)
            .map_or(Self::Absent, |value| Self::of_value(&value))
    }

    /// [`Self::of_metadata`] for a row already parsed.
    pub fn of_value(metadata: &serde_json::Value) -> Self {
        let Some(map) = metadata.as_object() else {
            return Self::Absent;
        };
        let fwhm = map
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("FWHM"))
            .map(|(_, value)| value);
        let value = match fwhm {
            None | Some(serde_json::Value::Null) => return Self::Absent,
            Some(serde_json::Value::Number(number)) => number.as_f64(),
            Some(serde_json::Value::String(text)) => text.trim().parse::<f64>().ok(),
            Some(_) => None,
        };
        if value.is_some_and(|fwhm| fwhm.is_finite() && fwhm > 0.0) {
            Self::Measured
        } else {
            Self::NotMeasured
        }
    }
}

/// Whether N.I.N.A. recorded an FWHM for the frame.
pub fn metadata_has_fwhm(metadata_json: &str) -> bool {
    FwhmEvidence::of_metadata(metadata_json) == FwhmEvidence::Measured
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
    fwhm: FwhmEvidence,
) -> DetectionSettings {
    let release = nina_release(facts.software.as_deref());
    let hocus_focus = match rig.detector {
        DetectorChoice::Auto => {
            fwhm == FwhmEvidence::Measured && release.is_some_and(|release| release <= (3, 2))
        }
        DetectorChoice::Nina => false,
        DetectorChoice::HocusFocus => true,
    };
    if hocus_focus {
        DetectionSettings::HocusFocus {
            version: rig.hocus_focus_version,
            knobs: None,
        }
    } else {
        DetectionSettings::Nina {
            measurement: release.map(NinaMeasurement::of_release).unwrap_or_default(),
            sensitivity: rig.sensitivity,
            noise_reduction: rig.noise_reduction,
            profile: None,
        }
    }
}

/// How to measure a frame with a recorded profile's settings.
///
/// The profile's selected detector decides, except where the catalog row
/// shows otherwise: on N.I.N.A. 3.2 and earlier a numeric FWHM proves Hocus
/// Focus measured the frame, and `NaN` proves it did not, whatever the
/// profile selects now.
pub fn resolve_record(
    record: &ProfileRecord,
    facts: &FrameFacts,
    fwhm: FwhmEvidence,
) -> DetectionSettings {
    let release = nina_release(facts.software.as_deref());
    let selected = record.settings.star_detector == StarDetectorSelection::HocusFocus;
    let hocus_focus = match (release.is_some_and(|release| release <= (3, 2)), fwhm) {
        (true, FwhmEvidence::Measured) => true,
        (true, FwhmEvidence::NotMeasured) => false,
        _ => selected,
    };
    if hocus_focus {
        let (knobs, _) = HocusFocusKnobs::from_profile(&record.settings);
        DetectionSettings::HocusFocus {
            version: record.hocus_focus_version,
            knobs: Some(knobs),
        }
    } else {
        DetectionSettings::Nina {
            measurement: release.map(NinaMeasurement::of_release).unwrap_or_default(),
            sensitivity: record.settings.star_sensitivity,
            noise_reduction: record.settings.noise_reduction,
            profile: NinaProfileOptions::of_profile(&record.settings),
        }
    }
}

/// Where a recorded profile snapshot came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordSource {
    /// The PSF Guard Sync plugin sent it as it captured.
    #[serde(rename = "plugin:sync", alias = "plugin")]
    SyncPlugin,
    /// The PSF Guard Director plugin sent it as it captured.
    #[serde(rename = "plugin:director")]
    DirectorPlugin,
    /// Someone imported a `.profile` file.
    #[serde(rename = "import")]
    Import,
}

/// One snapshot of a profile's star detection settings, in effect from a
/// date on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileRecord {
    pub id: String,
    /// From when it applies; `None` for every frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_from: Option<DateTime<Utc>>,
    pub source: RecordSource,
    pub received_at: DateTime<Utc>,
    /// The catalog whose plugin sent it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
    /// N.I.N.A.'s version, as the plugin reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nina_version: Option<String>,
    pub hocus_focus_version: HocusFocusVersion,
    pub settings: ProfileSettings,
}

/// A N.I.N.A. profile's star detection: its name, a manual row that beats
/// every record, and its recorded snapshots, oldest first.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ProfileStarDetection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual: Option<RigStarDetection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<ProfileRecord>,
}

impl ProfileStarDetection {
    /// The record in effect at `captured_at` (epoch seconds): the newest
    /// whose start is not after it, the latest received among equals. A
    /// frame taken before every record takes the earliest: a plugin sends
    /// its first snapshot after the frames already in the catalog, which
    /// were most likely taken with the same settings. With no capture time,
    /// the newest record.
    pub fn record_at(&self, captured_at: Option<i64>) -> Option<&ProfileRecord> {
        let captured = captured_at.and_then(|seconds| DateTime::from_timestamp(seconds, 0));
        self.records
            .iter()
            .filter(|record| match (record.effective_from, captured) {
                (None, _) | (_, None) => true,
                (Some(from), Some(captured)) => from <= captured,
            })
            .max_by_key(|record| (record.effective_from, record.received_at))
            .or_else(|| self.earliest())
    }

    /// The record that starts first, the latest received among equals.
    pub fn earliest(&self) -> Option<&ProfileRecord> {
        self.records.iter().min_by(|left, right| {
            left.effective_from
                .cmp(&right.effective_from)
                .then_with(|| right.received_at.cmp(&left.received_at))
        })
    }

    /// The newest record by start date, as the panel shows it.
    pub fn latest(&self) -> Option<&ProfileRecord> {
        self.record_at(None)
    }
}

/// The registry's star detection block: a default row, each rig's own, and
/// each N.I.N.A. profile's.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StarDetectionSettings {
    /// What a rig without a row of its own uses.
    #[serde(default)]
    pub default: RigStarDetection,
    /// Each rig's own choices, by the calibration library's rig UUID.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rigs: BTreeMap<String, RigStarDetectionEntry>,
    /// Each N.I.N.A. profile's manual row and recorded settings, by
    /// [`profile_key`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, ProfileStarDetection>,
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

    /// How to measure a frame: its profile's manual row, else its rig's,
    /// else its profile's record in effect when it was captured, else the
    /// default row.
    pub fn resolve(&self, facts: &FrameFacts, fwhm: FwhmEvidence) -> DetectionSettings {
        let profile = facts
            .profile_id
            .as_deref()
            .and_then(|id| self.profiles.get(id));
        if let Some(manual) = profile.and_then(|profile| profile.manual.as_ref()) {
            return resolve(manual, facts, fwhm);
        }
        if let Some(rig) = facts
            .rig_uuid
            .as_deref()
            .and_then(|uuid| self.rigs.get(uuid))
        {
            return resolve(&rig.settings, facts, fwhm);
        }
        if let Some(record) = profile.and_then(|profile| profile.record_at(facts.captured_at)) {
            return resolve_record(record, facts, fwhm);
        }
        resolve(&self.default, facts, fwhm)
    }

    /// The N.I.N.A. profile a plugin paired with `catalog_id` last sent
    /// settings from: the profile that catalog's rig captures in. `None`
    /// until a plugin sends one. A profile belongs to the catalog of its
    /// newest plugin record, so one paired again elsewhere stops counting
    /// here. With two profiles still paired to one catalog, the newer record
    /// wins.
    pub fn plugin_profile(&self, catalog_id: &str) -> Option<&str> {
        self.profiles
            .iter()
            .filter_map(|(id, profile)| {
                profile
                    .records
                    .iter()
                    .filter(|record| record.source != RecordSource::Import)
                    .max_by_key(|record| record.received_at)
                    .filter(|newest| newest.catalog_id.as_deref() == Some(catalog_id))
                    .map(|newest| (id, newest.received_at))
            })
            .max_by_key(|(_, received_at)| *received_at)
            .map(|(id, _)| id.as_str())
    }
}

/// Read the registry's `star_detection` block, or none with a warning when
/// it does not parse, as a later release's value or a hand edit may not.
/// The rest of the registry, its databases above all, must still load.
pub fn deserialize_lenient<'de, D>(
    deserializer: D,
) -> Result<Option<StarDetectionSettings>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| {
        serde_json::from_value(value)
            .inspect_err(|error| {
                tracing::warn!(
                    "Ignoring the registry's star_detection settings, which this build cannot \
                     read ({error}); every rig uses N.I.N.A.'s defaults until they are saved again"
                )
            })
            .ok()
    }))
}

/// A stored measurement's detection settings, or none when this build cannot
/// read them: the measurement is then outdated and measured again, and the
/// rest of the cache still loads.
pub fn deserialize_stored<'de, D>(deserializer: D) -> Result<Option<DetectionSettings>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
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
            ..Default::default()
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
            resolve(
                &auto,
                &facts("N.I.N.A. 3.2.0.9001 (x64)"),
                FwhmEvidence::Measured
            ),
            DetectionSettings::HocusFocus {
                version: HocusFocusVersion::V4,
                knobs: None,
            }
        );
        let nina = |measurement| DetectionSettings::Nina {
            measurement,
            sensitivity: Sensitivity::High,
            noise_reduction: NoiseReduction::None,
            profile: None,
        };
        assert_eq!(
            resolve(&auto, &facts("N.I.N.A. 3.1.2.9001"), FwhmEvidence::Measured),
            DetectionSettings::HocusFocus {
                version: HocusFocusVersion::V4,
                knobs: None,
            }
        );
        assert_eq!(
            resolve(
                &auto,
                &facts("N.I.N.A. 3.2.0.9001 (x64)"),
                FwhmEvidence::Absent
            ),
            nina(NinaMeasurement::Nina32)
        );
        assert_eq!(
            resolve(&auto, &facts("N.I.N.A. 3.1.2.9001"), FwhmEvidence::Absent),
            nina(NinaMeasurement::Nina31)
        );
        // 3.3 fills FWHM itself: a number there is N.I.N.A.'s own.
        assert_eq!(
            resolve(&auto, &facts("N.I.N.A. 3.3.0.1022"), FwhmEvidence::Measured),
            nina(NinaMeasurement::Nina33)
        );
        assert_eq!(
            resolve(&auto, &facts("N.I.N.A. 4.0.0.1"), FwhmEvidence::Absent),
            nina(NinaMeasurement::Nina33)
        );
        // Unknown software measures as 3.2, and its FWHM proves nothing.
        assert_eq!(
            resolve(&auto, &facts("PixInsight 1.9.3"), FwhmEvidence::Measured),
            nina(NinaMeasurement::Nina32)
        );
        assert_eq!(
            resolve(&auto, &FrameFacts::default(), FwhmEvidence::Absent),
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
            resolve(
                &forced_nina,
                &facts("N.I.N.A. 3.2.0.9001 (x64)"),
                FwhmEvidence::Measured
            ),
            DetectionSettings::Nina {
                measurement: NinaMeasurement::Nina32,
                sensitivity: Sensitivity::Highest,
                noise_reduction: NoiseReduction::Median,
                profile: None,
            }
        );
        let forced_hocus_focus = RigStarDetection {
            detector: DetectorChoice::HocusFocus,
            hocus_focus_version: HocusFocusVersion::V3,
            ..Default::default()
        };
        assert_eq!(
            resolve(
                &forced_hocus_focus,
                &facts("N.I.N.A. 3.3.0.1022"),
                FwhmEvidence::Absent
            ),
            DetectionSettings::HocusFocus {
                version: HocusFocusVersion::V3,
                knobs: None,
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
            settings.resolve(&frame, FwhmEvidence::Absent),
            DetectionSettings::Nina {
                measurement: NinaMeasurement::Nina32,
                sensitivity: Sensitivity::Normal,
                noise_reduction: NoiseReduction::None,
                profile: None,
            }
        );
        let other = FrameFacts {
            rig_uuid: Some("other".into()),
            ..frame
        };
        assert_eq!(
            settings.resolve(&other, FwhmEvidence::Absent),
            DetectionSettings::Nina {
                measurement: NinaMeasurement::Nina32,
                sensitivity: Sensitivity::High,
                noise_reduction: NoiseReduction::None,
                profile: None,
            }
        );
    }

    fn record(
        id: &str,
        from: Option<&str>,
        detector: StarDetectorSelection,
        inner_crop: f64,
    ) -> ProfileRecord {
        ProfileRecord {
            id: id.into(),
            effective_from: from.map(|from| from.parse().unwrap()),
            source: RecordSource::Import,
            received_at: "2026-11-02T00:00:00Z".parse().unwrap(),
            catalog_id: None,
            nina_version: None,
            hocus_focus_version: HocusFocusVersion::V4,
            settings: ProfileSettings {
                star_detector: detector,
                autofocus_inner_crop_ratio: inner_crop,
                hocus_focus: [(
                    "Simple_PixelScale".to_string(),
                    serde_json::json!("WideField"),
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            },
        }
    }

    const REDCAT: &str = "46d7a6e3-3ae8-48a9-8d29-598e3647f131";

    fn redcat_frame(captured: &str) -> FrameFacts {
        FrameFacts {
            rig_uuid: Some("rig".into()),
            software: Some("N.I.N.A. 3.2.0.9001 (x64)".into()),
            profile_id: profile_key(&REDCAT.to_uppercase()),
            captured_at: Some(captured.parse::<DateTime<Utc>>().unwrap().timestamp()),
        }
    }

    #[test]
    fn plugin_profile_follows_each_profiles_newest_plugin_record() {
        let sent = |id: &str, source: RecordSource, catalog: &str, at: &str| ProfileRecord {
            source,
            catalog_id: Some(catalog.into()),
            received_at: at.parse().unwrap(),
            ..record(id, None, StarDetectorSelection::Nina, 1.0)
        };
        let profile = |records: Vec<ProfileRecord>| ProfileStarDetection {
            records,
            ..Default::default()
        };
        let settings = StarDetectionSettings {
            profiles: [
                (
                    "aaaaaaaa-0000-4000-8000-000000000001".to_string(),
                    profile(vec![sent(
                        "a1",
                        RecordSource::SyncPlugin,
                        "rig",
                        "2026-11-01T00:00:00Z",
                    )]),
                ),
                (
                    "bbbbbbbb-0000-4000-8000-000000000002".to_string(),
                    profile(vec![
                        sent(
                            "b1",
                            RecordSource::DirectorPlugin,
                            "rig",
                            "2026-11-03T00:00:00Z",
                        ),
                        sent(
                            "b2",
                            RecordSource::SyncPlugin,
                            "other",
                            "2026-11-05T00:00:00Z",
                        ),
                    ]),
                ),
                (
                    "cccccccc-0000-4000-8000-000000000003".to_string(),
                    // An import names no rig, whatever catalog it lists.
                    profile(vec![sent(
                        "c1",
                        RecordSource::Import,
                        "rig",
                        "2026-11-06T00:00:00Z",
                    )]),
                ),
            ]
            .into(),
            ..Default::default()
        };

        // Profile b was paired again with "other", so it no longer counts
        // for "rig".
        assert_eq!(
            settings.plugin_profile("rig"),
            Some("aaaaaaaa-0000-4000-8000-000000000001")
        );
        assert_eq!(
            settings.plugin_profile("other"),
            Some("bbbbbbbb-0000-4000-8000-000000000002")
        );
        assert_eq!(settings.plugin_profile("unpaired"), None);
    }

    fn redcat_history() -> StarDetectionSettings {
        let mut settings = StarDetectionSettings::default();
        settings.profiles.insert(
            REDCAT.into(),
            ProfileStarDetection {
                name: Some("starfront-mini-redcat61".into()),
                manual: None,
                records: vec![
                    record("all", None, StarDetectorSelection::HocusFocus, 1.0),
                    record(
                        "sep9",
                        Some("2026-09-09T00:00:00Z"),
                        StarDetectorSelection::HocusFocus,
                        0.7,
                    ),
                    record(
                        "sep1",
                        Some("2026-09-01T00:00:00Z"),
                        StarDetectorSelection::HocusFocus,
                        0.5,
                    ),
                ],
            },
        );
        settings
    }

    fn crop(detection: DetectionSettings) -> f64 {
        match detection {
            DetectionSettings::HocusFocus {
                knobs: Some(knobs), ..
            } => knobs.autofocus_inner_crop_ratio,
            other => panic!("expected recorded Hocus Focus, got {other:?}"),
        }
    }

    #[test]
    fn a_frame_takes_the_record_in_effect_when_it_was_captured() {
        let settings = redcat_history();
        let measured = FwhmEvidence::Measured;
        let at = |captured| crop(settings.resolve(&redcat_frame(captured), measured));
        assert_eq!(at("2026-08-24T02:49:05Z"), 1.0);
        assert_eq!(at("2026-09-01T00:00:00Z"), 0.5);
        assert_eq!(at("2026-09-08T23:59:59Z"), 0.5);
        assert_eq!(at("2026-09-09T03:00:00Z"), 0.7);
        assert_eq!(at("2026-10-28T06:09:09Z"), 0.7);
        // The same snapshot received later replaces the earlier one.
        let mut later = settings.clone();
        let mut repeat = record(
            "again",
            Some("2026-09-09T00:00:00Z"),
            StarDetectorSelection::HocusFocus,
            0.8,
        );
        repeat.received_at = "2026-11-03T00:00:00Z".parse().unwrap();
        later.profiles.get_mut(REDCAT).unwrap().records.push(repeat);
        assert_eq!(
            crop(later.resolve(&redcat_frame("2026-10-01T00:00:00Z"), measured)),
            0.8
        );
    }

    #[test]
    fn a_frame_before_every_record_takes_the_earliest() {
        let mut settings = redcat_history();
        let profile = settings.profiles.get_mut(REDCAT).unwrap();
        profile
            .records
            .retain(|record| record.effective_from.is_some());
        let measured = FwhmEvidence::Measured;
        // A frame taken before the first record, as every frame already in
        // the catalog is when a plugin sends its first snapshot.
        assert_eq!(
            crop(settings.resolve(&redcat_frame("2026-08-24T02:49:05Z"), measured)),
            0.5
        );
        assert_eq!(
            crop(settings.resolve(&redcat_frame("2026-09-10T00:00:00Z"), measured)),
            0.7
        );
        // A single plugin record dated today covers the whole history.
        let mut today = StarDetectionSettings::default();
        today.profiles.insert(
            REDCAT.into(),
            ProfileStarDetection {
                records: vec![ProfileRecord {
                    source: RecordSource::SyncPlugin,
                    ..record(
                        "today",
                        Some("2026-11-02T00:00:00Z"),
                        StarDetectorSelection::HocusFocus,
                        0.7,
                    )
                }],
                ..Default::default()
            },
        );
        assert_eq!(
            crop(today.resolve(&redcat_frame("2026-08-24T02:49:05Z"), measured)),
            0.7
        );
    }

    #[test]
    fn record_sources_name_the_plugin_that_sent_them() {
        for (source, name) in [
            (RecordSource::SyncPlugin, "plugin:sync"),
            (RecordSource::DirectorPlugin, "plugin:director"),
            (RecordSource::Import, "import"),
        ] {
            assert_eq!(serde_json::to_value(source).unwrap(), name);
            assert_eq!(
                serde_json::from_value::<RecordSource>(name.into()).unwrap(),
                source
            );
        }
        assert_eq!(
            serde_json::from_value::<RecordSource>("plugin".into()).unwrap(),
            RecordSource::SyncPlugin
        );
    }

    #[test]
    fn manual_rows_beat_records_and_the_catalog_beats_the_selected_detector() {
        let mut settings = redcat_history();
        let frame = redcat_frame("2026-10-01T00:00:00Z");
        // NaN on 3.2 proves N.I.N.A.'s detector ran, whatever the profile
        // selects now; the profile's crop and sensitivity still apply.
        match settings.resolve(&frame, FwhmEvidence::NotMeasured) {
            DetectionSettings::Nina {
                profile: Some(profile),
                measurement: NinaMeasurement::Nina32,
                ..
            } => assert_eq!(profile.inner_crop_ratio, 0.7),
            other => panic!("{other:?}"),
        }
        // Without an FWHM to go by, the profile's selection decides.
        assert_eq!(crop(settings.resolve(&frame, FwhmEvidence::Absent)), 0.7);

        settings.rigs.insert(
            "rig".into(),
            RigStarDetectionEntry {
                name: None,
                settings: RigStarDetection {
                    detector: DetectorChoice::Nina,
                    ..Default::default()
                },
            },
        );
        assert!(matches!(
            settings.resolve(&frame, FwhmEvidence::Measured),
            DetectionSettings::Nina { profile: None, .. }
        ));
        settings.profiles.get_mut(REDCAT).unwrap().manual = Some(RigStarDetection {
            detector: DetectorChoice::HocusFocus,
            hocus_focus_version: HocusFocusVersion::V3,
            ..Default::default()
        });
        assert_eq!(
            settings.resolve(&frame, FwhmEvidence::NotMeasured),
            DetectionSettings::HocusFocus {
                version: HocusFocusVersion::V3,
                knobs: None,
            }
        );
    }

    #[test]
    fn a_record_of_ninas_defaults_measures_as_no_record() {
        let mut settings = StarDetectionSettings::default();
        settings.profiles.insert(
            REDCAT.into(),
            ProfileStarDetection {
                records: vec![ProfileRecord {
                    settings: ProfileSettings::default(),
                    ..record("plain", None, StarDetectorSelection::Nina, 1.0)
                }],
                ..Default::default()
            },
        );
        let frame = redcat_frame("2026-10-01T00:00:00Z");
        assert_eq!(
            settings.resolve(&frame, FwhmEvidence::Absent),
            StarDetectionSettings::default().resolve(&frame, FwhmEvidence::Absent)
        );
        // A frame of another profile, or none, ignores the record.
        let other = FrameFacts {
            profile_id: None,
            ..redcat_history_frame()
        };
        assert_eq!(
            redcat_history().resolve(&other, FwhmEvidence::Measured),
            DetectionSettings::HocusFocus {
                version: HocusFocusVersion::V4,
                knobs: None,
            }
        );
    }

    fn redcat_history_frame() -> FrameFacts {
        redcat_frame("2026-10-01T00:00:00Z")
    }

    #[test]
    fn crop_and_stretch_reach_ninas_detector() {
        let detection = DetectionSettings::Nina {
            measurement: NinaMeasurement::Nina32,
            sensitivity: Sensitivity::High,
            noise_reduction: NoiseReduction::None,
            profile: Some(NinaProfileOptions {
                inner_crop_ratio: 0.5,
                outer_crop_ratio: 1.0,
                stretch_factor: 0.15,
                black_clipping: -2.5,
            }),
        };
        let params = detection.nina_params(Some(1.0)).unwrap();
        assert!(params.use_roi);
        assert_eq!(params.inner_crop_ratio, 0.5);
        assert_eq!(params.outer_crop_ratio, 1.0);
        assert_eq!(detection.stretch_params().target_median, 0.15);
        assert!(
            !DetectionSettings::default()
                .nina_params(None)
                .unwrap()
                .use_roi
        );
        let json = serde_json::to_value(detection).unwrap();
        assert_eq!(
            serde_json::from_value::<DetectionSettings>(json).unwrap(),
            detection
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
            profile: None,
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
            knobs: None,
        };
        assert_eq!(
            v4.hocus_focus_params(None).unwrap().version,
            crate::hocus_focus_star_detection::HocusFocusVersion::V4
        );
        let v3 = DetectionSettings::HocusFocus {
            version: HocusFocusVersion::V3,
            knobs: None,
        };
        assert_eq!(
            v3.hocus_focus_params(Some(3.5)).unwrap().version,
            crate::hocus_focus_star_detection::HocusFocusVersion::V3
        );
    }
}
