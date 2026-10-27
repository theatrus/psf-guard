//! The calibration checks a person can tune, for the whole server and for
//! one rig.
//!
//! Matching, the light checks and the frame grades each read their limits
//! from a [`Rules`]. A value comes from the rig's own settings when it has
//! one, then the server's, then the built-in default. The settings live in
//! the database registry ([`crate::db_registry::CalibrationSettings`]) as
//! maps of key to value: a key a newer build wrote survives a save here,
//! and a value this build cannot read falls through to the next layer.
//!
//! [`fields`] lists every setting with its range and default. The settings
//! API sends it to the panel, validates edits against it, and the defaults
//! come from the code that used to hard-code them, so they cannot drift.
//!
//! A rig is the calibration library's rig ([`super::rig_signature`]): its
//! telescope, camera, frame size and binning. The UUID is a hash of those,
//! so one rig has the same key in every catalog, and a light's rig is read
//! off its own header.

use super::{DarkChoice, ExternalMasterPolicy};
use crate::commands::import::headers::FrameMeta;
use seiza_calibration::{BiasScreening, DarkScreening, FlatScreening, Limits, MatchTolerances};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, RwLock};

/// The widest dark temperature tolerance, in °C. Stacks check each light
/// with its rig's own tolerances, so this is a sanity bound: a dark much
/// further off carries a different dark current.
pub const MAX_DARK_TEMPERATURE_C: f64 = 20.0;

/// One layer of settings: key to value, as the registry keeps it.
pub type RuleValues = BTreeMap<String, Value>;

/// Flats further than this from a light are worth a look, though they are
/// within the age limit that makes a light one that cannot be calibrated.
pub const DEFAULT_FLAT_CHECK_AGE_DAYS: f64 = 30.0;

/// A dark or bias master built from fewer frames than this puts its own
/// read noise into every light.
pub const DEFAULT_THIN_MASTER_FRAMES: usize = 5;

/// Dust that splits two nights' flats, percent of the sky background. Real
/// nights with nothing moved differ by at most 0.46% (a filter reflection
/// that changes with the dawn sky); new dust on one rig measured 1.6%.
pub const DEFAULT_FLAT_SPLIT_DUST_PCT: f64 = 1.0;

/// A broad difference that splits two nights' flats, percent of the sky
/// background: the per-frame light-pattern suspect limit within a session.
pub const DEFAULT_FLAT_SPLIT_PATTERN_PCT: f64 = 2.0;

/// What a light check does when it finds something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckMode {
    /// At the check's own level: poor where the light cannot be calibrated
    /// properly, check where it is only worth a look.
    #[default]
    Flag,
    /// At most "check": shown, but never makes a light one that cannot be
    /// calibrated, so stacks keep it and it is never offered for rejection.
    Advisory,
    /// Not checked.
    Off,
}

impl CheckMode {
    fn parse(value: &Value) -> Option<Self> {
        match value.as_str()? {
            "flag" => Some(Self::Flag),
            "advisory" => Some(Self::Advisory),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// The light checks, each switchable on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Check {
    NoLibrary,
    NoFlat,
    NoBiasOrDark,
    StaleFlats,
    ThinDark,
    ThinBias,
    FlatSpots,
    OldFlats,
    MixedFlats,
    SuspectFrames,
    ShortAfterKeepOut,
    FlatsDiffer,
}

impl Check {
    pub const ALL: [Check; 12] = [
        Check::NoLibrary,
        Check::NoFlat,
        Check::NoBiasOrDark,
        Check::StaleFlats,
        Check::ThinDark,
        Check::ThinBias,
        Check::FlatSpots,
        Check::OldFlats,
        Check::MixedFlats,
        Check::SuspectFrames,
        Check::ShortAfterKeepOut,
        Check::FlatsDiffer,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Check::NoLibrary => "check_no_library",
            Check::NoFlat => "check_no_flat",
            Check::NoBiasOrDark => "check_no_bias_or_dark",
            Check::StaleFlats => "check_stale_flats",
            Check::ThinDark => "check_thin_dark",
            Check::ThinBias => "check_thin_bias",
            Check::FlatSpots => "check_flat_spots",
            Check::OldFlats => "check_old_flats",
            Check::MixedFlats => "check_mixed_flats",
            Check::SuspectFrames => "check_suspect_frames",
            Check::ShortAfterKeepOut => "check_short_after_keep_out",
            Check::FlatsDiffer => "check_flats_differ",
        }
    }

    fn index(self) -> usize {
        Check::ALL
            .iter()
            .position(|check| *check == self)
            .expect("every check is listed")
    }
}

/// A frame-grade check: which measurement, for which kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GradeCheck {
    BiasLevel,
    BiasNoise,
    BiasBanding,
    DarkLevel,
    DarkNoise,
    DarkHotPixels,
    DarkBanding,
    FlatClipped,
    FlatBright,
    FlatDim,
    FlatIllumination,
    FlatSpots,
    FlatChangingSpots,
    FlatCarriedSpots,
}

impl GradeCheck {
    pub const ALL: [GradeCheck; 14] = [
        GradeCheck::BiasLevel,
        GradeCheck::BiasNoise,
        GradeCheck::BiasBanding,
        GradeCheck::DarkLevel,
        GradeCheck::DarkNoise,
        GradeCheck::DarkHotPixels,
        GradeCheck::DarkBanding,
        GradeCheck::FlatClipped,
        GradeCheck::FlatBright,
        GradeCheck::FlatDim,
        GradeCheck::FlatIllumination,
        GradeCheck::FlatSpots,
        GradeCheck::FlatChangingSpots,
        GradeCheck::FlatCarriedSpots,
    ];

    pub fn key(self) -> &'static str {
        match self {
            GradeCheck::BiasLevel => "grade_bias_level",
            GradeCheck::BiasNoise => "grade_bias_noise",
            GradeCheck::BiasBanding => "grade_bias_banding",
            GradeCheck::DarkLevel => "grade_dark_level",
            GradeCheck::DarkNoise => "grade_dark_noise",
            GradeCheck::DarkHotPixels => "grade_dark_hot_pixels",
            GradeCheck::DarkBanding => "grade_dark_banding",
            GradeCheck::FlatClipped => "grade_flat_clipped",
            GradeCheck::FlatBright => "grade_flat_bright",
            GradeCheck::FlatDim => "grade_flat_dim",
            GradeCheck::FlatIllumination => "grade_flat_illumination",
            GradeCheck::FlatSpots => "grade_flat_spots",
            GradeCheck::FlatChangingSpots => "grade_flat_changing_spots",
            GradeCheck::FlatCarriedSpots => "grade_flat_carried_spots",
        }
    }

    /// Whether a lower value is the worse one.
    fn lower_is_worse(self) -> bool {
        self == GradeCheck::FlatDim
    }
}

pub use seiza_calibration::library::{
    ChoiceOption, Effect, FieldKind, FieldSpec, Group, RuleSource,
};

/// Where a rig's lights split into nights in Images, Sequence and the
/// calibration reports: at its local noon, as its calibration frames do,
/// or at the catalog's quiet hours, one split for every rig.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightNightSplit {
    #[default]
    Rig,
    Catalog,
}

impl LightNightSplit {
    /// A view's override from the link: `rig` or `catalog`.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "rig" => Some(Self::Rig),
            "catalog" => Some(Self::Catalog),
            _ => None,
        }
    }
}

/// How a rig's flats of different nights may share a master.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FlatGrouping {
    /// Nights mix in one master only when their flats measure alike, and a
    /// light takes flats from its own side of every measured change.
    #[default]
    Measured,
    /// By time alone: the flat session around the nearest flat.
    Time,
}

fn night_offset_seconds(value: &Value) -> Option<i64> {
    let hours: f64 = value.as_str()?.parse().ok()?;
    Some((hours * 3600.0).round() as i64)
}

/// Settings definitions are owned by Seiza, including defaults and ranges.
pub fn fields() -> &'static [FieldSpec] {
    static FIELDS: LazyLock<Vec<FieldSpec>> = LazyLock::new(|| {
        seiza_calibration::library::evaluate(Default::default())
            .expect("empty calibration library")
            .settings_schema
    });
    &FIELDS
}

pub fn field(key: &str) -> Option<&'static FieldSpec> {
    fields().iter().find(|field| field.key == key)
}

fn finite(value: &Value) -> Option<f64> {
    value.as_f64().filter(|value| value.is_finite())
}

/// Two stored values that mean the same: `2` and `2.0` are one tolerance.
pub fn equivalent(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        (Value::Object(left), Value::Object(right)) => {
            ["suspect", "poor"].iter().all(|step| {
                let pick = |object: &serde_json::Map<String, Value>| {
                    object.get(*step).cloned().unwrap_or(Value::Null)
                };
                equivalent(&pick(left), &pick(right))
            }) && left.len() == right.len()
        }
        _ => left == right,
    }
}

/// A value as text that is the same for equivalent values, for a key.
fn canonical(value: &Value) -> String {
    match value {
        Value::Number(number) => number
            .as_f64()
            .map(|number| format!("{number:?}"))
            .unwrap_or_else(|| number.to_string()),
        Value::Object(object) => ["suspect", "poor"]
            .iter()
            .map(|step| canonical(object.get(*step).unwrap_or(&Value::Null)))
            .collect::<Vec<_>>()
            .join("/"),
        other => other.to_string(),
    }
}

/// Read a stored value the way resolution does: anything this build cannot
/// read is `None`, and the next layer decides.
fn readable(spec: &FieldSpec, value: &Value) -> bool {
    validate(spec, value).is_ok()
}

/// Check a value against its setting. `Err` says what is wrong in words a
/// person can act on.
pub fn validate(spec: &FieldSpec, value: &Value) -> Result<(), String> {
    let label = spec.label;
    match &spec.kind {
        FieldKind::Number { min, max, unit, .. } => match finite(value) {
            Some(number) if (*min..=*max).contains(&number) => Ok(()),
            _ => Err(format!(
                "{label} must be a number from {} to {} {unit}",
                format_number(*min),
                format_number(*max)
            )),
        },
        FieldKind::Count { min, max, unit } => match value.as_u64() {
            Some(count) if (*min as u64..=*max as u64).contains(&count) => Ok(()),
            _ => Err(format!(
                "{label} must be a whole number from {min} to {max} {unit}"
            )),
        },
        FieldKind::Toggle => value
            .as_bool()
            .map(|_| ())
            .ok_or_else(|| format!("{label} must be true or false")),
        FieldKind::Choice { options } => match value.as_str() {
            Some(text) if options.iter().any(|option| option.value == text) => Ok(()),
            _ => Err(format!(
                "{label} must be one of {}",
                options
                    .iter()
                    .map(|option| option.value)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        },
        FieldKind::Check { .. } => CheckMode::parse(value)
            .map(|_| ())
            .ok_or_else(|| format!("{label} must be flag, advisory or off")),
        FieldKind::Limits {
            min,
            max,
            lower_is_worse,
            ..
        } => {
            let Some(object) = value.as_object() else {
                return Err(format!("{label} must be {{\"suspect\": …, \"poor\": …}}"));
            };
            if let Some(other) = object
                .keys()
                .find(|key| *key != "suspect" && *key != "poor")
            {
                return Err(format!("{label} has no limit called {other}"));
            }
            let step = |name: &str| -> Result<Option<f64>, String> {
                match object.get(name) {
                    None | Some(Value::Null) => Ok(None),
                    Some(value) => match finite(value) {
                        Some(number) if (*min..=*max).contains(&number) => Ok(Some(number)),
                        _ => Err(format!(
                            "{label}: the {name} limit must be a number from {} to {}, or \
                             null for never",
                            format_number(*min),
                            format_number(*max)
                        )),
                    },
                }
            };
            let (suspect, poor) = (step("suspect")?, step("poor")?);
            if let (Some(suspect), Some(poor)) = (suspect, poor) {
                let ordered = if *lower_is_worse {
                    suspect >= poor
                } else {
                    suspect <= poor
                };
                if !ordered {
                    return Err(format!(
                        "{label}: a frame must turn suspect before it turns poor"
                    ));
                }
            }
            Ok(())
        }
    }
}

fn format_number(value: f64) -> String {
    let rounded = (value * 10_000.0).round() / 10_000.0;
    let text = format!("{rounded}");
    if text == "-0" {
        "0".into()
    } else {
        text
    }
}

/// A value in words, the way the light's account names it.
pub fn describe(spec: &FieldSpec, value: &Value) -> String {
    match &spec.kind {
        FieldKind::Number { unit, zero, .. } => match (finite(value), zero) {
            (Some(0.0), Some(zero)) => (*zero).to_string(),
            (Some(number), _) if *unit == "°" => format!("{}°", format_number(number)),
            (Some(number), _) => format!("{} {unit}", format_number(number)),
            _ => value.to_string(),
        },
        FieldKind::Count { unit, .. } => format!("{value} {unit}"),
        FieldKind::Toggle => if value.as_bool() == Some(true) {
            "on"
        } else {
            "off"
        }
        .into(),
        FieldKind::Choice { options } => options
            .iter()
            .find(|option| Some(option.value) == value.as_str())
            .map(|option| option.label.to_string())
            .unwrap_or_else(|| value.to_string()),
        FieldKind::Check { levels } => match CheckMode::parse(value) {
            Some(CheckMode::Flag) => if levels.contains(&"poor") {
                "flag"
            } else {
                "check"
            }
            .into(),
            Some(CheckMode::Advisory) => "advisory".into(),
            Some(CheckMode::Off) => "off".into(),
            None => value.to_string(),
        },
        FieldKind::Limits { unit, percent, .. } => {
            let step = |name: &str| match value.get(name).and_then(finite) {
                Some(number) if *percent => format!("{}{unit}", format_number(number * 100.0)),
                Some(number) if *unit == "×" => format!("{}×", format_number(number)),
                Some(number) => format!("{} {unit}", format_number(number)),
                None => "never".into(),
            };
            format!("suspect {}, poor {}", step("suspect"), step("poor"))
        }
    }
}

/// One rig's settings, as the registry keeps them.
#[derive(Debug, Clone, Default)]
pub struct RigLayer {
    /// What the person saw the rig called when they saved it.
    pub name: Option<String>,
    pub values: RuleValues,
}

/// A rig that has settings of its own, as a light's rules name it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RigLabel {
    pub rig_uuid: String,
    pub name: String,
}

/// One setting that differs from its default for a light, and who set it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuleNote {
    pub key: &'static str,
    pub label: &'static str,
    pub value: Value,
    /// The value in words.
    pub text: String,
    pub source: RuleSource,
}

/// Everything matching, the light checks and grading read, resolved for
/// one rig: its own value, else the server's, else the default.
#[derive(Debug, Clone, PartialEq)]
pub struct Rules {
    /// The rig these were resolved for, when it has settings of its own.
    pub rig: Option<RigLabel>,
    pub rotation_deg: f64,
    pub focal_length_mm: f64,
    pub dark_temperature_c: f64,
    pub flat_session_seconds: u64,
    pub use_darks: bool,
    pub external_masters: ExternalMasterPolicy,
    pub dark_reach_days: f64,
    pub complete_dark_frames: usize,
    pub dark_choice: DarkChoice,
    /// Days; infinite when the limit is lifted.
    pub flat_max_age_days: f64,
    /// Days; infinite when the check is off.
    pub flat_check_age_days: f64,
    pub thin_master_frames: usize,
    pub flat_grouping: FlatGrouping,
    /// Percent of the sky background.
    pub flat_split_dust_pct: f64,
    pub flat_split_pattern_pct: f64,
    /// The `night_zone` setting: where the rig's nights split, as an
    /// offset from UTC in seconds, or `None` to follow its site
    /// ([`super::night`]).
    pub night_offset_seconds: Option<i64>,
    /// The `light_nights` setting.
    pub light_nights: LightNightSplit,
    checks: [CheckMode; Check::ALL.len()],
    pub bias: BiasScreening,
    pub dark: DarkScreening,
    pub flat: FlatScreening,
    /// Each setting's resolved value and where it came from.
    resolved: BTreeMap<&'static str, (Value, RuleSource)>,
}

impl Default for Rules {
    fn default() -> Self {
        Rules::resolve(&RuleValues::new(), None)
    }
}

impl Rules {
    /// Resolve every setting: the rig's layer first, then `global`, then
    /// the default.
    fn resolve(global: &RuleValues, rig: Option<(&str, &RigLayer)>) -> Self {
        let mut resolved = BTreeMap::new();
        for spec in fields() {
            let from_rig = rig
                .and_then(|(_, layer)| layer.values.get(spec.key))
                .filter(|value| readable(spec, value));
            let from_global = global.get(spec.key).filter(|value| readable(spec, value));
            let (value, source) = match (from_rig, from_global) {
                (Some(value), _) => (value.clone(), RuleSource::Rig),
                (None, Some(value)) => (value.clone(), RuleSource::Global),
                (None, None) => (spec.default.clone(), RuleSource::Default),
            };
            resolved.insert(spec.key, (value, source));
        }
        let value = |key: &str| &resolved[key].0;
        let number = |key: &str| finite(value(key)).unwrap_or(0.0);
        let count = |key: &str| value(key).as_u64().unwrap_or(0) as usize;
        let days_or_never = |key: &str| match number(key) {
            0.0 => f64::INFINITY,
            days => days,
        };
        let limits = |grade: GradeCheck| {
            let stored = value(grade.key());
            let never = if grade.lower_is_worse() {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            };
            let step = |name: &str| stored.get(name).and_then(finite).unwrap_or(never);
            Limits::new(step("suspect"), step("poor"))
        };
        let bias = BiasScreening {
            level: limits(GradeCheck::BiasLevel),
            noise: limits(GradeCheck::BiasNoise),
            banding: limits(GradeCheck::BiasBanding),
        };
        let dark = DarkScreening {
            level_excess: limits(GradeCheck::DarkLevel),
            noise: limits(GradeCheck::DarkNoise),
            hot_pixels: limits(GradeCheck::DarkHotPixels),
            banding: limits(GradeCheck::DarkBanding),
        };
        let flat = FlatScreening {
            clipped: limits(GradeCheck::FlatClipped),
            bright: limits(GradeCheck::FlatBright),
            dim: limits(GradeCheck::FlatDim),
            illumination: limits(GradeCheck::FlatIllumination),
            features: limits(GradeCheck::FlatSpots),
            changing_features: limits(GradeCheck::FlatChangingSpots),
            carried_features: limits(GradeCheck::FlatCarriedSpots),
            ..FlatScreening::default()
        };
        let mut checks = [CheckMode::Flag; Check::ALL.len()];
        for check in Check::ALL {
            checks[check.index()] = CheckMode::parse(value(check.key())).unwrap_or_default();
        }
        Self {
            rig: rig.map(|(uuid, layer)| RigLabel {
                rig_uuid: uuid.to_string(),
                name: layer
                    .name
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| "this rig".into()),
            }),
            rotation_deg: number("rotation_tolerance_deg"),
            focal_length_mm: number("focal_length_tolerance_mm"),
            dark_temperature_c: number("dark_temperature_tolerance_c"),
            flat_session_seconds: (number("flat_session_hours") * 3600.0).round() as u64,
            use_darks: value("use_darks").as_bool().unwrap_or(true),
            external_masters: value("external_masters")
                .as_str()
                .and_then(ExternalMasterPolicy::parse)
                .unwrap_or_default(),
            dark_reach_days: number("dark_reach_days"),
            complete_dark_frames: count("complete_dark_frames").max(super::MIN_MASTER_FRAMES),
            dark_choice: match value("dark_choice").as_str() {
                Some("nearest") => DarkChoice::Nearest,
                _ => DarkChoice::Complete,
            },
            flat_max_age_days: days_or_never("flat_max_age_days"),
            flat_check_age_days: days_or_never("flat_check_age_days"),
            thin_master_frames: count("thin_master_frames"),
            flat_grouping: match value("flat_grouping").as_str() {
                Some("time") => FlatGrouping::Time,
                _ => FlatGrouping::Measured,
            },
            flat_split_dust_pct: number("flat_split_dust_pct"),
            flat_split_pattern_pct: number("flat_split_pattern_pct"),
            night_offset_seconds: night_offset_seconds(value("night_zone")),
            light_nights: match value("light_nights").as_str() {
                Some("catalog") => LightNightSplit::Catalog,
                _ => LightNightSplit::Rig,
            },
            checks,
            bias,
            dark,
            flat,
            resolved,
        }
    }

    pub fn mode(&self, check: Check) -> CheckMode {
        self.checks[check.index()]
    }

    /// Where one setting's value came from.
    pub fn source(&self, key: &str) -> RuleSource {
        self.resolved
            .get(key)
            .map(|(_, source)| *source)
            .unwrap_or(RuleSource::Default)
    }

    /// Who set a value, in words, or `None` when it is the default: "set
    /// globally" or "set for SpaceCat61 · ASI2600MM Pro".
    pub fn set_by(&self, key: &str) -> Option<String> {
        let (value, source) = self.resolved.get(key)?;
        if field(key).is_some_and(|spec| equivalent(&spec.default, value)) {
            return None;
        }
        match source {
            RuleSource::Default => None,
            RuleSource::Global => Some("set globally".into()),
            RuleSource::Rig => Some(format!(
                "set for {}",
                self.rig
                    .as_ref()
                    .map_or("this rig", |rig| rig.name.as_str())
            )),
        }
    }

    /// `", set …"` after a limit, or nothing for a default.
    pub fn set_by_suffix(&self, key: &str) -> String {
        self.set_by(key)
            .map(|by| format!(", {by}"))
            .unwrap_or_default()
    }

    /// Resolved values for one selected light. Rule provenance remains on
    /// `Rules` for health/UI reporting; the core needs the effective values.
    pub(super) fn shared_settings(&self) -> Vec<seiza_calibration::library::Setting> {
        self.resolved
            .iter()
            .map(|(key, (value, _))| seiza_calibration::library::Setting {
                key: (*key).into(),
                value: value.clone(),
            })
            .collect()
    }

    /// The settings that differ from their defaults for these rules, in
    /// the panel's order.
    pub fn notes(&self) -> Vec<RuleNote> {
        fields()
            .iter()
            .filter_map(|spec| {
                let (value, source) = self.resolved.get(spec.key)?;
                (!equivalent(value, &spec.default)).then(|| RuleNote {
                    key: spec.key,
                    label: spec.label,
                    value: value.clone(),
                    text: describe(spec, value),
                    source: *source,
                })
            })
            .collect()
    }

    /// The tolerances a candidate is matched to a light with.
    pub fn tolerances(&self) -> MatchTolerances {
        MatchTolerances {
            rotation_deg: self.rotation_deg,
            focal_length_mm: self.focal_length_mm,
            dark_temperature_c: self.dark_temperature_c,
            flat_session_seconds: self.flat_session_seconds,
            ..MatchTolerances::default()
        }
    }

    /// The tolerances the frames of one master are held to among
    /// themselves. The integrator compares each flat with the first at its
    /// own default angle and focal length, so a wider match tolerance cannot
    /// widen them here: it would hand the integrator a set it refuses.
    pub fn coherence_tolerances(&self) -> MatchTolerances {
        let defaults = MatchTolerances::default();
        let mut tolerances = self.tolerances();
        tolerances.rotation_deg = tolerances.rotation_deg.min(defaults.rotation_deg);
        tolerances.focal_length_mm = tolerances.focal_length_mm.min(defaults.focal_length_mm);
        tolerances
    }

    /// The matching settings that differ from their defaults, as one key,
    /// or `None` when every one is the default. A stack's identity takes
    /// it, so a change that alters which frames a master takes builds the
    /// master again, while the defaults leave every existing key alone.
    pub fn matching_digest(&self) -> Option<String> {
        let changed: Vec<String> = fields()
            .iter()
            .filter(|spec| spec.effect == Effect::Matching)
            .filter_map(|spec| {
                let (value, _) = self.resolved.get(spec.key)?;
                (!equivalent(value, &spec.default))
                    .then(|| format!("{}={}", spec.key, canonical(value)))
            })
            .collect();
        (!changed.is_empty()).then(|| hex(&changed.join(";")))
    }
}

fn hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The server's settings and every rig's, with each rig's rules resolved
/// once.
#[derive(Debug, Clone)]
pub struct RuleBook {
    global: RuleValues,
    rigs: BTreeMap<String, RigLayer>,
    global_rules: Arc<Rules>,
    rig_rules: HashMap<String, Arc<Rules>>,
}

impl Default for RuleBook {
    fn default() -> Self {
        Self::new(RuleValues::new(), BTreeMap::new())
    }
}

impl RuleBook {
    pub fn new(global: RuleValues, rigs: BTreeMap<String, RigLayer>) -> Self {
        let global_rules = Arc::new(Rules::resolve(&global, None));
        let rig_rules = rigs
            .iter()
            .filter(|(_, layer)| !layer.values.is_empty())
            .map(|(uuid, layer)| {
                (
                    uuid.clone(),
                    Arc::new(Rules::resolve(&global, Some((uuid, layer)))),
                )
            })
            .collect();
        Self {
            global,
            rigs,
            global_rules,
            rig_rules,
        }
    }

    pub(super) fn shared_request(&self) -> seiza_calibration::library::Request {
        use seiza_calibration::library::{Request, RigSettings, Setting};
        let settings = |values: &RuleValues| {
            values
                .iter()
                .map(|(key, value)| Setting {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect()
        };
        Request {
            global_settings: settings(&self.global),
            rigs: self
                .rigs
                .iter()
                .map(|(id, layer)| RigSettings {
                    rig_uuid: id.clone(),
                    name: layer.name.clone(),
                    settings: settings(&layer.values),
                })
                .collect(),
            ..Request::default()
        }
    }

    pub fn global_values(&self) -> &RuleValues {
        &self.global
    }

    /// The rigs with settings of their own.
    pub fn rig_uuids(&self) -> impl Iterator<Item = &str> {
        self.rigs
            .iter()
            .filter(|(_, layer)| !layer.values.is_empty())
            .map(|(uuid, _)| uuid.as_str())
    }

    /// Whether a rig sets `key` for itself rather than inheriting it.
    pub fn rig_sets(&self, rig_uuid: &str, key: &str) -> bool {
        self.rigs
            .get(rig_uuid)
            .is_some_and(|layer| layer.values.contains_key(key))
    }

    /// The rules every rig without settings of its own follows.
    pub fn global(&self) -> Arc<Rules> {
        Arc::clone(&self.global_rules)
    }

    pub fn for_rig(&self, rig_uuid: &str) -> Arc<Rules> {
        self.rig_rules
            .get(rig_uuid)
            .cloned()
            .unwrap_or_else(|| self.global())
    }

    pub fn for_light(&self, light: &FrameMeta) -> Arc<Rules> {
        if self.rig_rules.is_empty() {
            return self.global();
        }
        self.for_rig(&light_rig_uuid(light))
    }

    /// Whether nothing is set anywhere.
    pub fn is_empty(&self) -> bool {
        self.global.is_empty() && self.rig_rules.is_empty()
    }

    /// A key of every setting, or `None` when nothing is set.
    pub fn digest(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let rigs: BTreeMap<&String, &RuleValues> = self
            .rigs
            .iter()
            .filter(|(_, layer)| !layer.values.is_empty())
            .map(|(uuid, layer)| (uuid, &layer.values))
            .collect();
        Some(hex(
            &json!({ "global": self.global, "rigs": rigs }).to_string()
        ))
    }

    /// Whether any setting grading reads differs between two books, so a
    /// change of settings knows whether grading must run again: every
    /// frame-grade limit, and the matching tolerances grading groups peers
    /// by ([`GRADING_GROUPS`]).
    pub fn grades_differ(&self, other: &RuleBook) -> bool {
        let grade_values = |book: &RuleBook| {
            let pick = |values: &RuleValues| -> BTreeMap<String, Value> {
                values
                    .iter()
                    .filter(|(key, _)| {
                        GRADING_GROUPS.contains(&key.as_str())
                            || field(key).is_some_and(|spec| spec.effect == Effect::Grades)
                    })
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            };
            let rigs: BTreeMap<String, BTreeMap<String, Value>> = book
                .rigs
                .iter()
                .map(|(uuid, layer)| (uuid.clone(), pick(&layer.values)))
                .filter(|(_, values)| !values.is_empty())
                .collect();
            (pick(&book.global), rigs)
        };
        grade_values(self) != grade_values(other)
    }
}

/// Matching settings that grading reads too: a dark is graded against the
/// darks within its rig's temperature tolerance, and a flat against the
/// flats its rig's rotation and focal length tolerances put in its session.
/// Changing one regroups the peers, so the frames are graded again.
pub const GRADING_GROUPS: [&str; 3] = [
    "dark_temperature_tolerance_c",
    "rotation_tolerance_deg",
    "focal_length_tolerance_mm",
];

/// Whether Seiza takes a tolerance as it is, beyond this setting's own
/// range: a stack hands each rig's tolerances to Seiza, which refuses a
/// non-finite, negative or over-wide one rather than clamp it. A setting
/// that is not a tolerance passes.
pub fn stacker_accepts(key: &str, value: &Value) -> Result<(), String> {
    let mut tolerances = MatchTolerances::default();
    let Some(number) = value.as_f64() else {
        return Ok(());
    };
    match key {
        "rotation_tolerance_deg" => tolerances.rotation_deg = number,
        "focal_length_tolerance_mm" => tolerances.focal_length_mm = number,
        "dark_temperature_tolerance_c" => tolerances.dark_temperature_c = number,
        "flat_session_hours" => tolerances.flat_session_seconds = (number * 3600.0).round() as u64,
        _ => return Ok(()),
    }
    tolerances.validate().map_err(|error| {
        let label = field(key).map(|spec| spec.label).unwrap_or(key);
        format!("{label}: {error}")
    })
}

/// The rig a light was shot with, by the key the library files rigs
/// under.
pub fn light_rig_uuid(light: &FrameMeta) -> String {
    super::stable_uuid(&super::rig_signature(None, light))
}

static BOOK: LazyLock<RwLock<Arc<RuleBook>>> =
    LazyLock::new(|| RwLock::new(Arc::new(RuleBook::default())));

#[cfg(test)]
thread_local! {
    /// Settings one test put in force for itself, seen only on its own
    /// thread, so the tests running beside it keep the defaults.
    static TEST_BOOK: std::cell::RefCell<Option<Arc<RuleBook>>> =
        const { std::cell::RefCell::new(None) };
}

/// Put settings in force for the calling test's thread alone, or `None`
/// to go back to the process's.
#[cfg(test)]
pub(crate) fn set_test_rule_book(book: Option<RuleBook>) {
    TEST_BOOK.with(|slot| *slot.borrow_mut() = book.map(Arc::new));
}

/// The settings in force.
pub fn rule_book() -> Arc<RuleBook> {
    #[cfg(test)]
    if let Some(book) = TEST_BOOK.with(|slot| slot.borrow().clone()) {
        return book;
    }
    Arc::clone(&BOOK.read().unwrap_or_else(|poisoned| poisoned.into_inner()))
}

/// Put new settings in force for every match, check and grade after this.
pub fn set_rule_book(book: RuleBook) {
    *BOOK
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(book);
}

/// Change the server's layer and keep every rig's.
pub(crate) fn edit_global(edit: impl FnOnce(&mut RuleValues)) {
    let mut guard = BOOK
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut global = guard.global.clone();
    edit(&mut global);
    *guard = Arc::new(RuleBook::new(global, guard.rigs.clone()));
}

/// The built-in defaults, whatever the settings: for questions about a
/// frame itself rather than about what it calibrates.
pub fn defaults() -> &'static Rules {
    static DEFAULTS: LazyLock<Rules> = LazyLock::new(Rules::default);
    &DEFAULTS
}

/// The rules a light is matched and checked with.
pub fn rules_for_light(light: &FrameMeta) -> Arc<Rules> {
    rule_book().for_light(light)
}

/// The rules a calibration frame of this rig is graded with.
pub fn rules_for_rig(rig_uuid: &str) -> Arc<Rules> {
    rule_book().for_rig(rig_uuid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, Value)]) -> RuleValues {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn the_defaults_are_the_values_the_checks_were_written_with() {
        let rules = Rules::default();
        let tolerances = MatchTolerances::default();
        assert_eq!(rules.tolerances().rotation_deg, tolerances.rotation_deg);
        assert_eq!(
            rules.tolerances().focal_length_mm,
            tolerances.focal_length_mm
        );
        assert_eq!(
            rules.tolerances().dark_temperature_c,
            tolerances.dark_temperature_c
        );
        assert_eq!(
            rules.tolerances().flat_session_seconds,
            tolerances.flat_session_seconds
        );
        assert_eq!(
            rules.tolerances().master_temperature_c,
            tolerances.master_temperature_c
        );
        assert_eq!(rules.coherence_tolerances().rotation_deg, 2.0);
        assert!(rules.use_darks);
        assert_eq!(rules.external_masters, ExternalMasterPolicy::Prefer);
        assert_eq!(rules.dark_reach_days, 183.0);
        assert_eq!(rules.complete_dark_frames, 10);
        assert_eq!(rules.dark_choice, DarkChoice::Complete);
        assert_eq!(rules.flat_max_age_days, 60.0);
        assert_eq!(rules.flat_check_age_days, 30.0);
        assert_eq!(rules.thin_master_frames, 5);
        for check in Check::ALL {
            assert_eq!(rules.mode(check), CheckMode::Flag, "{check:?}");
        }
        assert_eq!(rules.bias, BiasScreening::default());
        assert_eq!(rules.dark, DarkScreening::default());
        assert_eq!(rules.flat, FlatScreening::default());
        assert_eq!(rules.matching_digest(), None);
        assert!(rules.notes().is_empty());
        assert_eq!(rules.set_by("dark_reach_days"), None);
        assert!(RuleBook::default().digest().is_none());
    }

    #[test]
    fn every_default_passes_its_own_validation() {
        for spec in fields() {
            assert_eq!(validate(spec, &spec.default), Ok(()), "{}", spec.key);
        }
        let mut keys: Vec<&str> = fields().iter().map(|spec| spec.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), fields().len(), "keys are unique");
    }

    #[test]
    fn a_rig_value_beats_the_global_one_which_beats_the_default() {
        let global = values(&[
            ("dark_reach_days", json!(90)),
            ("thin_master_frames", json!(4)),
        ]);
        let rig = RigLayer {
            name: Some("SpaceCat61 · ASI2600MM Pro".into()),
            values: values(&[("dark_reach_days", json!(400))]),
        };
        let book = RuleBook::new(global, BTreeMap::from([("rig-a".to_string(), rig)]));

        let rig_rules = book.for_rig("rig-a");
        assert_eq!(rig_rules.dark_reach_days, 400.0);
        assert_eq!(rig_rules.source("dark_reach_days"), RuleSource::Rig);
        assert_eq!(rig_rules.thin_master_frames, 4, "inherits the global value");
        assert_eq!(rig_rules.source("thin_master_frames"), RuleSource::Global);
        assert_eq!(rig_rules.complete_dark_frames, 10, "inherits the default");
        assert_eq!(
            rig_rules.source("complete_dark_frames"),
            RuleSource::Default
        );
        assert_eq!(
            rig_rules.set_by("dark_reach_days").as_deref(),
            Some("set for SpaceCat61 · ASI2600MM Pro")
        );
        assert_eq!(
            rig_rules.set_by("thin_master_frames").as_deref(),
            Some("set globally")
        );

        let other = book.for_rig("rig-b");
        assert_eq!(other.dark_reach_days, 90.0, "another rig takes the global");
        assert_eq!(other.rig, None);
        assert!(book.digest().is_some());
    }

    #[test]
    fn a_rig_names_its_night_zone_and_others_follow_the_global_or_their_site() {
        assert_eq!(Rules::default().night_offset_seconds, None, "auto");
        let global = values(&[("night_zone", json!("5.75"))]);
        let rig = RigLayer {
            name: Some("C925".into()),
            values: values(&[("night_zone", json!("-8"))]),
        };
        let book = RuleBook::new(global, BTreeMap::from([("c925".to_string(), rig)]));
        assert_eq!(book.for_rig("c925").night_offset_seconds, Some(-8 * 3600));
        assert_eq!(
            book.for_rig("other").night_offset_seconds,
            Some(5 * 3600 + 45 * 60)
        );
        // A zone the list does not offer falls through, as any unreadable
        // value does; "auto" at the rig beats a global offset.
        let book = RuleBook::new(
            values(&[("night_zone", json!("-8"))]),
            BTreeMap::from([
                (
                    "odd".to_string(),
                    RigLayer {
                        name: None,
                        values: values(&[("night_zone", json!("-7.25"))]),
                    },
                ),
                (
                    "auto".to_string(),
                    RigLayer {
                        name: None,
                        values: values(&[("night_zone", json!("auto"))]),
                    },
                ),
            ]),
        );
        assert_eq!(book.for_rig("odd").night_offset_seconds, Some(-8 * 3600));
        assert_eq!(book.for_rig("auto").night_offset_seconds, None);
    }

    #[test]
    fn a_value_this_build_cannot_read_falls_through_to_the_next_layer() {
        let global = values(&[
            ("dark_choice", json!("by_gain")),
            ("rotation_tolerance_deg", json!(4.0)),
        ]);
        let rig = RigLayer {
            name: None,
            values: values(&[
                ("rotation_tolerance_deg", json!("wide")),
                ("check_no_flat", json!("sometimes")),
            ]),
        };
        let book = RuleBook::new(global, BTreeMap::from([("rig".to_string(), rig)]));
        let rules = book.for_rig("rig");
        assert_eq!(rules.dark_choice, DarkChoice::Complete);
        assert_eq!(rules.rotation_deg, 4.0);
        assert_eq!(rules.mode(Check::NoFlat), CheckMode::Flag);
    }

    #[test]
    fn grade_limits_turn_off_a_step_with_null() {
        let global = values(&[
            (
                "grade_flat_illumination",
                json!({"suspect": 0.05, "poor": null}),
            ),
            ("grade_flat_dim", json!({"suspect": null, "poor": 0.01})),
        ]);
        let rules = RuleBook::new(global, BTreeMap::new()).global();
        assert_eq!(rules.flat.illumination.suspect, 0.05);
        assert_eq!(rules.flat.illumination.poor, f64::INFINITY);
        assert_eq!(rules.flat.dim.suspect, f64::NEG_INFINITY);
        assert_eq!(rules.flat.dim.poor, 0.01);
    }

    #[test]
    fn validation_names_the_setting_and_its_range() {
        let spec = field("dark_temperature_tolerance_c").unwrap();
        assert!(validate(spec, &json!(2.0)).is_ok());
        assert!(validate(spec, &json!(0)).is_ok());
        // Wider than Seiza's default: stacks check each light with its rig's own.
        assert!(validate(spec, &json!(3.5)).is_ok());
        assert!(validate(spec, &json!(20.0)).is_ok());
        assert!(validate(spec, &json!(20.5))
            .unwrap_err()
            .contains("from 0 to 20"));
        assert!(validate(spec, &json!("2")).is_err());
        let count = field("thin_master_frames").unwrap();
        assert!(validate(count, &json!(4.5)).is_err());
        assert!(validate(count, &json!(1)).is_err());
        assert!(validate(count, &json!(64)).is_ok());
        let mode = field("check_no_flat").unwrap();
        assert!(validate(mode, &json!("advisory")).is_ok());
        assert!(validate(mode, &json!("poor")).is_err());
        let limits = field("grade_flat_dim").unwrap();
        assert!(validate(limits, &json!({"suspect": 0.05, "poor": 0.02})).is_ok());
        assert!(validate(limits, &json!({"suspect": 0.02, "poor": 0.05}))
            .unwrap_err()
            .contains("suspect before"));
        assert!(validate(limits, &json!({"suspect": 0.05, "bad": 0.02})).is_err());
        let higher = field("grade_flat_illumination").unwrap();
        assert!(validate(higher, &json!({"suspect": 0.1, "poor": 0.05})).is_err());
        assert!(validate(higher, &json!({"suspect": null, "poor": null})).is_ok());
    }

    #[test]
    fn only_changed_matching_settings_change_a_stacks_key() {
        let flags_only = RuleBook::new(
            values(&[
                ("thin_master_frames", json!(3)),
                ("check_mixed_flats", json!("off")),
                ("grade_flat_dim", json!({"suspect": 0.05, "poor": 0.02})),
            ]),
            BTreeMap::new(),
        );
        assert_eq!(flags_only.global().matching_digest(), None);
        let same_as_default = RuleBook::new(
            values(&[("rotation_tolerance_deg", json!(2.0))]),
            BTreeMap::new(),
        );
        assert_eq!(same_as_default.global().matching_digest(), None);
        let wider = RuleBook::new(
            values(&[("rotation_tolerance_deg", json!(5.0))]),
            BTreeMap::new(),
        );
        assert!(wider.global().matching_digest().is_some());
        assert_eq!(wider.global().coherence_tolerances().rotation_deg, 2.0);
        assert_eq!(wider.global().tolerances().rotation_deg, 5.0);
    }

    /// Seiza refuses a tolerance it cannot use rather than clamp it, so the
    /// settings refuse it first, naming the setting; a wide one it can use
    /// passes.
    #[test]
    fn a_tolerance_seiza_would_refuse_is_refused_by_name() {
        assert!(stacker_accepts("rotation_tolerance_deg", &json!(6)).is_ok());
        assert!(stacker_accepts("dark_temperature_tolerance_c", &json!(10)).is_ok());
        assert!(stacker_accepts("flat_session_hours", &json!(72)).is_ok());
        let refused = stacker_accepts("rotation_tolerance_deg", &json!(181)).unwrap_err();
        assert!(refused.starts_with("Flat rotation: "), "{refused}");
        let refused = stacker_accepts("focal_length_tolerance_mm", &json!(-1)).unwrap_err();
        assert!(refused.starts_with("Flat focal length: "), "{refused}");
        assert!(stacker_accepts("thin_master_frames", &json!(-1)).is_ok());
    }

    /// Grading screens flats by session, and sessions are nights, so a
    /// night_zone change grades again; it picks no frames itself, so it
    /// does not re-key every master and stack of the rig.
    #[test]
    fn a_night_zone_grades_again_and_keys_no_master() {
        let base = RuleBook::new(BTreeMap::new(), BTreeMap::new());
        let rig = RigLayer {
            name: None,
            values: values(&[("night_zone", json!("-8"))]),
        };
        let zoned = RuleBook::new(BTreeMap::new(), BTreeMap::from([("c925".to_string(), rig)]));
        assert!(
            base.grades_differ(&zoned),
            "a new night split regroups flat grading sessions"
        );
        assert_eq!(
            zoned.for_rig("c925").matching_digest(),
            None,
            "night_zone alone re-keys every master and stack of the rig"
        );
    }

    #[test]
    fn grades_differ_only_when_a_grade_setting_changes() {
        let base = RuleBook::new(values(&[("thin_master_frames", json!(3))]), BTreeMap::new());
        let flags = RuleBook::new(values(&[("thin_master_frames", json!(4))]), BTreeMap::new());
        assert!(!base.grades_differ(&flags));
        let rig = RigLayer {
            name: None,
            values: values(&[(
                "grade_flat_illumination",
                json!({"suspect": 0.05, "poor": 0.1}),
            )]),
        };
        let grades = RuleBook::new(
            values(&[("thin_master_frames", json!(3))]),
            BTreeMap::from([("rig".to_string(), rig)]),
        );
        assert!(base.grades_differ(&grades));
    }

    /// Grading groups a dark's peers by the dark temperature tolerance and
    /// a flat's session by the rotation and focal length tolerances, so
    /// changing one, globally or for a rig, grades the frames again. Other
    /// matching settings leave the grades alone.
    #[test]
    fn the_tolerances_grading_groups_by_grade_again() {
        let base = RuleBook::new(values(&[]), BTreeMap::new());
        for (key, value) in [
            ("dark_temperature_tolerance_c", json!(1)),
            ("rotation_tolerance_deg", json!(1)),
            ("focal_length_tolerance_mm", json!(0.5)),
        ] {
            let global = RuleBook::new(values(&[(key, value.clone())]), BTreeMap::new());
            assert!(base.grades_differ(&global), "{key} globally");
            let rig = RigLayer {
                name: None,
                values: values(&[(key, value)]),
            };
            let for_rig = RuleBook::new(values(&[]), BTreeMap::from([("rig".to_string(), rig)]));
            assert!(base.grades_differ(&for_rig), "{key} for a rig");
        }
        for (key, value) in [
            ("dark_reach_days", json!(30)),
            ("flat_session_hours", json!(72)),
            ("use_darks", json!(false)),
        ] {
            let other = RuleBook::new(values(&[(key, value)]), BTreeMap::new());
            assert!(!base.grades_differ(&other), "{key}");
        }
    }

    #[test]
    fn notes_list_what_differs_with_its_source_in_words() {
        let rig = RigLayer {
            name: Some("Askar · 533".into()),
            values: values(&[
                ("check_no_bias_or_dark", json!("advisory")),
                (
                    "grade_flat_illumination",
                    json!({"suspect": 0.05, "poor": 0.1}),
                ),
                ("flat_max_age_days", json!(0)),
            ]),
        };
        let book = RuleBook::new(
            values(&[("dark_temperature_tolerance_c", json!(2))]),
            BTreeMap::from([("rig".to_string(), rig)]),
        );
        let notes = book.for_rig("rig").notes();
        let texts: Vec<(&str, &str, RuleSource)> = notes
            .iter()
            .map(|note| (note.key, note.text.as_str(), note.source))
            .collect();
        assert_eq!(
            texts,
            vec![
                ("dark_temperature_tolerance_c", "2 °C", RuleSource::Global),
                ("flat_max_age_days", "no limit", RuleSource::Rig),
                ("check_no_bias_or_dark", "advisory", RuleSource::Rig),
                (
                    "grade_flat_illumination",
                    "suspect 5%, poor 10%",
                    RuleSource::Rig
                ),
            ]
        );
    }
}
