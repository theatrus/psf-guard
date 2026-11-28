//! The Tone and color step of a document: Camera Raw's Basic controls for
//! the finished picture. Seiza's [`Tone`] does the work; this file reads and
//! writes the step's settings.

use seiza_compose::tone::{Tone, CONTROLS, MAX_HEADROOM};

/// The picture's HDR headroom control: how far above SDR white an HDR
/// picture reaches, in stops. The document holds it for the whole picture;
/// every Tone and color step takes it from there.
pub const HEADROOM: (&str, &str, f32, f32) = ("headroom", "Headroom", 0.0, MAX_HEADROOM);

/// The picture's HDR knee control: where each stretch's continuation into
/// the headroom starts, as the stretch's output.
pub const HDR_KNEE: (&str, &str, f32, f32) = ("hdr_knee", "Knee", 0.3, 0.95);

/// The HDR knee of a picture that sets none: three quarters of white.
pub const DEFAULT_HDR_KNEE: f32 = 0.75;

/// The headroom a picture has when HDR is first turned on: two stops, four
/// times SDR white's light.
pub const DEFAULT_HEADROOM: f32 = 2.0;

/// Control `index` of a Tone and color step: one of [`CONTROLS`].
pub fn control(index: usize) -> Option<(&'static str, &'static str, f32, f32)> {
    CONTROLS.get(index).copied()
}

/// The name of the step in a document.
pub const OPERATION: &str = "Tone and color";

/// The step's settings, with 0 for a control they leave out or do not give
/// as a number, and each control held to its range. Whether the picture is
/// HDR is the document's, not the step's.
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
    let settings: serde_json::Map<String, serde_json::Value> = CONTROLS
        .iter()
        .map(|(key, ..)| (key.to_string(), serde_json::Value::from(0.0)))
        .collect();
    serde_json::Value::Object(settings)
}

/// Moves HDR from a Tone and color step to the document. A document made
/// when HDR was a switch of the step becomes an HDR picture with that
/// step's headroom, and its steps lose the switch.
pub fn adopt_headroom(document: &mut crate::studio_model::Document) {
    use crate::studio_model::StepKind;
    let steps: Vec<(uuid::Uuid, serde_json::Value)> = document
        .picture_steps()
        .filter_map(|step| match &step.kind {
            StepKind::Operation {
                operation,
                settings,
                ..
            } if operation == OPERATION && settings.get("hdr").is_some() => {
                Some((step.id, settings.clone()))
            }
            _ => None,
        })
        .collect();
    for (id, mut settings) in steps {
        let on = settings["hdr"].as_bool().unwrap_or(false);
        let headroom = settings[HEADROOM.0]
            .as_f64()
            .map_or(DEFAULT_HEADROOM, |value| value as f32);
        if on && document.headroom.is_none() {
            document.headroom = Some(headroom.clamp(0.0, MAX_HEADROOM));
        }
        if let Some(settings) = settings.as_object_mut() {
            settings.remove("hdr");
            settings.remove(HEADROOM.0);
        }
        document.set_step_settings(id, settings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_new_step_leaves_the_picture_as_it_is() {
        let tone = from_settings(&new_settings());
        assert_eq!(tone, Tone::default());
        assert_eq!(tone.peak(), 1.0);
        assert_eq!(control(CONTROLS.len()), None);
    }

    #[test]
    fn a_step_s_hdr_switch_becomes_the_picture_s_headroom() {
        use crate::image_steps::Accepts;
        use crate::studio_model::{Document, StageKind, StepKind, StepRecord};
        let mut document = Document::new("hdr");
        let step = StepRecord::new(StepKind::Operation {
            operation: OPERATION.to_string(),
            accepts: Accepts::Display,
            settings: json!({ "exposure": 1.0, "hdr": true, "headroom": 3.0 }),
        });
        let id = step.id;
        document.insert_step(StageKind::Finish, 0, step).unwrap();
        adopt_headroom(&mut document);
        assert_eq!(document.headroom, Some(3.0));
        let Some((_, step)) = document.step(id) else {
            panic!("the step is gone");
        };
        let StepKind::Operation { settings, .. } = &step.kind else {
            panic!("not an operation");
        };
        assert_eq!(settings, &json!({ "exposure": 1.0 }));
        // A step whose switch was off leaves the picture SDR.
        let mut sdr = Document::new("sdr");
        let step = StepRecord::new(StepKind::Operation {
            operation: OPERATION.to_string(),
            accepts: Accepts::Display,
            settings: json!({ "hdr": false, "headroom": 2.0 }),
        });
        sdr.insert_step(StageKind::Finish, 0, step).unwrap();
        adopt_headroom(&mut sdr);
        assert_eq!(sdr.headroom, None);
    }

    #[test]
    fn settings_out_of_range_are_held_to_it_and_others_are_zero() {
        let tone = from_settings(&json!({ "exposure": 9.0, "contrast": -4.0, "shadows": "lots" }));
        assert_eq!(tone.exposure, 3.0);
        assert_eq!(tone.contrast, -1.0);
        assert_eq!(tone.shadows, 0.0);
    }
}
