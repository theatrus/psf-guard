//! The Tone and color step of a document: Camera Raw's Basic controls for
//! the finished picture. Seiza's [`Tone`] does the work; this file reads and
//! writes the step's settings.

use seiza_compose::tone::{Tone, CONTROLS, MAX_HEADROOM};

/// The HDR headroom's control: how far above SDR white an HDR picture
/// reaches, in stops. A slider after [`CONTROLS`].
pub const HEADROOM: (&str, &str, f32, f32) = ("headroom", "Headroom", 0.0, MAX_HEADROOM);

/// The headroom a step has before a person sets one: two stops, four times
/// SDR white.
const DEFAULT_HEADROOM: f32 = 2.0;

/// Control `index` of a Tone and color step: one of [`CONTROLS`], then the
/// HDR headroom.
pub fn control(index: usize) -> Option<(&'static str, &'static str, f32, f32)> {
    CONTROLS
        .get(index)
        .copied()
        .or((index == CONTROLS.len()).then_some(HEADROOM))
}

/// The name of the step in a document.
pub const OPERATION: &str = "Tone and color";

/// The step's settings, with 0 for a control they leave out or do not give
/// as a number, and each control held to its range.
pub fn from_settings(settings: &serde_json::Value) -> Tone {
    let numbers: serde_json::Map<String, serde_json::Value> = CONTROLS
        .iter()
        .filter_map(|(key, ..)| {
            let value = settings.get(*key)?.as_f64()?;
            Some((key.to_string(), serde_json::Value::from(value)))
        })
        .collect();
    let mut tone =
        serde_json::from_value::<Tone>(serde_json::Value::Object(numbers)).unwrap_or_default();
    tone.hdr = settings
        .get("hdr")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    tone.headroom = settings
        .get(HEADROOM.0)
        .and_then(serde_json::Value::as_f64)
        .map_or(DEFAULT_HEADROOM, |value| value as f32);
    tone.held()
}

/// The settings a new step starts with: every control at 0.
pub fn new_settings() -> serde_json::Value {
    let mut settings: serde_json::Map<String, serde_json::Value> = CONTROLS
        .iter()
        .map(|(key, ..)| (key.to_string(), serde_json::Value::from(0.0)))
        .collect();
    settings.insert("hdr".into(), false.into());
    settings.insert(HEADROOM.0.into(), DEFAULT_HEADROOM.into());
    serde_json::Value::Object(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_new_step_leaves_the_picture_as_it_is() {
        let tone = from_settings(&new_settings());
        assert_eq!(
            tone,
            Tone {
                headroom: 2.0,
                ..Tone::default()
            }
        );
        assert_eq!(tone.peak(), 1.0);
    }

    #[test]
    fn hdr_and_its_headroom_are_read() {
        let tone = from_settings(&json!({ "hdr": true, "headroom": 3.0 }));
        assert!(tone.hdr);
        assert_eq!(tone.headroom, 3.0);
        assert_eq!(from_settings(&json!({ "hdr": true })).headroom, 2.0);
        assert_eq!(control(CONTROLS.len()), Some(HEADROOM));
        assert_eq!(control(CONTROLS.len() + 1), None);
    }

    #[test]
    fn settings_out_of_range_are_held_to_it_and_others_are_zero() {
        let tone = from_settings(&json!({ "exposure": 9.0, "contrast": -4.0, "shadows": "lots" }));
        assert_eq!(tone.exposure, 3.0);
        assert_eq!(tone.contrast, -1.0);
        assert_eq!(tone.shadows, 0.0);
    }
}
