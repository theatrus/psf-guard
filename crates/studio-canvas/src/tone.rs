//! The Tone and color step: Camera Raw's Basic controls for the finished
//! picture.
//!
//! Exposure, Contrast, Highlights, Shadows, Whites and Blacks make one curve
//! of the luminance. The canvas, the histogram and an export all apply it
//! from one stretch-table row, which [`curve_row`] fills, so they agree.
//! Vibrance and Saturation are applied per pixel after the curve; the
//! Finish shader and the CPU render each do that, with the same formula.

use crate::cpu::contrast_curve;
use crate::STRETCH_TABLE_WIDTH;

/// The name of the step in a document.
pub const OPERATION: &str = "Tone and color";

/// The settings of a Tone and color step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tone {
    /// Stops, from -3 to 3.
    pub exposure: f32,
    /// From -1 to 1, as every control below.
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub vibrance: f32,
    pub saturation: f32,
}

/// Each control: its key in the step's settings, its label, and its range.
pub const CONTROLS: [(&str, &str, f32, f32); 8] = [
    ("exposure", "Exposure", -3.0, 3.0),
    ("contrast", "Contrast", -1.0, 1.0),
    ("highlights", "Highlights", -1.0, 1.0),
    ("shadows", "Shadows", -1.0, 1.0),
    ("whites", "Whites", -1.0, 1.0),
    ("blacks", "Blacks", -1.0, 1.0),
    ("vibrance", "Vibrance", -1.0, 1.0),
    ("saturation", "Saturation", -1.0, 1.0),
];

/// How far Highlights or Shadows at 1 moves the tone it changes most. Any
/// larger and the two together could make the curve turn back.
const SHIFT: f32 = 0.14;
/// How far Whites at 1 brings the white point down.
const WHITE_REACH: f32 = 0.3;
/// How far Blacks at -1 brings the black point up.
const BLACK_REACH: f32 = 0.1;

impl Tone {
    /// The step's settings, with 0 for any control they leave out.
    pub fn from_settings(settings: &serde_json::Value) -> Self {
        let value = |key: &str, low: f32, high: f32| {
            settings
                .get(key)
                .and_then(serde_json::Value::as_f64)
                .map_or(0.0, |value| (value as f32).clamp(low, high))
        };
        let [exposure, contrast, highlights, shadows, whites, blacks, vibrance, saturation] =
            CONTROLS.map(|(key, _, low, high)| value(key, low, high));
        Self {
            exposure,
            contrast,
            highlights,
            shadows,
            whites,
            blacks,
            vibrance,
            saturation,
        }
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

    /// The luminance curve at `x`, from 0 to 1.
    pub fn curve(&self, x: f32) -> f32 {
        let mut y = x.max(0.0) * self.exposure.exp2();
        // Whites and Blacks move the white and black points.
        let white = 1.0 - WHITE_REACH * self.whites;
        let black = -BLACK_REACH * self.blacks;
        y = ((y - black) / (white - black)).clamp(0.0, 1.0);
        // Shadows and Highlights move the tones a third and two thirds of
        // the way up most; 6.75 makes each bump 1 at its peak.
        y += SHIFT * self.shadows * 6.75 * y * (1.0 - y) * (1.0 - y);
        y += SHIFT * self.highlights * 6.75 * y * y * (1.0 - y);
        contrast_curve(y.clamp(0.0, 1.0), self.contrast)
    }
}

/// The step's curve as a row of the stretch table: entry i is the curve at
/// (i / (width - 1)) squared, as every row is indexed.
pub fn curve_row(tone: &Tone) -> Vec<f32> {
    let last = (STRETCH_TABLE_WIDTH - 1) as f32;
    (0..STRETCH_TABLE_WIDTH)
        .map(|index| tone.curve((index as f32 / last).powi(2)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn controls_at_zero_leave_the_picture_as_it_is() {
        let tone = Tone::from_settings(&Tone::new_settings());
        assert_eq!(tone, Tone::default());
        for x in [0.0_f32, 0.01, 0.2, 0.5, 0.9, 1.0] {
            assert!((tone.curve(x) - x).abs() < 1e-6, "{x}");
        }
    }

    #[test]
    fn each_control_moves_the_tones_it_names() {
        let at = |settings: serde_json::Value, x: f32| Tone::from_settings(&settings).curve(x);
        assert!((at(json!({ "exposure": 1.0 }), 0.2) - 0.4).abs() < 1e-6);
        assert!(at(json!({ "shadows": 1.0 }), 0.3) > 0.35);
        assert!((at(json!({ "shadows": 1.0 }), 0.9) - 0.9).abs() < 0.02);
        assert!(at(json!({ "highlights": -1.0 }), 0.7) < 0.65);
        assert!((at(json!({ "highlights": -1.0 }), 0.1) - 0.1).abs() < 0.02);
        // Whites up clips the brightest tones; Blacks down clips the darkest.
        assert_eq!(at(json!({ "whites": 1.0 }), 0.75), 1.0);
        assert_eq!(at(json!({ "blacks": -1.0 }), 0.05), 0.0);
        // Contrast darkens below the middle and brightens above it.
        assert!(at(json!({ "contrast": 1.0 }), 0.25) < 0.25);
        assert!(at(json!({ "contrast": 1.0 }), 0.75) > 0.75);
    }

    #[test]
    fn the_curve_never_turns_back() {
        for values in [
            [1.0, 1.0, 1.0, 1.0],
            [-1.0, -1.0, -1.0, -1.0],
            [1.0, -1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0, 1.0],
        ] {
            let [highlights, shadows, whites, blacks] = values;
            let tone = Tone {
                highlights,
                shadows,
                whites,
                blacks,
                contrast: values[0],
                ..Tone::default()
            };
            let row = curve_row(&tone);
            for pair in row.windows(2) {
                assert!(pair[1] >= pair[0] - 1e-6, "{values:?}");
            }
        }
    }

    #[test]
    fn settings_out_of_range_are_held_to_it() {
        let tone = Tone::from_settings(&json!({ "exposure": 9.0, "contrast": -4.0 }));
        assert_eq!(tone.exposure, 3.0);
        assert_eq!(tone.contrast, -1.0);
    }
}
