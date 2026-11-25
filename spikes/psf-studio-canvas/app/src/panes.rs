//! The window's panes.
//!
//! - **Left: one tree** of the whole document (`tree.rs`), top to bottom in
//!   the order the picture is made: each input with its steps, the layers of
//!   the Combine stage, then the Finish steps. An edit that belongs to one
//!   input or one layer is indented under it.
//! - **Middle: the canvas**, split into one or more panes. A pane shows the
//!   result up to the selected row, unless it is pinned.
//! - **Right: the selected row's controls** (`inspector.rs`).
//!
//! What a row is, what a pane shows and how a split chooses its panes are
//! decided in `src/studio/place.rs`, which knows of no toolkit. These files
//! only lay that out with GPUI, and turn a place into what the canvas draws.

use super::{Bands, Drag, Scene, SliderId, Studio, Tool};
use crate::image_steps::Accepts;
use crate::store_tiles::BandStretch;
use crate::studio_model::{
    band_layer, color_from_hue, document_from_color_setup, flow_operations, hue_of, layer_name,
    panes, plan, BlendMode, Document, LayerRecord, LayerSource, MaskRecord, PlannedStretch, Row,
    Split, StackEntry, StageKind, StepKind, StepRecord, Stroke, StrokeMode, View,
};
use gpui::{
    canvas, div, prelude::*, px, relative, rgb, AnyElement, Context, MouseButton, MouseDownEvent,
    Rgba, SharedString, Window,
};
use std::sync::Arc;
use studio_render_spike::{
    Adjustment, Blend, FinishKind, FinishStep, Layer, LayerHeal, Segment, MAX_FINISH_STEPS,
    MAX_HEALED_LAYERS, MAX_HEALS, MAX_LAYERS, MAX_MASKS,
};
use uuid::Uuid;

pub(super) const BACKGROUND: u32 = 0x16181c;
pub(super) const PANE: u32 = 0x1e2126;
pub(super) const CONTROL: u32 = 0x2a2e34;
pub(super) const SELECTED: u32 = 0x3b6ea8;
pub(super) const TEXT: u32 = 0xd8dee6;
pub(super) const QUIET: u32 = 0x8a93a0;

/// The adjustments offered in the Combine stage, for one layer or for the
/// layers beneath: the name and what the canvas does.
pub(super) const ADJUSTMENTS: [(&str, Adjustment); 3] = [
    ("Exposure", Adjustment::Exposure),
    ("Curves", Adjustment::Curves),
    ("Saturation", Adjustment::Saturation),
];

pub(super) fn operation(name: &str, accepts: Accepts) -> StepRecord {
    StepRecord::new(StepKind::Operation {
        operation: name.to_string(),
        accepts,
        settings: serde_json::Value::Null,
    })
}

/// PSF Guard's built-in default color setup.
const DEFAULT_COLOR_SETUP: &str =
    include_str!("../../../../src/processing_setups/default_color.json");

/// A new document: PSF Guard's default color flow over the store's inputs.
/// The steps and their settings are the ones PSF Guard's color previews
/// apply. The layers are the palette that uses the most of the inputs.
pub(super) fn flow_document(inputs: &[String]) -> Document {
    let channels: Vec<&str> = inputs.iter().map(String::as_str).collect();
    let settings = serde_json::from_str(DEFAULT_COLOR_SETUP).expect("the default setup is JSON");
    let mut document = document_from_color_setup("PSF Guard default color", &settings, &channels)
        .expect("the default setup makes a document");
    // When no palette fits the inputs, each still gets a layer in a color
    // of its own, so nothing a store holds is left out of the first picture.
    let plain: Vec<Uuid> = document
        .layers()
        .iter()
        .filter(|layer| layer.color.is_none())
        .map(|layer| layer.id)
        .collect();
    let count = plain.len() as f32;
    for (at, id) in plain.into_iter().enumerate() {
        let layer = document.layer_mut(id).expect("the layer was just listed");
        let LayerSource::Band { band } = &layer.source else {
            continue;
        };
        let fresh = band_layer(band, 360.0 * at as f32 / count, 0.8, BlendMode::Add);
        layer.name = fresh.name;
        layer.blend = fresh.blend;
        layer.color = fresh.color.map(|color| color.map(|part| part / count));
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
pub(super) fn counted(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// A layer's color for its swatch. A layer's color is scaled so the layers
/// sum to full scale; the swatch shows its hue at full brightness.
pub(super) fn swatch(color: [f32; 3], alpha: f32) -> Rgba {
    let brightest = color.iter().copied().fold(f32::MIN_POSITIVE, f32::max);
    Rgba {
        r: color[0] / brightest,
        g: color[1] / brightest,
        b: color[2] / brightest,
        a: alpha,
    }
}

/// The range of an operation's amount, for the operations the canvas draws.
pub(super) fn amount_range(operation: &str) -> Option<(f32, f32)> {
    match operation {
        "Exposure" => Some((-2.0, 2.0)),
        "Curves" => Some((-1.0, 1.0)),
        "Saturation" => Some((0.0, 2.0)),
        flow_operations::BACKGROUND => Some((0.0, 1.0)),
        _ => None,
    }
}

/// The setting that holds an operation's amount. Background extraction
/// keeps PSF Guard's settings, where the amount is `strength`.
fn amount_key(operation: &str) -> &'static str {
    if operation == flow_operations::BACKGROUND {
        "strength"
    } else {
        "amount"
    }
}

/// An operation's amount from its settings, or the amount a new one starts
/// with.
pub(super) fn amount_in(operation: &str, settings: &serde_json::Value) -> f32 {
    let start = match operation {
        "Saturation" => 1.4,
        flow_operations::BACKGROUND => 1.0,
        _ => 0.5,
    };
    settings
        .get(amount_key(operation))
        .and_then(serde_json::Value::as_f64)
        .map_or(start, |amount| amount as f32)
}

pub(super) fn amount(step: &StepRecord) -> f32 {
    match &step.kind {
        StepKind::Operation {
            operation,
            settings,
            ..
        } => amount_in(operation, settings),
        StepKind::Heal { .. } => 0.0,
    }
}

/// Settings with the operation's amount changed and the rest kept.
fn with_amount(operation: &str, settings: &serde_json::Value, amount: f32) -> serde_json::Value {
    let mut settings = match settings {
        serde_json::Value::Object(settings) => settings.clone(),
        _ => serde_json::Map::new(),
    };
    settings.insert(amount_key(operation).to_string(), amount.into());
    serde_json::Value::Object(settings)
}

/// Where Seiza's automatic stretch places the background when a step
/// does not say: Seiza's default.
const DEFAULT_STRETCH_TARGET: f64 = 0.2;

/// Where a stretch step places the background.
pub(super) fn stretch_target(step: &StepRecord) -> f64 {
    let StepKind::Operation { settings, .. } = &step.kind else {
        return DEFAULT_STRETCH_TARGET;
    };
    settings["model"]["target_median"]
        .as_f64()
        .unwrap_or(DEFAULT_STRETCH_TARGET)
}

/// What the canvas does for an adjustment, by its name.
fn adjustment_kind(operation: &str) -> Option<Adjustment> {
    ADJUSTMENTS
        .iter()
        .find(|(name, _)| *name == operation)
        .map(|(_, kind)| *kind)
}

/// Whether the canvas draws a step, or only keeps it in the document.
pub(super) fn is_drawn(stage: StageKind, step: &StepRecord, bands: &Bands) -> bool {
    match (&step.kind, stage) {
        // Drawn when the store holds each band corrected as well.
        (StepKind::Operation { operation, .. }, StageKind::Linear) => {
            operation == flow_operations::BACKGROUND && bands.has_correction()
        }
        (StepKind::Operation { operation, .. }, StageKind::Stretch) => {
            operation == flow_operations::STRETCH
        }
        (StepKind::Operation { operation, .. }, StageKind::Combine) => {
            adjustment_kind(operation).is_some()
        }
        // A heal of one input, of one layer, or of the whole picture.
        (StepKind::Heal { .. }, StageKind::Linear) => step.band.is_some(),
        (StepKind::Heal { .. }, StageKind::Combine | StageKind::Finish) => true,
        (StepKind::Operation { operation, .. }, StageKind::Finish) => {
            operation == "Curves" || operation == "Saturation"
        }
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

fn mask_strokes(mask: &Option<MaskRecord>) -> Vec<Segment> {
    match mask {
        Some(MaskRecord::Painted(mask)) => mask.strokes.iter().flat_map(segments).collect(),
        _ => Vec::new(),
    }
}

/// The strokes that limit a step: its painted mask, or a heal's own stroke.
fn step_strokes(step: &StepRecord) -> Vec<Segment> {
    match &step.kind {
        StepKind::Heal { stroke, .. } => segments(stroke),
        StepKind::Operation { .. } => mask_strokes(&step.mask),
    }
}

/// A mask in a few words, for a row or a note.
pub(super) fn mask_words(mask: &Option<MaskRecord>) -> Option<String> {
    match mask {
        Some(MaskRecord::Painted(mask)) => Some(format!(
            "painted mask, {}",
            counted(mask.strokes.len(), "stroke")
        )),
        Some(MaskRecord::Range { .. }) => Some("range mask".to_string()),
        None => None,
    }
}

/// Adds a heal of one layer to a scene, or records that it is left out
/// because the frame has no room for it.
fn push_heal(
    heals: &mut Vec<LayerHeal>,
    omitted: &mut Vec<String>,
    layer: usize,
    before_stretch: bool,
    step: &StepRecord,
    mask: Result<Option<usize>, ()>,
) {
    let StepKind::Heal { offset, shift, .. } = &step.kind else {
        return;
    };
    let mut healed: Vec<usize> = heals.iter().map(|heal| heal.layer).collect();
    healed.sort_unstable();
    healed.dedup();
    let fits =
        heals.len() < MAX_HEALS && (healed.contains(&layer) || healed.len() < MAX_HEALED_LAYERS);
    match mask {
        Ok(Some(mask)) if fits => heals.push(LayerHeal {
            layer,
            before_stretch,
            offset: [offset[0] as f32, offset[1] as f32],
            shift: shift.first().copied().unwrap_or(0.0),
            mask,
        }),
        // A heal with no stroke heals nothing.
        Ok(None) => {}
        _ => omitted.push("a heal".to_string()),
    }
}

/// What the document asks the canvas to draw for a view. Returns the scene,
/// without its stretch analyses, and each band's background strength in it.
/// What one frame has no room for is listed in the scene, never drawn
/// without its mask.
fn scene(document: &Document, bands: &Bands, view: &View, version: u64) -> (Scene, Vec<f32>) {
    let planned = plan(document, view);
    let mut strengths = vec![0.0; bands.len()];
    let mut layers: Vec<Layer> = Vec::new();
    let mut layer_bands = Vec::new();
    let mut layer_targets = Vec::new();
    let mut masks: Vec<Vec<Segment>> = Vec::new();
    let mut heals: Vec<LayerHeal> = Vec::new();
    let mut omitted: Vec<String> = Vec::new();
    // Strokes as a place in `masks`. `Err` when the masks are full: what the
    // mask limits is then left out, so it is never drawn over the whole
    // image by mistake.
    let mut mask_for = |strokes: Vec<Segment>| -> Result<Option<usize>, ()> {
        if strokes.is_empty() {
            return Ok(None);
        }
        if masks.len() == MAX_MASKS {
            return Err(());
        }
        masks.push(strokes);
        Ok(Some(masks.len() - 1))
    };
    let adjustment = |kind: Adjustment, amount: f32, opacity: f32, mask, own| Layer {
        band: 0,
        blended: None,
        color: [1.0; 3],
        opacity,
        blend: Blend::Normal,
        curve: 0.0,
        mask,
        adjustment: Some((kind, amount)),
        own,
    };
    let find = |id: &Uuid| document.layers().iter().find(|layer| layer.id == *id);
    // A channel view draws its channel alone; a blended view draws the stack.
    let entries: Vec<(Option<usize>, Option<&LayerRecord>)> = if planned.stack.is_empty() {
        (0..planned.channels.len())
            .map(|at| (Some(at), None))
            .collect()
    } else {
        planned
            .stack
            .iter()
            .map(|entry| match entry {
                StackEntry::Band { layer, channel } => (Some(*channel), find(layer)),
                StackEntry::Adjustment { layer } => (None, find(layer)),
            })
            .collect()
    };
    for (at, layer) in entries {
        if layers.len() >= MAX_LAYERS {
            omitted.push(layer.map_or("a layer".to_string(), |layer| layer.name.clone()));
            continue;
        }
        let Some(channel) = at.and_then(|at| planned.channels.get(at)) else {
            // An adjustment of the layers beneath it.
            let Some(layer) = layer else { continue };
            let LayerSource::Adjustment {
                operation,
                settings,
            } = &layer.source
            else {
                continue;
            };
            let Some(kind) = adjustment_kind(operation) else {
                continue;
            };
            let Ok(mask) = mask_for(mask_strokes(&layer.mask)) else {
                omitted.push(layer.name.clone());
                continue;
            };
            let opacity = if layer.visible { layer.opacity } else { 0.0 };
            layers.push(adjustment(
                kind,
                amount_in(operation, settings),
                opacity,
                mask,
                false,
            ));
            continue;
        };
        let Some(band) = bands.band_of(&channel.channel) else {
            continue;
        };
        let strength = channel
            .background
            .and_then(|id| document.step(id))
            .map_or(0.0, |(_, step)| amount(step));
        strengths[band] = strength;
        let target = match &channel.stretch {
            PlannedStretch::Document(steps) => steps
                .first()
                .and_then(|id| document.step(*id))
                .map_or(DEFAULT_STRETCH_TARGET, |(_, step)| stretch_target(step)),
            PlannedStretch::Preview => DEFAULT_STRETCH_TARGET,
        };
        let shown = |color, opacity, blend, mask| Layer {
            band: bands.tile_band[band],
            blended: bands.corrected[band].map(|corrected| (corrected, strength)),
            color,
            opacity,
            blend,
            curve: 0.0,
            mask,
            adjustment: None,
            own: false,
        };
        // The heals of the input alone change its samples before the
        // stretch, wherever the input is shown.
        let input_heals: Vec<&StepRecord> = channel
            .heals
            .iter()
            .filter_map(|id| document.step(*id))
            .map(|(_, step)| step)
            .collect();
        let Some(layer) = layer else {
            // A channel on its own is drawn in gray.
            layers.push(shown([1.0; 3], 1.0, Blend::Normal, None));
            layer_bands.push(band);
            layer_targets.push(target);
            for step in input_heals {
                let mask = mask_for(step_strokes(step));
                push_heal(&mut heals, &mut omitted, layers.len() - 1, true, step, mask);
            }
            continue;
        };
        let Ok(mask) = mask_for(mask_strokes(&layer.mask)) else {
            omitted.push(layer.name.clone());
            continue;
        };
        layers.push(shown(
            layer.color.unwrap_or([1.0; 3]),
            if layer.visible { layer.opacity } else { 0.0 },
            match layer.blend {
                BlendMode::Normal => Blend::Normal,
                BlendMode::Screen => Blend::Screen,
                BlendMode::Add => Blend::Add,
                BlendMode::Multiply => Blend::Multiply,
                BlendMode::Lighten => Blend::Lighten,
                BlendMode::Darken => Blend::Darken,
                BlendMode::Luminance => Blend::Luminance,
            },
            mask,
        ));
        layer_bands.push(band);
        layer_targets.push(target);
        let shown_at = layers.len() - 1;
        for step in input_heals {
            let mask = mask_for(step_strokes(step));
            push_heal(&mut heals, &mut omitted, shown_at, true, step, mask);
        }
        // The layer's own edits change it before it is blended. Its heals
        // change the stretched layer; its adjustments follow.
        let own = layer
            .steps
            .iter()
            .filter(|step| planned.own_steps.contains(&step.id));
        for step in own {
            let StepKind::Operation { operation, .. } = &step.kind else {
                let mask = mask_for(step_strokes(step));
                push_heal(&mut heals, &mut omitted, shown_at, false, step, mask);
                continue;
            };
            let Some(kind) = adjustment_kind(operation) else {
                continue;
            };
            let mask = mask_for(mask_strokes(&step.mask));
            match mask {
                Ok(mask) if layers.len() < MAX_LAYERS => {
                    layers.push(adjustment(kind, amount(step), step.opacity, mask, true));
                }
                _ => omitted.push(format!("{} of {}", operation, layer.name)),
            }
        }
    }

    let mut finish = Vec::new();
    for step in &document.stage(StageKind::Finish).steps {
        if !planned.finish.contains(&step.id) || !is_drawn(StageKind::Finish, step, bands) {
            continue;
        }
        if finish.len() == MAX_FINISH_STEPS {
            omitted.push(step.kind.name().to_string());
            continue;
        }
        let Ok(mask) = mask_for(step_strokes(step)) else {
            omitted.push(step.kind.name().to_string());
            continue;
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
        // A heal with no stroke heals nothing.
        if matches!(kind, FinishKind::Heal { .. }) && mask.is_none() {
            continue;
        }
        finish.push(FinishStep {
            kind,
            amount: amount(step),
            opacity: step.opacity,
            mask,
        });
    }
    let scene = Scene {
        layers,
        layer_bands,
        layer_targets,
        finish,
        heals,
        masks,
        omitted,
        stretches: Arc::default(),
        version,
    };
    (scene, strengths)
}

impl Studio {
    // ---- what the selection is ------------------------------------------

    /// The selected step: an input's, a layer's own, or a Finish step.
    pub(super) fn selected_step(&self) -> Option<Uuid> {
        match &self.selected {
            Row::InputStep { step, .. } | Row::LayerStep { step, .. } | Row::Finish(step) => {
                Some(*step)
            }
            _ => None,
        }
    }

    pub(super) fn layer(&self, id: Uuid) -> Option<&LayerRecord> {
        self.document.layers().iter().find(|layer| layer.id == id)
    }

    fn selected_layer(&self) -> Option<&LayerRecord> {
        match &self.selected {
            Row::Layer(id) => self.layer(*id),
            _ => None,
        }
    }

    /// Whether the mask brush has something to paint: the canvas draws
    /// masks for layers, for a layer's own adjustments and for Finish steps.
    pub(super) fn can_mask(&self) -> bool {
        match &self.selected {
            Row::Layer(_) | Row::Finish(_) => true,
            Row::LayerStep { step, .. } => self
                .document
                .step(*step)
                .is_some_and(|(_, step)| matches!(step.kind, StepKind::Operation { .. })),
            _ => false,
        }
    }

    /// The strokes shown over the image while a brush is in hand: the
    /// selected row's mask.
    fn selected_strokes(&self) -> Vec<Segment> {
        if let Some(layer) = self.selected_layer() {
            return mask_strokes(&layer.mask);
        }
        self.selected_step()
            .and_then(|id| self.document.step(id))
            .map(|(_, step)| step_strokes(step))
            .unwrap_or_default()
    }

    /// Whether the selected row has strokes to show: a painted mask, or a
    /// heal's stroke.
    pub(super) fn has_strokes(&self) -> bool {
        !self.selected_strokes().is_empty()
    }

    /// Selects a row of the tree. A pane that is not pinned follows.
    pub(super) fn select(&mut self, row: Row) {
        self.selected = row;
        if self.tool == Tool::Brush && !self.can_mask() {
            self.tool = Tool::Pan;
        }
        self.refresh_canvas();
    }

    /// Divides the canvas another way. No pane stays pinned.
    pub(super) fn set_split(&mut self, split: Split) {
        self.split = split;
        self.drag = None;
        self.pins.clear();
        self.refresh_canvas();
        // The panes changed size, so the document is fitted to one again.
        self.canvas.edit(|canvas| canvas.fit());
    }

    // ---- the canvas -----------------------------------------------------

    /// Each band's stretch analysis at these background strengths. An
    /// analysis is kept, so a pane that returns to a place does not wait.
    fn analysis(&mut self, strengths: &[f32]) -> Arc<Vec<BandStretch>> {
        if let Some((_, known)) = self.analysed.iter().find(|(at, _)| at == strengths) {
            return known.clone();
        }
        let stretches = Arc::new(self.band_info.stretches(strengths).unwrap_or_default());
        self.analysed
            .insert(0, (strengths.to_vec(), stretches.clone()));
        self.analysed.truncate(8);
        stretches
    }

    /// Sends the canvas what the document now asks it to draw: one scene
    /// per pane.
    pub(super) fn refresh_canvas(&mut self) {
        self.scene_version += 1;
        let selected = self.selected.place(self.alone);
        let mut places = panes(self.split, &selected, &self.document);
        self.pins.resize(places.len(), None);
        for (place, pin) in places.iter_mut().zip(&self.pins) {
            if let Some(pinned) = pin {
                *place = pinned.clone();
            }
        }
        let scenes: Vec<Scene> = places
            .iter()
            .map(|place| {
                let (mut scene, strengths) = scene(
                    &self.document,
                    &self.band_info,
                    &place.view(),
                    self.scene_version,
                );
                // The stretch is applied to the corrected samples, so each
                // band's analysis follows its background strength.
                scene.stretches = self.analysis(&strengths);
                scene
            })
            .collect();
        self.places = places;
        let scenes = Arc::new(scenes);
        // The selected row's mask shows over the image while a person has
        // it switched on, so they see what they have painted.
        let overlay = if self.mask_shown {
            self.selected_strokes()
        } else {
            Vec::new()
        };
        let wipe = (self.split == Split::Wipe).then_some(self.wipe);
        self.canvas.edit(|canvas| {
            canvas.scenes = scenes;
            canvas.strokes = overlay;
            canvas.wipe = wipe;
        });
    }

    /// Ends a brush stroke. A mask stroke joins the selected row's mask. A
    /// heal stroke becomes a step where the selection is: of the selected
    /// input, of the selected layer, or of the combined picture.
    pub(super) fn commit_stroke(&mut self, stroke: Stroke) {
        match self.tool {
            Tool::Pan => {}
            Tool::Brush => match &self.selected {
                Row::Layer(id) => {
                    let _ = self.document.paint_layer_mask(*id, stroke);
                }
                Row::LayerStep { step, .. } | Row::Finish(step) => {
                    let _ = self.document.paint_step_mask(*step, stroke);
                }
                _ => {}
            },
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
                let row = match self.selected.clone() {
                    Row::Input(channel) | Row::InputStep { channel, .. } => self
                        .document
                        .insert_step(StageKind::Linear, usize::MAX, step.for_band(&channel))
                        .ok()
                        .map(|()| Row::InputStep { channel, step: id }),
                    Row::Layer(layer) | Row::LayerStep { layer, .. } => {
                        let shows_pixels = self.layer(layer).is_some_and(|layer| {
                            !matches!(layer.source, LayerSource::Adjustment { .. })
                        });
                        (shows_pixels && self.document.insert_layer_step(layer, step).is_ok())
                            .then_some(Row::LayerStep { layer, step: id })
                    }
                    Row::Combined | Row::Finish(_) | Row::Final => self
                        .document
                        .insert_step(StageKind::Finish, usize::MAX, step)
                        .ok()
                        .map(|()| Row::Finish(id)),
                };
                if let Some(row) = row {
                    self.selected = row;
                }
            }
        }
        self.refresh_canvas();
    }

    /// Applies a slider's new position, from 0 to 1, to the selected row.
    pub(super) fn slider_moved(&mut self, slider: SliderId, position: f32) {
        match slider {
            SliderId::Midtone | SliderId::Amount => {
                let Some(id) = self.selected_step() else {
                    return;
                };
                let Some((
                    _,
                    StepRecord {
                        kind:
                            StepKind::Operation {
                                operation,
                                settings,
                                ..
                            },
                        ..
                    },
                )) = self.document.step(id)
                else {
                    return;
                };
                let changed = if slider == SliderId::Midtone {
                    // The slider covers background targets from 0.05 to 0.5.
                    let mut settings = settings.clone();
                    if !settings.is_object() {
                        settings = serde_json::json!({ "model": { "type": "auto-mtf" } });
                    }
                    settings["model"]["target_median"] = (0.05 + 0.45 * f64::from(position)).into();
                    settings
                } else {
                    let amount = amount_range(operation)
                        .map_or(position, |(low, high)| low + (high - low) * position);
                    with_amount(operation, settings, amount)
                };
                self.document.set_step_settings(id, changed);
            }
            SliderId::LayerAmount => {
                let Some(layer) = self.selected_layer() else {
                    return;
                };
                let id = layer.id;
                let LayerSource::Adjustment {
                    operation,
                    settings,
                } = &layer.source
                else {
                    return;
                };
                let Some((low, high)) = amount_range(operation) else {
                    return;
                };
                let changed = with_amount(operation, settings, low + (high - low) * position);
                if let Some(layer) = self.document.layer_mut(id)
                    && let LayerSource::Adjustment { settings, .. } = &mut layer.source
                {
                    *settings = changed;
                }
            }
            SliderId::Opacity | SliderId::Hue | SliderId::Saturation => {
                // A step's opacity, when a step is selected.
                if let (SliderId::Opacity, Some(step)) = (slider, self.selected_step()) {
                    self.document.set_step_opacity(step, position);
                    return;
                }
                let Row::Layer(id) = &self.selected else {
                    return;
                };
                let Some(layer) = self.document.layer_mut(*id) else {
                    return;
                };
                if slider == SliderId::Opacity {
                    layer.opacity = position;
                    return;
                }
                // The color keeps its brightness, which balances the layers.
                let (hue, saturation, value) = hue_of(layer.color.unwrap_or([1.0; 3]));
                let (hue, saturation) = if slider == SliderId::Hue {
                    (position * 359.9, saturation)
                } else {
                    (hue, position)
                };
                let color = color_from_hue(hue, saturation, value);
                layer.color = Some(color);
                if let LayerSource::Band { band } = &layer.source {
                    layer.name = layer_name(band, color, layer.blend);
                }
            }
        }
    }

    // ---- widgets the panes share ----------------------------------------

    /// A text button. `on` draws it as the selected choice.
    pub(super) fn button(
        label: impl Into<SharedString>,
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

    /// A button that changes the document or the view, then redraws.
    pub(super) fn act(
        label: impl Into<SharedString>,
        id: impl Into<gpui::ElementId>,
        on: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        Self::button(label, id, on).on_click(cx.listener(move |this, _, _, cx| {
            action(this);
            this.refresh_canvas();
            cx.stop_propagation();
            cx.notify();
        }))
    }

    pub(super) fn heading(text: impl Into<SharedString>) -> gpui::Div {
        div()
            .pt_2()
            .text_color(rgb(QUIET))
            .text_xs()
            .child(text.into())
    }

    /// A line of quiet small text.
    pub(super) fn note(text: impl Into<SharedString>) -> gpui::Div {
        div().text_xs().text_color(rgb(QUIET)).child(text.into())
    }

    pub(super) fn slider(&self, id: SliderId, value: f32, cx: &mut Context<Self>) -> AnyElement {
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
                    .left(relative(value.clamp(0.0, 1.0) * 0.96))
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

    /// A slider with its label and value in front of it.
    pub(super) fn labelled_slider(
        &self,
        label: String,
        id: SliderId,
        value: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().w(px(104.0)).text_xs().child(label))
            .child(self.slider(id, value, cx))
    }

    // ---- the bar over the canvas ----------------------------------------

    /// The document's name and how the canvas is divided.
    fn top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let splits = Split::ALL.into_iter().enumerate().map(|(index, split)| {
            Self::act(
                split.name(),
                ("split", index),
                self.split == split,
                cx,
                move |this| this.set_split(split),
            )
        });
        div()
            .flex_none()
            .h(px(44.0))
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .bg(rgb(PANE))
            .child(div().pr_3().child(self.document.name.clone()))
            .child(Self::note("Canvas"))
            .children(splits)
    }

    /// One label per pane, drawn in the pane's top left corner: what the
    /// pane shows, what it leaves out, and a button that keeps it there
    /// while the selection moves.
    fn pane_labels(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let scale = self.scale_factor.max(1.0);
        let (rects, omitted): (Vec<[u32; 4]>, Vec<Vec<String>>) = self.canvas.read(|shared| {
            let canvas = &shared.canvas;
            (0..canvas.scenes.len())
                .map(|index| {
                    (
                        canvas.pane_rects(index).1,
                        canvas.scenes[index].omitted.clone(),
                    )
                })
                .unzip()
        });
        self.places
            .iter()
            .zip(rects)
            .zip(omitted)
            .enumerate()
            .map(|(index, ((place, rect), omitted))| {
                let pinned = self.pins.get(index).is_some_and(Option::is_some);
                let place = place.clone();
                let left_out = (!omitted.is_empty()).then(|| {
                    format!(
                        "Not drawn, because one frame has no room for more: {}.",
                        omitted.join(", ")
                    )
                });
                div()
                    .absolute()
                    .left(px(rect[0] as f32 / scale + 6.0))
                    .top(px(rect[1] as f32 / scale + 6.0))
                    .max_w(px((rect[2] as f32 / scale - 12.0).max(40.0)))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(gpui::rgba(0x16181cd0))
                    .text_xs()
                    // A press on a label is not a press on the canvas.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(place.label(&self.document))
                            .child(Self::act(
                                if pinned { "Pinned" } else { "Pin" },
                                ("pin", index),
                                pinned,
                                cx,
                                move |this| {
                                    if let Some(pin) = this.pins.get_mut(index) {
                                        *pin = if pin.is_some() {
                                            None
                                        } else {
                                            Some(place.clone())
                                        };
                                    }
                                },
                            )),
                    )
                    .children(left_out.map(Self::note))
                    .into_any_element()
            })
            .collect()
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
            .child(self.top_bar(cx))
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .overflow_hidden()
                    .child(self.tree(cx))
                    .child(
                        div().flex_1().h_full().flex().flex_col().child(
                            div()
                                .flex_1()
                                .w_full()
                                .relative()
                                .child(self.native_view.clone())
                                .child(
                                    // In front of the native view, so
                                    // GPUI receives the mouse there. It
                                    // also reports the canvas's size.
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
                                                        (f32::from(bounds.size.width)
                                                            * scale_factor)
                                                            .round()
                                                            as u32,
                                                        (f32::from(bounds.size.height)
                                                            * scale_factor)
                                                            .round()
                                                            as u32,
                                                    );
                                                    let known = handle
                                                        .read(|shared| shared.canvas.surface_size);
                                                    if known != size {
                                                        handle.edit(|canvas| {
                                                            canvas.surface_size = size
                                                        });
                                                    }
                                                },
                                                |_, _, _, _| {},
                                            )
                                            .size_full(),
                                        )
                                        .children(self.pane_labels(cx)),
                                ),
                        ),
                    )
                    .child(self.inspector(cx)),
            )
    }
}
