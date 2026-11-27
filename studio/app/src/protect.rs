//! Regions a background extraction keeps out of its samples, drawn on the
//! canvas: a nebula's faint halo, which a background model would otherwise
//! take for sky and subtract.
//!
//! A region is stored in the Background extraction step's settings as Seiza
//! takes it, `config.protected_regions`, in fractions of the image's width
//! and height. The canvas outlines the selected step's regions, and the
//! Protect tool draws an ellipse: press at its centre and drag to its edge.
//! The background is worked out when the store is prepared, so a change is
//! flagged as tools not run until the tools run again.

use super::Studio;
use crate::studio_model::{flow_operations, Row, StepKind};
use gpui::{div, prelude::*, Context};
use seiza_compose::picture::Segment;
use serde_json::{json, Value};
use uuid::Uuid;

/// How many straight pieces an ellipse's outline is drawn with.
const OUTLINE_PIECES: usize = 64;

/// The smallest radius, in document pixels, that makes a region; a click
/// with no drag makes none.
const SMALLEST_RADIUS: f64 = 4.0;

impl Studio {
    /// The Background extraction step the selected row is, if it is one.
    pub(super) fn selected_background_step(&self) -> Option<Uuid> {
        let Row::InputStep { step, .. } = &self.selected else {
            return None;
        };
        let (_, record) = self.document.step(*step)?;
        (record.kind.name() == flow_operations::BACKGROUND).then_some(*step)
    }

    /// The regions a Background extraction step keeps out, as stored.
    fn protected_regions(&self, step: Uuid) -> Vec<Value> {
        let Some((_, record)) = self.document.step(step) else {
            return Vec::new();
        };
        let StepKind::Operation { settings, .. } = &record.kind else {
            return Vec::new();
        };
        settings["config"]["protected_regions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    fn set_protected_regions(&mut self, step: Uuid, regions: Vec<Value>) {
        let Some((_, record)) = self.document.step(step) else {
            return;
        };
        let StepKind::Operation { settings, .. } = &record.kind else {
            return;
        };
        let mut settings = settings.clone();
        settings["config"]["protected_regions"] = Value::Array(regions);
        self.document.set_step_settings(step, settings);
    }

    /// Adds an ellipse centred at `center` that reaches `edge`, both in
    /// document pixels, to the selected Background extraction step. A drag
    /// too short to see makes none.
    pub(super) fn add_protected_ellipse(&mut self, center: [f64; 2], edge: [f64; 2]) {
        let Some(step) = self.selected_background_step() else {
            return;
        };
        let (width, height) = self.canvas.read(|shared| shared.canvas.document_size);
        let (width, height) = (f64::from(width.max(2)), f64::from(height.max(2)));
        let radii = [(edge[0] - center[0]).abs(), (edge[1] - center[1]).abs()];
        if radii[0].max(radii[1]) < SMALLEST_RADIUS {
            return;
        }
        // A drag along one axis makes a circle.
        let radii = [radii[0].max(radii[1] * 0.1), radii[1].max(radii[0] * 0.1)];
        let mut regions = self.protected_regions(step);
        regions.push(json!({
            "kind": "ellipse",
            "center": [
                (center[0] / (width - 1.0)).clamp(0.0, 1.0),
                (center[1] / (height - 1.0)).clamp(0.0, 1.0),
            ],
            "radii": [radii[0] / width, radii[1] / height],
            "rotation_degrees": 0.0,
        }));
        self.set_protected_regions(step, regions);
    }

    /// The outlines of the selected Background extraction step's regions, in
    /// document pixels, `line` wide, for the canvas to show.
    pub(super) fn protected_outlines(&self, line: f32) -> Vec<Segment> {
        let Some(step) = self.selected_background_step() else {
            return Vec::new();
        };
        let (width, height) = self.canvas.read(|shared| shared.canvas.document_size);
        let (width, height) = (width.max(2) as f32, height.max(2) as f32);
        self.protected_regions(step)
            .iter()
            .flat_map(|region| outline(region, width, height, line))
            .collect()
    }

    /// The selected Background extraction step's regions, with a button to
    /// remove each, and how to draw one.
    pub(super) fn protect_block(&self, step: Uuid, cx: &mut Context<Self>) -> gpui::Div {
        let regions = self.protected_regions(step);
        let (width, height) = self.canvas.read(|shared| shared.canvas.document_size);
        let rows = regions.iter().enumerate().map(|(index, region)| {
            let words = describe(region, width as f32, height as f32);
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().text_xs().child(words))
                .child(Self::act(
                    "Remove",
                    ("protected-remove", index),
                    false,
                    cx,
                    move |this| {
                        let mut regions = this.protected_regions(step);
                        if index < regions.len() {
                            regions.remove(index);
                            this.set_protected_regions(step, regions);
                        }
                    },
                ))
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Self::heading("Protected regions"))
            .when(regions.is_empty(), |block| {
                block.child(Self::note("None drawn."))
            })
            .children(rows)
            .child(Self::act(
                "Draw a region",
                "protected-draw",
                self.tool == super::Tool::Protect,
                cx,
                |this| this.tool = super::Tool::Protect,
            ))
            .children(Self::help(
                "The background is measured outside these regions, so a faint halo is not taken for sky. With Protect, press at a region's centre and drag out: the drag sets its width and height. The canvas outlines them while this step is selected. The change takes effect when the tools run.",
            ))
    }
}

/// A region's outline in document pixels, for a region being drawn.
pub(super) fn outline_of(region: &Value, width: f32, height: f32, line: f32) -> Vec<Segment> {
    outline(region, width, height, line)
}

/// A region's outline in document pixels: an ellipse's, or a polygon's.
fn outline(region: &Value, width: f32, height: f32, line: f32) -> Vec<Segment> {
    let point = |value: &Value| -> Option<[f32; 2]> {
        Some([
            value.get(0)?.as_f64()? as f32 * (width - 1.0),
            value.get(1)?.as_f64()? as f32 * (height - 1.0),
        ])
    };
    let corners: Vec<[f32; 2]> = match region["kind"].as_str() {
        Some("ellipse") => {
            let (Some(center), Some(radii)) =
                (point(&region["center"]), region["radii"].as_array())
            else {
                return Vec::new();
            };
            let radius = |at: usize, scale: f32| {
                radii.get(at).and_then(Value::as_f64).unwrap_or(0.0) as f32 * scale
            };
            let (rx, ry) = (radius(0, width), radius(1, height));
            let turn = (region["rotation_degrees"].as_f64().unwrap_or(0.0) as f32).to_radians();
            (0..OUTLINE_PIECES)
                .map(|piece| {
                    let angle = piece as f32 / OUTLINE_PIECES as f32 * std::f32::consts::TAU;
                    let (x, y) = (rx * angle.cos(), ry * angle.sin());
                    [
                        center[0] + x * turn.cos() - y * turn.sin(),
                        center[1] + x * turn.sin() + y * turn.cos(),
                    ]
                })
                .collect()
        }
        Some("polygon") => region["points"]
            .as_array()
            .map(|points| points.iter().filter_map(point).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    (0..corners.len())
        .map(|at| Segment {
            from: corners[at],
            to: corners[(at + 1) % corners.len()],
            radius: line,
            hardness: 1.0,
            value: 1.0,
        })
        .collect()
}

/// A region in words: where it is and how large, in pixels.
fn describe(region: &Value, width: f32, height: f32) -> String {
    let number =
        |value: &Value, at: usize| value.get(at).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    match region["kind"].as_str() {
        Some("ellipse") => {
            let (center, radii) = (&region["center"], &region["radii"]);
            format!(
                "Ellipse at {:.0}, {:.0}, {:.0} by {:.0} px",
                number(center, 0) * (width - 1.0),
                number(center, 1) * (height - 1.0),
                2.0 * number(radii, 0) * width,
                2.0 * number(radii, 1) * height,
            )
        }
        Some("polygon") => format!(
            "Outline of {} points",
            region["points"].as_array().map_or(0, Vec::len)
        ),
        _ => "A region Studio does not show".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ellipse_is_outlined_and_described_in_pixels() {
        let region = json!({
            "kind": "ellipse",
            "center": [0.5, 0.5],
            "radii": [0.1, 0.2],
            "rotation_degrees": 0.0,
        });
        let pieces = outline(&region, 1001.0, 501.0, 1.0);
        assert_eq!(pieces.len(), OUTLINE_PIECES);
        // The first point is the right end of the long axis: centre plus
        // the x radius.
        assert!(
            (pieces[0].from[0] - (500.0 + 100.1)).abs() < 0.01,
            "{:?}",
            pieces[0].from
        );
        assert!((pieces[0].from[1] - 250.0).abs() < 0.01);
        assert_eq!(
            describe(&region, 1001.0, 501.0),
            "Ellipse at 500, 250, 200 by 200 px"
        );
        assert!(outline(&json!({ "kind": "unknown" }), 10.0, 10.0, 1.0).is_empty());
    }
}
