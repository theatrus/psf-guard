//! Bandpass identity from filter names, and default exposure lengths. A
//! bandpass is what the project asks for; a filter is what one rig has. Two
//! rigs' "Ha" and "H-alpha 3nm" filters serve the same bandpass, and a
//! broadband red plate never serves a narrowband objective.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BandpassKind {
    Broadband,
    Narrowband,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Bandpass {
    /// Stable lower-case identity such as `luminance`, `red`, `h_alpha`, `oiii`.
    pub id: String,
    pub name: &'static str,
    pub kind: BandpassKind,
}

const KNOWN: &[(&str, &str, BandpassKind, &[&str])] = &[
    (
        "luminance",
        "Luminance",
        BandpassKind::Broadband,
        &[
            "l",
            "lum",
            "luminance",
            "clear",
            "uv/ir",
            "uvir",
            "uv-ir",
            "lpro",
            "l-pro",
            "none",
            "nofilter",
            "no filter",
            "osc",
            "rgb",
        ],
    ),
    ("red", "Red", BandpassKind::Broadband, &["r", "red"]),
    ("green", "Green", BandpassKind::Broadband, &["g", "green"]),
    ("blue", "Blue", BandpassKind::Broadband, &["b", "blue"]),
    (
        "h_alpha",
        "H-alpha",
        BandpassKind::Narrowband,
        &[
            "ha",
            "h-alpha",
            "halpha",
            "h_alpha",
            "hα",
            "h-a",
            "h alpha",
            "hydrogen alpha",
            "ha3",
            "ha5",
            "ha7",
            "ha3nm",
            "ha5nm",
            "ha7nm",
        ],
    ),
    (
        "oiii",
        "O III",
        BandpassKind::Narrowband,
        &[
            "oiii",
            "o3",
            "o-iii",
            "o iii",
            "oxygen",
            "oxygen iii",
            "oiii3nm",
            "oiii5nm",
            "oiii7nm",
        ],
    ),
    (
        "sii",
        "S II",
        BandpassKind::Narrowband,
        &[
            "sii",
            "s2",
            "s-ii",
            "s ii",
            "sulfur",
            "sulphur",
            "sulfur ii",
            "sii3nm",
            "sii5nm",
            "sii7nm",
        ],
    ),
    (
        "h_beta",
        "H-beta",
        BandpassKind::Narrowband,
        &["hb", "hbeta", "h-beta", "h beta"],
    ),
    (
        "dual_band",
        "Dual band",
        BandpassKind::Narrowband,
        &[
            "dual",
            "dualband",
            "dual band",
            "l-extreme",
            "lextreme",
            "l-enhance",
            "lenhance",
            "l-ultimate",
            "lultimate",
            "nbz",
            "ha/oiii",
            "ha+oiii",
            "duo",
            "duo-narrowband",
            "alp-t",
            "alpt",
        ],
    ),
    (
        "infrared",
        "Infrared",
        BandpassKind::Broadband,
        &[
            "ir", "nir", "ir-pass", "irpass", "infrared", "ir685", "ir742", "ir807", "ir850",
        ],
    ),
];

fn squash(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Match a filter name to a bandpass. Unknown names get a bandpass of their
/// own, keyed by the cleaned name, so two rigs with the same custom label
/// still agree while nothing is silently folded into a known band.
pub fn bandpass_for_filter(filter_name: &str) -> Bandpass {
    let cleaned = squash(filter_name);
    let stripped: String = cleaned
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .collect();
    // Pass one: exact alias. Pass two: alias plus a bandwidth suffix such as
    // "Ha 3nm" or "OIII-6.5nm" reduces to the alias once digits and "nm" go.
    let candidates = [
        cleaned.clone(),
        stripped.clone(),
        strip_bandwidth(&stripped),
    ];
    for (id, name, kind, aliases) in KNOWN {
        for candidate in &candidates {
            if aliases.iter().any(|alias| {
                alias.replace([' ', '-', '_'], "") == *candidate || *alias == candidate.as_str()
            }) {
                return Bandpass {
                    id: (*id).to_owned(),
                    name,
                    kind: *kind,
                };
            }
        }
    }
    let id: String = stripped
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    Bandpass {
        id: if id.is_empty() {
            "unknown".to_owned()
        } else {
            id
        },
        name: "Custom",
        kind: BandpassKind::Broadband,
    }
}

fn strip_bandwidth(value: &str) -> String {
    let mut out = value.to_owned();
    if let Some(index) = out.find("nm") {
        out.truncate(index);
    }
    while out.ends_with(|c: char| c.is_ascii_digit() || c == '.') && out.len() > 1 {
        out.pop();
    }
    out
}

/// The sky at the rig, as far as exposure length cares.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExposureContext {
    #[serde(deserialize_with = "Option::deserialize")]
    pub focal_ratio: Option<f64>,
    /// Bortle class 1 (darkest) to 9; `None` means unknown, treated as 5.
    #[serde(deserialize_with = "Option::deserialize")]
    pub bortle_class: Option<u8>,
    pub kind: BandpassKind,
}

/// A starting exposure length for a rig and sky, rounded to a tidy value.
/// Longer for slow optics, dark skies and narrowband; shorter under bright
/// skies where the background would otherwise swamp the stars. A default,
/// never a claim about the camera's well depth.
pub fn default_exposure_seconds(context: ExposureContext) -> f64 {
    let base = match context.kind {
        BandpassKind::Broadband => 120.0,
        BandpassKind::Narrowband => 300.0,
    };
    let ratio = context
        .focal_ratio
        .filter(|f| f.is_finite() && *f > 0.0)
        .unwrap_or(5.0);
    let optics = (ratio / 5.0).powi(2).clamp(0.4, 3.0);
    let sky = match context.bortle_class.unwrap_or(5) {
        0..=3 => 1.5,
        4 | 5 => 1.0,
        6 | 7 => 0.6,
        _ => 0.4,
    };
    let raw = (base * optics * sky).clamp(30.0, 900.0);
    tidy(raw)
}

fn tidy(seconds: f64) -> f64 {
    const STEPS: [f64; 13] = [
        30.0, 45.0, 60.0, 90.0, 120.0, 150.0, 180.0, 240.0, 300.0, 360.0, 480.0, 600.0, 900.0,
    ];
    STEPS
        .iter()
        .copied()
        .min_by(|a, b| (a - seconds).abs().total_cmp(&(b - seconds).abs()))
        .unwrap_or(seconds)
}

/// Accepted frames needed to reach a goal with one exposure length.
pub fn frames_for_hours(hours: f64, exposure_seconds: f64) -> Option<u32> {
    if !hours.is_finite()
        || hours <= 0.0
        || !exposure_seconds.is_finite()
        || exposure_seconds <= 0.0
    {
        return None;
    }
    let frames = (hours * 3600.0 / exposure_seconds).ceil();
    (frames <= f64::from(u32::MAX)).then_some(frames as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_names_from_different_rigs_meet_at_one_bandpass() {
        for name in ["Ha", "H-alpha", "Ha 3nm", "HA3", "H_alpha", "Hα"] {
            assert_eq!(bandpass_for_filter(name).id, "h_alpha", "{name}");
            assert_eq!(bandpass_for_filter(name).kind, BandpassKind::Narrowband);
        }
        for name in ["OIII", "O3", "O-III 6.5nm"] {
            assert_eq!(bandpass_for_filter(name).id, "oiii", "{name}");
        }
        assert_eq!(bandpass_for_filter("L").id, "luminance");
        assert_eq!(bandpass_for_filter("Lum").id, "luminance");
        assert_eq!(bandpass_for_filter("R").id, "red");
        assert_eq!(bandpass_for_filter("Red").kind, BandpassKind::Broadband);
        assert_eq!(bandpass_for_filter("L-eXtreme").id, "dual_band");
        // A custom label stays its own bandpass and keeps agreeing with itself.
        let custom = bandpass_for_filter("Baader Solar Continuum");
        assert_eq!(custom.id, "baadersolarcontinuum");
        assert_eq!(custom, bandpass_for_filter("baader solar continuum"));
        assert_ne!(custom.id, bandpass_for_filter("Red").id);
    }

    #[test]
    fn default_exposures_follow_optics_sky_and_band() {
        let at = |focal_ratio: f64, bortle: u8, kind: BandpassKind| {
            default_exposure_seconds(ExposureContext {
                focal_ratio: Some(focal_ratio),
                bortle_class: Some(bortle),
                kind,
            })
        };
        assert_eq!(at(5.0, 4, BandpassKind::Broadband), 120.0);
        assert_eq!(at(5.0, 4, BandpassKind::Narrowband), 300.0);
        assert!(at(2.8, 4, BandpassKind::Broadband) < at(5.0, 4, BandpassKind::Broadband));
        assert!(at(10.0, 4, BandpassKind::Broadband) > at(5.0, 4, BandpassKind::Broadband));
        assert!(at(5.0, 8, BandpassKind::Broadband) < at(5.0, 2, BandpassKind::Broadband));
        assert_eq!(at(5.0, 8, BandpassKind::Broadband), 45.0);
        assert_eq!(at(20.0, 1, BandpassKind::Narrowband), 900.0);
        assert_eq!(
            default_exposure_seconds(ExposureContext {
                focal_ratio: None,
                bortle_class: None,
                kind: BandpassKind::Broadband
            }),
            120.0
        );
    }

    #[test]
    fn hours_become_whole_frames_rounded_up() {
        assert_eq!(frames_for_hours(2.0, 120.0), Some(60));
        assert_eq!(frames_for_hours(1.0, 300.0), Some(12));
        assert_eq!(frames_for_hours(0.5, 300.0), Some(6));
        assert_eq!(frames_for_hours(1.0, 7.0), Some(515));
        assert_eq!(frames_for_hours(0.0, 120.0), None);
        assert_eq!(frames_for_hours(1.0, 0.0), None);
    }
}
