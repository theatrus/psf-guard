//! The Tone and color step of a document: Camera Raw's Basic controls for
//! the finished picture. Seiza's [`Tone`] does the work; this file reads and
//! writes the step's settings.

use seiza_compose::tone::{Tone, CONTROLS};

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
    serde_json::from_value::<Tone>(serde_json::Value::Object(numbers))
        .unwrap_or_default()
        .held()
}

/// The settings a new step starts with: every control at 0.
pub fn new_settings() -> serde_json::Value {
    serde_json::Value::Object(
        CONTROLS
            .iter()
            .map(|(key, ..)| (key.to_string(), serde_json::Value::from(0.0)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_new_step_leaves_the_picture_as_it_is() {
        assert_eq!(from_settings(&new_settings()), Tone::default());
    }

    #[test]
    fn settings_out_of_range_are_held_to_it_and_others_are_zero() {
        let tone = from_settings(&json!({ "exposure": 9.0, "contrast": -4.0, "shadows": "lots" }));
        assert_eq!(tone.exposure, 3.0);
        assert_eq!(tone.contrast, -1.0);
        assert_eq!(tone.shadows, 0.0);
    }
}
