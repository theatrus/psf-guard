//! The staged workflow's panes: a rail of stages across the top, the showing
//! stage's steps or layers on the left, the canvas in the middle, and the
//! stage's own controls on the right.

use super::{
    Drag, Scene, SliderId, Studio, Tool, BAND_COLORS, BAND_NAMES, PANEL_WIDTH, STEPS_WIDTH,
};
use crate::image_steps::Accepts;
use crate::studio_model::{
    document_from_color_setup, level_for_zoom, BlendMode, Document, JunctionStatus, LayerSource,
    MaskRecord, StageKind, StepKind, StepRecord, Stroke, StrokeMode,
};
use gpui::{
    canvas, div, prelude::*, px, relative, rgb, AnyElement, Context, MouseButton, MouseDownEvent,
    Rgba, Window,
};
use std::sync::Arc;
use studio_render_spike::{
    Blend, FinishKind, FinishStep, Layer, Segment, MAX_FINISH_STEPS, MAX_LAYERS, MAX_MASKS,
};

const BACKGROUND: u32 = 0x16181c;
const PANE: u32 = 0x1e2126;
const CONTROL: u32 = 0x2a2e34;
const SELECTED: u32 = 0x3b6ea8;
const TEXT: u32 = 0xd8dee6;
const QUIET: u32 = 0x8a93a0;

/// The steps a reduced view leaves out, because their result depends on
/// pixel scale. The engine reports these per step; the demo names them.
const FULL_RESOLUTION_ONLY: [&str; 2] = ["Deconvolution", "Noise reduction"];

/// The operations offered in each stage, with the samples each works on.
fn offered(stage: StageKind) -> &'static [(&'static str, Accepts)] {
    match stage {
        StageKind::Sources | StageKind::Export | StageKind::Combine => &[],
        StageKind::Linear => &[
            ("Background", Accepts::Linear),
            ("Deconvolution", Accepts::Linear),
            ("Noise reduction", Accepts::Linear),
            ("Star separation", Accepts::Either),
        ],
        StageKind::Stretch => &[("Stretch", Accepts::Linear)],
        StageKind::Finish => &[
            ("Curves", Accepts::Display),
            ("Saturation", Accepts::Display),
            ("Local contrast", Accepts::Display),
        ],
    }
}

fn stage_title(stage: StageKind) -> &'static str {
    match stage {
        StageKind::Sources => "Sources",
        StageKind::Linear => "Linear",
        StageKind::Stretch => "Stretch",
        StageKind::Combine => "Combine",
        StageKind::Finish => "Finish",
        StageKind::Export => "Export",
    }
}

fn stage_help(stage: StageKind) -> &'static str {
    match stage {
        StageKind::Sources => "Choose stacks or masters and name their bands.",
        StageKind::Linear => "Work on linear data, before any stretch.",
        StageKind::Stretch => "Take each band from linear data to a display image.",
        StageKind::Combine => "Edit each layer on its own, then blend the layers.",
        StageKind::Finish => "Adjust the combined image as a whole.",
        StageKind::Export => "Choose the color space and write the file.",
    }
}

fn operation(name: &str, accepts: Accepts) -> StepRecord {
    StepRecord::new(StepKind::Operation {
        operation: name.to_string(),
        accepts,
        settings: serde_json::Value::Null,
    })
}

/// A name and a color for each band of a document, up to the layers one
/// frame draws: the eight synthetic bands, red, green and blue for three,
/// and gray for one.
pub(super) fn band_labels(bands: usize) -> Vec<(String, [f32; 3])> {
    let bright = |color: [f32; 3]| color.map(|channel| (channel * 2.0).min(1.0));
    match bands {
        1 => vec![("L".to_string(), [1.0; 3])],
        3 => ["R", "G", "B"]
            .iter()
            .enumerate()
            .map(|(index, name)| (name.to_string(), bright(BAND_COLORS[index + 1])))
            .collect(),
        _ => (0..bands.min(MAX_LAYERS))
            .map(|index| match BAND_NAMES.get(index) {
                Some(name) if bands == BAND_NAMES.len() => {
                    (name.to_string(), bright(BAND_COLORS[index]))
                }
                _ => (
                    format!("Band {}", index + 1),
                    bright(BAND_COLORS[index % 8]),
                ),
            })
            .collect(),
    }
}

/// The name PSF Guard's settings give a band's channel, such as `ha` for
/// `Ha`. A band PSF Guard has no role for, such as NII, keeps its own name in
/// lower case.
pub(super) fn channel_name(band: &str) -> String {
    match band {
        "L" => "luminance".to_string(),
        "R" => "red".to_string(),
        "G" => "green".to_string(),
        "B" => "blue".to_string(),
        other => other.to_lowercase(),
    }
}

/// PSF Guard's built-in default color setup.
const DEFAULT_COLOR_SETUP: &str =
    include_str!("../../../../src/processing_setups/default_color.json");

/// A new document: PSF Guard's default color flow over the bands. The steps
/// and their settings are the ones PSF Guard's color previews apply. The
/// layers get a color each, which the settings leave to the palette.
pub(super) fn flow_document(bands: usize) -> Document {
    let labels = band_labels(bands);
    let channels: Vec<String> = labels.iter().map(|(name, _)| channel_name(name)).collect();
    let channels: Vec<&str> = channels.iter().map(String::as_str).collect();
    let settings = serde_json::from_str(DEFAULT_COLOR_SETUP).expect("the default setup is JSON");
    let mut document = document_from_color_setup("PSF Guard default color", &settings, &channels)
        .expect("the default setup makes a document");
    // The layers add together, so their colors are scaled until the
    // brightest channel of their sum is full scale.
    let brightest = (0..3)
        .map(|channel| labels.iter().map(|(_, color)| color[channel]).sum::<f32>())
        .fold(f32::MIN_POSITIVE, f32::max);
    let ids: Vec<_> = document.layers().iter().map(|layer| layer.id).collect();
    for (id, (name, color)) in ids.into_iter().zip(labels) {
        let layer = document.layer_mut(id).expect("the layer was just listed");
        layer.name = name;
        layer.blend = BlendMode::Add;
        layer.color = Some(color.map(|channel| channel / brightest));
    }
    // Two adjustments PSF Guard's flow does not have, so the Finish stage
    // has something to draw. They stay out of PSF Guard's settings.
    for name in ["Curves", "Saturation"] {
        document
            .insert_step(
                StageKind::Finish,
                usize::MAX,
                operation(name, Accepts::Display),
            )
            .expect("a Finish step fits its stage");
    }
    document
}

/// A count with its noun: "1 step", "3 steps".
fn counted(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn swatch(color: [f32; 3], alpha: f32) -> Rgba {
    Rgba {
        r: color[0],
        g: color[1],
        b: color[2],
        a: alpha,
    }
}

/// The range of an operation's amount, for the operations the canvas draws.
pub(super) fn amount_range(operation: &str) -> Option<(f32, f32)> {
    match operation {
        "Curves" => Some((-1.0, 1.0)),
        "Saturation" => Some((0.0, 2.0)),
        _ => None,
    }
}

/// An operation's amount, or the amount a new one starts with.
fn amount(step: &StepRecord) -> f32 {
    let StepKind::Operation {
        operation,
        settings,
        ..
    } = &step.kind
    else {
        return 0.0;
    };
    settings
        .get("amount")
        .and_then(serde_json::Value::as_f64)
        .map_or(
            if operation == "Saturation" { 1.4 } else { 0.5 },
            |amount| amount as f32,
        )
}

/// Whether the canvas draws a step, or only keeps it in the document.
fn is_drawn(stage: StageKind, step: &StepRecord) -> bool {
    match (&step.kind, stage) {
        (StepKind::Heal { .. }, StageKind::Finish) => true,
        (StepKind::Operation { operation, .. }, StageKind::Finish) => {
            amount_range(operation).is_some()
        }
        (StepKind::Operation { operation, .. }, StageKind::Stretch) => operation == "Stretch",
        _ => false,
    }
}

/// A stroke's path as the segments the canvas draws.
fn segments(stroke: &Stroke) -> Vec<Segment> {
    let segment = |from: [f32; 2], to: [f32; 2]| Segment {
        from,
        to,
        radius: stroke.radius,
        hardness: stroke.hardness,
        value: if stroke.mode == StrokeMode::Add {
            1.0
        } else {
            0.0
        },
        _pad: 0.0,
    };
    match stroke.points.as_slice() {
        [] => Vec::new(),
        [only] => vec![segment(*only, *only)],
        points => points
            .windows(2)
            .map(|pair| segment(pair[0], pair[1]))
            .collect(),
    }
}

/// The strokes that limit a step: its painted mask, or a heal's own stroke.
fn step_strokes(step: &StepRecord) -> Vec<Segment> {
    match (&step.kind, &step.mask) {
        (StepKind::Heal { stroke, .. }, _) => segments(stroke),
        (_, Some(MaskRecord::Painted(mask))) => mask.strokes.iter().flat_map(segments).collect(),
        _ => Vec::new(),
    }
}

/// What the document asks the canvas to draw.
fn scene(document: &Document, bands: usize, version: u64) -> Scene {
    let labels = band_labels(bands);
    let layers = document
        .layers()
        .iter()
        .take(MAX_LAYERS)
        .filter_map(|layer| {
            let LayerSource::Band { band } = &layer.source else {
                // A layer made from a step's result needs the engine.
                return None;
            };
            let band = labels
                .iter()
                .position(|(name, _)| name == band || &channel_name(name) == band)?;
            // A layer's own Curves steps add up to one curve.
            let curve = layer
                .steps
                .iter()
                .filter(|step| step.enabled && step.kind.name() == "Curves")
                .map(amount)
                .sum();
            Some(Layer {
                band,
                color: layer.color.unwrap_or([1.0; 3]),
                opacity: if layer.visible { layer.opacity } else { 0.0 },
                blend: match layer.blend {
                    BlendMode::Normal => Blend::Normal,
                    BlendMode::Screen => Blend::Screen,
                    BlendMode::Add => Blend::Add,
                    BlendMode::Multiply => Blend::Multiply,
                    BlendMode::Lighten => Blend::Lighten,
                    BlendMode::Darken => Blend::Darken,
                },
                curve,
            })
        })
        .collect();

    let mut masks: Vec<Vec<Segment>> = Vec::new();
    let mut finish = Vec::new();
    for step in &document.stage(StageKind::Finish).steps {
        if !step.enabled || !is_drawn(StageKind::Finish, step) || finish.len() == MAX_FINISH_STEPS {
            continue;
        }
        let strokes = step_strokes(step);
        let needs_mask = !strokes.is_empty() || matches!(step.kind, StepKind::Heal { .. });
        let mask = if needs_mask {
            // A step whose mask does not fit is left out, so it is never
            // drawn over the whole image by mistake.
            if masks.len() == MAX_MASKS {
                continue;
            }
            masks.push(strokes);
            Some(masks.len() - 1)
        } else {
            None
        };
        let kind = match &step.kind {
            StepKind::Heal { offset, shift, .. } => FinishKind::Heal {
                offset: [offset[0] as f32, offset[1] as f32],
                shift: std::array::from_fn(|channel| shift.get(channel).copied().unwrap_or(0.0)),
            },
            StepKind::Operation { operation, .. } if operation == "Saturation" => {
                FinishKind::Saturation
            }
            StepKind::Operation { .. } => FinishKind::Curves,
        };
        finish.push(FinishStep {
            kind,
            amount: amount(step),
            mask,
        });
    }
    Scene {
        layers,
        finish,
        masks,
        version,
    }
}

impl Studio {
    /// Ends a brush stroke: a mask stroke joins the selected step's mask,
    /// and a heal stroke becomes a step of the showing stage.
    pub(super) fn commit_stroke(&mut self, stroke: Stroke) {
        match self.tool {
            Tool::Pan => {}
            Tool::Brush => {
                if let Some(id) = self.selected_step {
                    let _ = self.document.paint_step_mask(id, stroke);
                }
            }
            Tool::Heal => {
                // Copy from three radii to the right until a person can
                // choose the source. The brightness shift is measured by the
                // engine from the image, which the canvas cannot read yet,
                // so the copy is not matched to its surroundings.
                let offset = [(stroke.radius * 3.0) as i64, 0];
                let step = StepRecord::new(StepKind::Heal {
                    stroke,
                    offset,
                    shift: Vec::new(),
                });
                let id = step.id;
                if self
                    .document
                    .insert_step(self.stage, usize::MAX, step)
                    .is_ok()
                {
                    self.selected_step = Some(id);
                }
            }
        }
        self.refresh_canvas();
    }

    /// Sends the canvas what the document now asks it to draw.
    pub(super) fn refresh_canvas(&mut self) {
        self.scene_version += 1;
        let scene = Arc::new(scene(&self.document, self.bands, self.scene_version));
        // While a brush is in hand, the selected step's mask shows over the
        // image, so a person sees what they have painted.
        let overlay = if self.tool == Tool::Pan {
            Vec::new()
        } else {
            self.selected_step
                .and_then(|id| self.document.step(id))
                .map(|(_, step)| step_strokes(step))
                .unwrap_or_default()
        };
        self.canvas.edit(|canvas| {
            canvas.scene = scene;
            canvas.strokes = overlay;
        });
    }

    fn show_stage(&mut self, stage: StageKind) {
        self.stage = stage;
        self.selected_step = self.document.stage(stage).steps.first().map(|step| step.id);
        if !stage.allows_manual_edits() {
            self.tool = Tool::Pan;
        }
        self.refresh_canvas();
    }

    /// A text button. `on` draws it as the selected choice.
    fn button(
        label: impl Into<gpui::SharedString>,
        id: impl Into<gpui::ElementId>,
        on: bool,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .px_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .bg(rgb(if on { SELECTED } else { CONTROL }))
            .child(label.into())
    }

    fn heading(text: &'static str) -> gpui::Div {
        div().pt_2().text_color(rgb(QUIET)).text_xs().child(text)
    }

    fn slider(&self, id: SliderId, value: f32, cx: &mut Context<Self>) -> AnyElement {
        let bounds = self.slider_bounds[&id].clone();
        div()
            .flex_1()
            .h(px(20.0))
            .relative()
            .child(
                canvas(move |measured, _, _| bounds.set(measured), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top(px(9.0))
                    .h(px(2.0))
                    .w_full()
                    .bg(rgb(0x4a4f57)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(3.0))
                    .left(relative(value * 0.96))
                    .w(px(8.0))
                    .h(px(14.0))
                    .rounded_sm()
                    .bg(rgb(TEXT)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.drag = Some(Drag::Slider(id));
                    this.set_slider(id, event.position.x);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    // ---- the rail ------------------------------------------------------

    fn stage_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let stages = StageKind::ALL
            .into_iter()
            .enumerate()
            .map(|(index, stage)| {
                let record = self.document.stage(stage);
                let count = if stage == StageKind::Combine {
                    counted(record.layers.len(), "layer")
                } else {
                    counted(record.steps.len(), "step")
                };
                let edited = record.junction.edited_file().is_some();
                Self::button(stage_title(stage), ("stage", index), stage == self.stage)
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_xs().text_color(rgb(QUIET)).child(count))
                    .when(edited, |button| {
                        button.child(div().text_xs().child("edited"))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.show_stage(stage);
                        cx.notify();
                    }))
            });
        div()
            .flex_none()
            .h(px(48.0))
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .bg(rgb(PANE))
            .child(div().pr_3().child(self.document.name.clone()))
            .children(stages)
    }

    // ---- the left pane: steps or layers ---------------------------------

    fn step_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let stage = self.stage;
        self.document
            .stage(stage)
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let id = step.id;
                let mask = match &step.mask {
                    Some(MaskRecord::Painted(mask)) => {
                        format!("painted mask, {}", counted(mask.strokes.len(), "stroke"))
                    }
                    Some(MaskRecord::Range { .. }) => "range mask".to_string(),
                    None => "everywhere".to_string(),
                };
                let drawn = if is_drawn(stage, step) {
                    "drawn"
                } else {
                    "recorded, not drawn yet"
                };
                let mask = format!("{mask} · {drawn}");
                div()
                    .id(("step", index))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(rgb(if self.selected_step == Some(id) {
                        SELECTED
                    } else {
                        CONTROL
                    }))
                    .child(match &step.band {
                        Some(band) => format!("{}. {} · {band}", index + 1, step.kind.name()),
                        None => format!("{}. {}", index + 1, step.kind.name()),
                    })
                    .child(div().text_xs().text_color(rgb(QUIET)).child(mask))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_step = Some(id);
                        this.refresh_canvas();
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect()
    }

    fn layer_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        // The top layer is listed first, as layer lists are read.
        self.document
            .layers()
            .iter()
            .enumerate()
            .rev()
            .map(|(index, layer)| {
                let color = layer.color.unwrap_or([0.6; 3]);
                div()
                    .id(("layer", index))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(rgb(if self.selected_layer == index {
                        SELECTED
                    } else {
                        CONTROL
                    }))
                    .child(
                        div()
                            .w(px(12.0))
                            .h(px(12.0))
                            .rounded_sm()
                            .bg(swatch(color, if layer.visible { 1.0 } else { 0.2 })),
                    )
                    .child(div().flex_1().child(layer.name.clone()))
                    .child(div().text_xs().text_color(rgb(QUIET)).child(format!(
                        "{} · {:.0}%{}",
                        layer.blend,
                        layer.opacity * 100.0,
                        if layer.visible { "" } else { " · hidden" }
                    )))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_layer = index;
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect()
    }

    fn junction_block(&self) -> impl IntoElement {
        // The key for everything before the junction comes from the engine's
        // cache keys, which the canvas does not use yet.
        let status = match self.document.stage(self.stage).junction.status("") {
            JunctionStatus::Computed => "Later stages use this stage's own result.",
            JunctionStatus::WaitingForEdit => "A file is open in an external editor.",
            JunctionStatus::Edited => "Later stages use the file edited outside Studio.",
            JunctionStatus::EditedFromOlderResult => {
                "The edited file was made from an older result of this stage."
            }
            JunctionStatus::WaitingOnOlderResult => {
                "The file in the external editor was made from an older result."
            }
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Self::heading("End of this stage"))
            .child(div().text_xs().child(status))
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(CONTROL))
                    .text_color(rgb(QUIET))
                    .child("Open in external editor"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(QUIET))
                    .child("Disabled: Studio cannot write the file for an editor yet."),
            )
    }

    fn left_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let stage = self.stage;
        let rows = if stage == StageKind::Combine {
            self.layer_rows(cx)
        } else {
            self.step_rows(cx)
        };
        let empty = rows.is_empty();
        // The selected step's amount, for the steps the canvas draws.
        let amount_control = self
            .selected_step
            .and_then(|id| self.document.step(id))
            .filter(|(at, _)| *at == stage)
            .and_then(|(_, step)| {
                let (low, high) = amount_range(step.kind.name())?;
                let value = amount(step);
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .child(Self::heading("Selected step"))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .w(px(96.0))
                                        .text_xs()
                                        .child(format!("{} {value:.2}", step.kind.name())),
                                )
                                .child(self.slider(
                                    SliderId::Amount,
                                    (value - low) / (high - low),
                                    cx,
                                )),
                        ),
                )
            });
        let add = offered(stage)
            .iter()
            .enumerate()
            .map(|(index, (name, accepts))| {
                let (name, accepts) = (*name, *accepts);
                Self::button(format!("Add {name}"), ("add", index), false).on_click(cx.listener(
                    move |this, _, _, cx| {
                        let step = operation(name, accepts);
                        let id = step.id;
                        if this
                            .document
                            .insert_step(this.stage, usize::MAX, step)
                            .is_ok()
                        {
                            this.selected_step = Some(id);
                        }
                        this.refresh_canvas();
                        cx.notify();
                    },
                ))
            });
        div()
            .flex_none()
            .w(px(STEPS_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .bg(rgb(PANE))
            .child(stage_title(stage))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(QUIET))
                    .child(stage_help(stage)),
            )
            .child(Self::heading(if stage == StageKind::Combine {
                "Layers, top first"
            } else {
                "Steps, in order"
            }))
            .children(rows)
            .when(empty, |pane| {
                pane.child(div().text_xs().text_color(rgb(QUIET)).child("None yet."))
            })
            .children(amount_control)
            .child(div().flex().flex_wrap().gap_1().pt_1().children(add))
            .child(div().flex_1())
            .child(self.junction_block())
    }

    // ---- the right pane: the stage's own controls -----------------------

    fn tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tool = |tool: Tool, label: &'static str| {
            Self::button(label, label, self.tool == tool).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.tool = tool;
                    this.refresh_canvas();
                    cx.notify();
                },
            ))
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Self::heading("Tools"))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(tool(Tool::Pan, "Pan"))
                    .child(tool(Tool::Brush, "Mask brush"))
                    .child(tool(Tool::Heal, "Heal")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(QUIET))
                    .child(match self.tool {
                        Tool::Pan => "Drag to move the view.",
                        Tool::Brush => "Drag to paint the selected step's mask.",
                        Tool::Heal => "Drag over a blemish. Each stroke becomes a Heal step.",
                    }),
            )
            .child(
                Self::button("Clear strokes shown", "clear", false).on_click(
                    cx.listener(|this, _, _, _| this.canvas.edit(|canvas| canvas.strokes.clear())),
                ),
            )
    }

    fn band_rows(
        &self,
        slider: Option<fn(usize) -> SliderId>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        band_labels(self.bands)
            .into_iter()
            .enumerate()
            .map(|(band, (name, color))| {
                let (midtone, enabled) = self
                    .canvas
                    .read(|shared| (shared.canvas.sliders[band], true));
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(52.0))
                            .px_1()
                            .rounded_sm()
                            .bg(swatch(color, if enabled { 1.0 } else { 0.25 }))
                            .text_color(rgb(0xffffff))
                            .child(name),
                    )
                    .when_some(slider, |row, slider| {
                        row.child(self.slider(slider(band), midtone, cx))
                    })
                    .into_any_element()
            })
            .collect()
    }

    fn linear_notes(&self) -> impl IntoElement {
        let zoom = self.canvas.read(|shared| shared.canvas.view.zoom);
        let level = level_for_zoom(zoom, self.levels);
        let omitted: Vec<&str> = self
            .document
            .stage(StageKind::Linear)
            .steps
            .iter()
            .map(|step| step.kind.name())
            .filter(|name| level > 0 && FULL_RESOLUTION_ONLY.contains(name))
            .collect();
        let note = if omitted.is_empty() {
            "The view shows every step.".to_string()
        } else {
            format!(
                "At this zoom the view leaves out: {}. Zoom to 100% or use a crop preview to see them.",
                omitted.join(", ")
            )
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Self::heading("What the view shows"))
            .child(
                div()
                    .text_xs()
                    .child(format!("Zoom {:.0}%, pyramid level {level}", zoom * 100.0)),
            )
            .child(div().text_xs().child(note))
    }

    fn layer_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let index = self.selected_layer;
        let Some(layer) = self.document.layers().get(index) else {
            return div().child("Select a layer.").into_any_element();
        };
        let id = layer.id;
        let (visible, blend, opacity) = (layer.visible, layer.blend, layer.opacity);
        let steps: Vec<String> = layer
            .steps
            .iter()
            .map(|step| step.kind.name().to_string())
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Self::heading("Selected layer"))
            .child(layer.name.clone())
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        Self::button(if visible { "Shown" } else { "Hidden" }, "visible", visible)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(layer) = this.document.layer_mut(id) {
                                    layer.visible = !layer.visible;
                                    let visible = layer.visible;
                                    let _ = visible;
                                    this.refresh_canvas();
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Self::button(format!("Blend: {blend}"), "blend", false).on_click(
                            cx.listener(move |this, _, _, cx| {
                                if let Some(layer) = this.document.layer_mut(id) {
                                    let at = BlendMode::ALL
                                        .iter()
                                        .position(|mode| *mode == layer.blend)
                                        .unwrap_or(0);
                                    layer.blend = BlendMode::ALL[(at + 1) % BlendMode::ALL.len()];
                                }
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(70.0))
                            .text_xs()
                            .child(format!("Opacity {:.0}%", opacity * 100.0)),
                    )
                    .child(self.slider(SliderId::Opacity(index), opacity, cx)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(QUIET))
                    .child("Layers blend in the order of the list, bottom first."),
            )
            .child(Self::heading("This layer's adjustments"))
            .children(steps.into_iter().map(|name| div().text_xs().child(name)))
            .child(
                Self::button("Add Curves to this layer", "layer-curves", false).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let _ = this
                            .document
                            .insert_layer_step(id, operation("Curves", Accepts::Display));
                        cx.notify();
                    }),
                ),
            )
            .into_any_element()
    }

    fn right_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let stage = self.stage;
        let controls: AnyElement = match stage {
            StageKind::Sources => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(Self::heading("Bands"))
                .children(self.band_rows(None, cx))
                .into_any_element(),
            StageKind::Linear => self.linear_notes().into_any_element(),
            StageKind::Stretch => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(Self::heading("Midtone per band"))
                .children(self.band_rows(Some(SliderId::Midtone), cx))
                .into_any_element(),
            StageKind::Combine => self.layer_controls(cx),
            StageKind::Finish => div()
                .text_xs()
                .child("Steps here act on every layer at once. A step may still be masked.")
                .into_any_element(),
            StageKind::Export => div()
                .text_xs()
                .child("The canvas line below shows the surface's format and color space.")
                .into_any_element(),
        };
        div()
            .flex_none()
            .w(px(PANEL_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(PANE))
            .child(controls)
            .when(stage.allows_manual_edits(), |pane| {
                pane.child(self.tools(cx))
            })
            .child(div().flex_1())
            .child(Self::heading("Canvas"))
            .children(
                self.readout()
                    .into_iter()
                    .map(|line| div().text_xs().child(line)),
            )
    }
}

impl Render for Studio {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.scale_factor = window.scale_factor();
        let scale_factor = self.scale_factor;
        let handle = self.canvas.clone();
        let canvas_bounds = self.canvas_bounds.clone();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BACKGROUND))
            .text_color(rgb(TEXT))
            .text_sm()
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .child(self.stage_rail(cx))
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .overflow_hidden()
                    .child(self.left_pane(cx))
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .relative()
                            .child(self.native_view.clone())
                            .child(
                                // In front of the native view, so GPUI
                                // receives the mouse there. It also reports
                                // the canvas's size.
                                div()
                                    .absolute()
                                    .size_full()
                                    .top_0()
                                    .left_0()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(Self::canvas_down),
                                    )
                                    .on_mouse_move(cx.listener(Self::mouse_move))
                                    .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
                                    .on_scroll(cx.listener(Self::scroll))
                                    .child(
                                        canvas(
                                            move |bounds, _, _| {
                                                canvas_bounds.set(bounds);
                                                let size = (
                                                    (f32::from(bounds.size.width) * scale_factor)
                                                        .round()
                                                        as u32,
                                                    (f32::from(bounds.size.height) * scale_factor)
                                                        .round()
                                                        as u32,
                                                );
                                                let known = handle
                                                    .read(|shared| shared.canvas.surface_size);
                                                if known != size {
                                                    handle
                                                        .edit(|canvas| canvas.surface_size = size);
                                                }
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .size_full(),
                                    ),
                            ),
                    )
                    .child(self.right_pane(cx)),
            )
    }
}
