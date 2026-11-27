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

use super::{picture_patches, Bands, Drag, PictureRequest, Scene, SliderId, Studio, Tool};
use crate::canvas::{Adjustment, ComputedMask, Layer, Mark, MaskBand};
use crate::image_steps::Accepts;
use crate::store_tiles::{BandState, BandStretch};
use crate::studio_model::{
    band_layer, channel_label, color_from_hue, document_from_color_setup, flow_operations, hue_of,
    layer_name, panes, plan, BlendMode, ChannelPlan, CropRecord, Document, LayerRecord,
    LayerSource, MaskRecord, MaskShape, Place, PlannedStretch, Row, SharedMask, Split, StackEntry,
    StageKind, StepKind, StepRecord, Stroke, StrokeMode, Subject, View,
};
use crate::{output_stretch, tone_step};
use gpui::{
    canvas, div, img, prelude::*, px, relative, rgb, AnyElement, Context, MouseButton,
    MouseDownEvent, Rgba, SharedString, Window,
};
use seiza_compose::gpu::{
    MAX_COMPUTED_MASKS, MAX_FINISH_STEPS, MAX_HEALED_LAYERS, MAX_HEALS, MAX_LAYERS, MAX_MASKS,
};
use seiza_compose::picture::{Blend, FinishKind, FinishStep, LayerHeal, Segment};
use seiza_stretch::StretchModel;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

pub(super) const BACKGROUND: u32 = 0x16181c;
pub(super) const PANE: u32 = 0x1e2126;
pub(super) const CONTROL: u32 = 0x2a2e34;
pub(super) const SELECTED: u32 = 0x3b6ea8;
pub(super) const TEXT: u32 = 0xd8dee6;
pub(super) const QUIET: u32 = 0x8a93a0;
/// Behind the context line while a mask is being edited.
const MASK_CONTEXT: u32 = 0x7a4a12;

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

/// A new Output stretch step: a generalized hyperbolic stretch of the
/// combined picture, linked across the channels. It needs no samples of the
/// picture, so the canvas draws it at once.
pub(super) fn output_stretch_step() -> StepRecord {
    StepRecord::new(StepKind::Operation {
        operation: flow_operations::OUTPUT_STRETCH.to_string(),
        accepts: Accepts::Display,
        settings: serde_json::json!({
            "model": new_stretch("ghs"),
            "color_strategy": "linked",
        }),
    })
}

/// PSF Guard's built-in default color setup.
const DEFAULT_COLOR_SETUP: &str = include_str!("../../../src/processing_setups/default_color.json");

/// A new document: PSF Guard's default color flow over the store's inputs.
/// The steps and their settings are the ones PSF Guard's color previews
/// apply. The layers are the palette that uses the most of the inputs.
pub(super) fn flow_document(inputs: &[String]) -> Document {
    let channels: Vec<&str> = inputs.iter().map(String::as_str).collect();
    let settings = serde_json::from_str(DEFAULT_COLOR_SETUP).expect("the default setup is JSON");
    let mut document = document_from_color_setup("Untitled", &settings, &channels)
        .expect("the default setup makes a document");
    // Studio starts from the layered stretch.
    document.use_layered_stretch();
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

/// PSF Studio's mark, from the application icon.
pub(super) fn brand_mark() -> Arc<gpui::Image> {
    static MARK: std::sync::OnceLock<Arc<gpui::Image>> = std::sync::OnceLock::new();
    MARK.get_or_init(|| {
        Arc::new(gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            include_bytes!("../../icons/128x128.png").to_vec(),
        ))
    })
    .clone()
}

/// The names documents had before they were named after their picture.
const SETUP_NAMES: [&str; 2] = ["PSF Guard default color", "Default"];

impl Studio {
    /// The picture's name: the document's, or for a document named after
    /// the setup it started from, the name of its store.
    pub(super) fn picture_name(&self) -> String {
        let named = &self.document.name;
        if !SETUP_NAMES.contains(&named.as_str()) {
            return named.clone();
        }
        self.store_path()
            .and_then(|store| store.file_stem())
            .map_or_else(
                || "Untitled".to_string(),
                |stem| stem.to_string_lossy().into_owned(),
            )
    }
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
        flow_operations::STAR_STRETCH => Some((0.0, 8.0)),
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
        flow_operations::STAR_STRETCH => 5.0,
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

/// The stretches Seiza has that work on samples alone, as a person
/// chooses between them: Seiza's name for the stretch, and a label. The
/// layered stretch is not here: it needs the image, so the engine draws it.
pub(super) const STRETCHES: [(&str, &str); 10] = [
    ("auto-mtf", "Automatic midtones"),
    ("median-anchor", "Median anchor"),
    ("ghs", "Generalized hyperbolic"),
    ("linear", "Linear"),
    ("asinh", "Asinh"),
    ("percentile-asinh", "Asinh, by percentiles"),
    ("mtf", "Midtones"),
    ("ht", "Histogram transformation"),
    ("identity", "None"),
    ("layered", "Layered"),
];

/// The settings of one of Seiza's stretches that a slider sets: Seiza's
/// name for the setting, a label, and the range the slider covers.
pub(super) fn stretch_settings(kind: &str) -> &'static [(&'static str, &'static str, f64, f64)] {
    match kind {
        "layered" => &[
            ("target_background", "Background", 0.05, 0.5),
            ("aggressiveness", "Black point", 0.0, 1.0),
            ("dynamic_range_compression", "Star cores", 0.0, 1.0),
            ("contrast_recovery", "Contrast", 0.0, 1.0),
        ],
        "auto-mtf" => &[
            ("target_median", "Background", 0.05, 0.5),
            ("shadows_clip", "Shadows clip", -5.0, 0.0),
        ],
        "median-anchor" => &[
            ("target_median", "Background", 0.05, 0.5),
            ("black_point_sigma", "Black point, sigmas", 0.5, 6.0),
            ("curves_boost", "Curves boost", 0.0, 0.9),
            ("hdr_amount", "Highlight compression", 0.0, 1.0),
            ("hdr_knee", "Compression starts at", 0.3, 0.95),
        ],
        "ghs" => &[
            ("stretch_factor", "Stretch factor", 0.0, 10.0),
            ("local_intensity", "Local intensity", -5.0, 15.0),
            ("symmetry_point", "Symmetry point", 0.0, 1.0),
            ("protect_shadows", "Protect shadows below", 0.0, 1.0),
            ("protect_highlights", "Protect highlights above", 0.0, 1.0),
            ("black", "Black point", 0.0, 0.5),
            ("white", "White point", 0.5, 1.0),
        ],
        "linear" => &[
            ("black", "Black point", 0.0, 0.5),
            ("white", "White point", 0.5, 1.0),
        ],
        "asinh" => &[
            ("black", "Black point", 0.0, 0.5),
            ("white", "White point", 0.5, 1.0),
            ("strength", "Strength", 1.0, 500.0),
        ],
        "percentile-asinh" => &[
            ("black_percentile", "Black percentile", 0.0, 0.2),
            ("white_percentile", "White percentile", 0.9, 1.0),
            ("strength", "Strength", 1.0, 500.0),
        ],
        "mtf" => &[
            ("shadows", "Shadows", 0.0, 0.5),
            ("midtone", "Midtone", 0.01, 0.99),
            ("highlights", "Highlights", 0.5, 1.0),
        ],
        "ht" => &[
            ("shadows", "Shadows", 0.0, 0.5),
            ("midtone", "Midtone", 0.01, 0.99),
            ("highlights", "Highlights", 0.5, 1.0),
        ],
        _ => &[],
    }
}

/// One of Seiza's stretches with the settings a new one starts with.
pub(super) fn new_stretch(kind: &str) -> serde_json::Value {
    use serde_json::json;
    match kind {
        "auto-mtf" => json!({ "type": kind, "target_median": 0.2, "shadows_clip": -2.8 }),
        "median-anchor" => json!({
            "type": kind, "target_median": 0.25, "black_point_sigma": 2.7,
            "curves_boost": 0.0, "hdr_amount": 0.0, "hdr_knee": 0.75, "normalize": false,
        }),
        "ghs" => json!({
            "type": kind, "stretch_factor": 1.0, "local_intensity": 4.0, "symmetry_point": 0.2,
            "protect_shadows": 0.0, "protect_highlights": 1.0, "black": 0.0, "white": 1.0,
        }),
        "linear" => json!({ "type": kind, "black": 0.0, "white": 1.0 }),
        "asinh" => json!({ "type": kind, "black": 0.0, "white": 1.0, "strength": 50.0 }),
        "percentile-asinh" => json!({
            "type": kind, "black_percentile": 0.001, "white_percentile": 0.999, "strength": 50.0,
        }),
        "mtf" => json!({ "type": kind, "shadows": 0.0, "midtone": 0.25, "highlights": 1.0 }),
        "ht" => json!({ "type": kind, "shadows": 0.0, "midtone": 0.5, "highlights": 1.0 }),
        "layered" => json!({
            "type": kind, "target_background": 0.15, "aggressiveness": 0.5,
            "dynamic_range_compression": 0.0, "contrast_recovery": 0.0,
        }),
        _ => json!({ "type": "identity" }),
    }
}

/// Keeps a generalized hyperbolic stretch in the order Seiza asks for after
/// `changed` moved: shadow protection at or below the symmetry point, and
/// highlight protection at or above it.
fn keep_stretch_valid(model: &mut serde_json::Value, changed: &str) {
    if model["type"] != "ghs" {
        return;
    }
    let value = |model: &serde_json::Value, key: &str| model[key].as_f64().unwrap_or(0.0);
    let symmetry = value(model, "symmetry_point");
    match changed {
        "protect_shadows" => model["protect_shadows"] = value(model, changed).min(symmetry).into(),
        "protect_highlights" => {
            model["protect_highlights"] = value(model, changed).max(symmetry).into()
        }
        _ => {
            model["protect_shadows"] = value(model, "protect_shadows").min(symmetry).into();
            model["protect_highlights"] = value(model, "protect_highlights").max(symmetry).into();
        }
    }
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
            let band = step.band.as_deref().and_then(|band| bands.band_of(band));
            (operation == flow_operations::BACKGROUND && bands.has_correction())
                // The store's red, green and blue hold the calibration.
                || (operation == flow_operations::COLOR_CALIBRATION && bands.is_calibrated())
                // An input's tools are drawn when the store holds their
                // result.
                || ((operation == flow_operations::RC_ASTRO_INPUT
                    || operation == flow_operations::DECONVOLUTION)
                    && band.is_some_and(|band| bands.has_tools(band)))
                // A color group's tools are drawn when the store holds
                // their result for the group's inputs.
                || (operation == flow_operations::RC_ASTRO_COLOR
                    && crate::studio_model::COLOR_GROUPS
                        .iter()
                        .find(|(group, _)| step.band.as_deref() == Some(*group))
                        .is_some_and(|(_, inputs)| {
                            inputs
                                .iter()
                                .filter_map(|input| bands.band_of(input))
                                .any(|band| bands.has_tools(band))
                        }))
        }
        (StepKind::Operation { .. }, StageKind::Stretch) if stretch_model(step).is_some() => {
            // The layered stretch is drawn when the store stretched the
            // step's input over the whole image.
            !matches!(stretch_model(step), Some(StretchModel::Layered(_)))
                || step
                    .band
                    .as_deref()
                    .and_then(|band| bands.band_of(band))
                    .is_some_and(|band| bands.stretched_plane(band).is_some())
        }
        (StepKind::Operation { operation, .. }, StageKind::Stretch) => {
            operation == flow_operations::STRETCH
                || (operation == flow_operations::STAR_STRETCH && bands.has_star_layer())
        }
        (StepKind::Operation { operation, .. }, StageKind::Combine) => {
            adjustment_kind(operation).is_some()
        }
        // A heal of one input, of one layer, or of the whole picture.
        (StepKind::Heal { .. }, StageKind::Linear) => step.band.is_some(),
        (StepKind::Heal { .. }, StageKind::Combine | StageKind::Finish) => true,
        (
            StepKind::Operation {
                operation,
                settings,
                ..
            },
            StageKind::Finish,
        ) => {
            operation == "Curves"
                || operation == "Saturation"
                || operation == tone_step::OPERATION
                // The layered stretch needs the whole image.
                || (operation == flow_operations::OUTPUT_STRETCH
                    && output_stretch::config_of(settings)
                        .is_some_and(|config| !matches!(config.model, StretchModel::Layered(_))))
        }
        _ => false,
    }
}

/// The band of a stretch table row that leaves samples as they are: the row
/// of the planes a store holds stretched for display.
pub(super) const DISPLAY_ROW: usize = usize::MAX;

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
        Some(MaskRecord::Painted(mask)) => mask
            .drawn_strokes()
            .flat_map(|stroke| segments(&stroke))
            .collect(),
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
pub(super) fn mask_words(document: &Document, mask: &Option<MaskRecord>) -> Option<String> {
    match mask {
        Some(MaskRecord::Painted(mask)) => Some(format!(
            "painted mask, {}",
            counted(mask.strokes.len(), "stroke")
        )),
        Some(MaskRecord::Range { .. }) => Some("range mask".to_string()),
        Some(MaskRecord::Shared { mask, inverted }) => {
            let name = document
                .mask(*mask)
                .map_or("a mask that is gone", |mask| mask.name.as_str());
            Some(if *inverted {
                format!("where {name} is weak")
            } else {
                format!("where {name} is strong")
            })
        }
        None => None,
    }
}

/// A layer that shows nothing yet: what every entry of a scene starts from.
fn plain_layer() -> Layer {
    Layer {
        mark: Mark::Layer,
        clip: None,
        computed: None,
        band: 0,
        blended: None,
        color: [1.0; 3],
        opacity: 1.0,
        blend: Blend::Normal,
        curve: 0.0,
        mask: None,
        adjustment: None,
        own: false,
    }
}

fn blend_of(blend: BlendMode) -> Blend {
    match blend {
        BlendMode::Normal => Blend::Normal,
        BlendMode::Screen => Blend::Screen,
        BlendMode::Add => Blend::Add,
        BlendMode::Multiply => Blend::Multiply,
        BlendMode::Lighten => Blend::Lighten,
        BlendMode::Darken => Blend::Darken,
        BlendMode::Luminance => Blend::Luminance,
    }
}

/// Adds the adjustments that belong to one layer or one group, after the
/// entry they change. An adjustment the frame has no room for is recorded
/// as left out.
fn push_own_adjustments(
    layers: &mut Vec<Layer>,
    omitted: &mut Vec<String>,
    mask_for: &mut impl FnMut(Vec<Segment>) -> Result<Option<usize>, ()>,
    owner: &LayerRecord,
    applied: &[Uuid],
) {
    for step in owner.steps.iter().filter(|step| applied.contains(&step.id)) {
        let StepKind::Operation { operation, .. } = &step.kind else {
            continue;
        };
        let Some(kind) = adjustment_kind(operation) else {
            continue;
        };
        match mask_for(mask_strokes(&step.mask)) {
            Ok(mask) if layers.len() < MAX_LAYERS => layers.push(Layer {
                opacity: step.opacity,
                mask,
                adjustment: Some((kind, amount(step))),
                own: true,
                ..plain_layer()
            }),
            _ => omitted.push(format!("{} of {}", operation, owner.name)),
        }
    }
}

/// The stretch a Stretch step holds, as Seiza reads it. `None` when the
/// step's settings are not a stretch Seiza knows.
pub(super) fn stretch_model(step: &StepRecord) -> Option<StretchModel> {
    let StepKind::Operation { settings, .. } = &step.kind else {
        return None;
    };
    serde_json::from_value(settings.get("model")?.clone()).ok()
}

/// What one input is drawn from in a view: its state, the tile channel
/// that holds it in that state, the channel to blend in for the background
/// correction with how much of it, and its stretches in order.
///
/// What the canvas cannot draw is recorded as left out: a background
/// correction for a view before the input's tools, where the store
/// corrected the tools' result, and a stretch that needs the engine.
fn band_source(
    document: &Document,
    bands: &Bands,
    band: usize,
    plan: &ChannelPlan,
    omitted: &mut Vec<String>,
) -> (
    BandState,
    usize,
    Option<(usize, f32)>,
    Vec<StretchModel>,
    bool,
) {
    let label = channel_label(&plan.channel);
    let strength = plan
        .background
        .and_then(|id| document.step(id))
        .map_or(0.0, |(_, step)| amount(step));
    let mut state = BandState {
        tools: plan.tools.is_some(),
        background: strength,
    };
    let (plane, corrected) = bands.planes(band, state);
    if corrected.is_none() {
        if strength > 0.0 && bands.corrected[band].is_some() {
            omitted.push(format!("the background of {label} without its tools"));
        }
        state.background = 0.0;
    }
    // A channel whose stretches need the whole image is drawn from the
    // plane the store stretched, as it is.
    if let (PlannedStretch::Document(steps), Some(stretched)) =
        (&plan.stretch, bands.stretched_plane(band))
    {
        let layered = steps
            .iter()
            .filter_map(|id| document.step(*id))
            .any(|(_, step)| matches!(stretch_model(step), Some(StretchModel::Layered(_))));
        if layered {
            return (state, stretched, None, Vec::new(), true);
        }
    }
    // A band the store holds stretched for display has no stretch to draw.
    if bands.is_display(band) {
        return (
            state,
            plane,
            corrected.map(|corrected| (corrected, state.background)),
            Vec::new(),
            false,
        );
    }
    let stages = match &plan.stretch {
        PlannedStretch::Preview => Vec::new(),
        PlannedStretch::Document(steps) => steps
            .iter()
            .filter_map(|id| document.step(*id))
            .filter_map(|(_, step)| match stretch_model(step) {
                Some(StretchModel::Layered(_)) | None => {
                    omitted.push(format!("a stretch of {label} that needs the engine"));
                    None
                }
                Some(model) => Some(model),
            })
            .collect(),
    };
    (
        state,
        plane,
        corrected.map(|corrected| (corrected, state.background)),
        stages,
        false,
    )
}

/// The input whose stretch a band is drawn with: its own, or, for the
/// stars a tool took out of an input, that input's.
fn stretch_source<'a>(
    bands: &Bands,
    inputs: &'a [ChannelPlan],
    band: usize,
    plan: &'a ChannelPlan,
) -> (usize, &'a ChannelPlan) {
    plan.channel
        .strip_suffix("@stars")
        .or_else(|| plan.channel.strip_suffix("@line"))
        // Stars the store holds stretched for display are drawn as they
        // are, not through their input's stretch.
        .filter(|_| !bands.is_display(band))
        .and_then(|input| {
            Some((
                bands.band_of(input)?,
                inputs.iter().find(|other| other.channel == input)?,
            ))
        })
        .unwrap_or((band, plan))
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
            before_transfer: before_stretch,
            offset: *offset,
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
/// `picture_samples` are RGB samples of the combined picture, with the id of
/// the sampling they came from, for the output stretches Seiza resolves from
/// the picture. `chains` keeps output stretches resolved before.
pub(super) fn scene(
    document: &Document,
    bands: &Bands,
    view: &View,
    version: u64,
    picture_samples: Option<(u64, &[f32])>,
    chains: &mut output_stretch::ResolvedChains,
) -> (Scene, Vec<BandState>) {
    let planned = plan(document, view);
    let mut states = vec![BandState::default(); bands.len()];
    let mut layers: Vec<Layer> = Vec::new();
    let mut layer_bands = Vec::new();
    let mut layer_stretches: Vec<Vec<StretchModel>> = Vec::new();
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
    let find = |id: &Uuid| document.layers().iter().find(|layer| layer.id == *id);
    // The mask of each layer drawn so far, for the layers clipped to it.
    let mut mask_of: HashMap<Uuid, Option<usize>> = HashMap::new();
    // The entries whose layer may have a mask computed from inputs: the
    // entry's place in `layers`, and the layer.
    let mut masked: Vec<(usize, &LayerRecord)> = Vec::new();
    // A channel view draws its channel alone; a blended view draws the stack.
    let entries: Vec<StackEntry> = if planned.stack.is_empty() {
        (0..planned.channels.len())
            .map(|channel| StackEntry::Band {
                layer: Uuid::nil(),
                channel,
                clip: None,
            })
            .collect()
    } else {
        planned.stack.clone()
    };
    for entry in entries {
        let name = |layer: Option<&LayerRecord>| {
            layer.map_or("a layer".to_string(), |layer| layer.name.clone())
        };
        if layers.len() >= MAX_LAYERS {
            let left_out = match entry {
                StackEntry::Band { layer, .. }
                | StackEntry::Adjustment { layer }
                | StackEntry::GroupEnd { group: layer } => name(find(&layer)),
                StackEntry::GroupBegin { .. } => continue,
            };
            omitted.push(left_out);
            continue;
        }
        let (at, layer, clip) = match entry {
            StackEntry::GroupBegin { .. } => {
                layers.push(Layer {
                    mark: Mark::GroupBegin,
                    ..plain_layer()
                });
                continue;
            }
            StackEntry::GroupEnd { group } => {
                // A group that is gone still has to end, so the layers
                // after it are not blended into it.
                let found = find(&group);
                let mask = found.map_or(Ok(None), |group| mask_for(mask_strokes(&group.mask)));
                if mask.is_err() {
                    omitted.push(format!("the mask of {}", name(found)));
                }
                let mask = mask.unwrap_or(None);
                mask_of.insert(group, mask);
                layers.push(Layer {
                    mark: Mark::GroupEnd,
                    opacity: found.map_or(
                        1.0,
                        |group| {
                            if group.visible {
                                group.opacity
                            } else {
                                0.0
                            }
                        },
                    ),
                    blend: found.map_or(Blend::Normal, |group| blend_of(group.blend)),
                    mask,
                    ..plain_layer()
                });
                if let Some(group) = found {
                    masked.push((layers.len() - 1, group));
                }
                if let Some(group) = found {
                    push_own_adjustments(
                        &mut layers,
                        &mut omitted,
                        &mut mask_for,
                        group,
                        &planned.own_steps,
                    );
                }
                continue;
            }
            StackEntry::Adjustment { layer } => (None, find(&layer), None),
            StackEntry::Band {
                layer,
                channel,
                clip,
            } => (Some(channel), find(&layer), clip),
        };
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
            layers.push(Layer {
                opacity: if layer.visible { layer.opacity } else { 0.0 },
                mask,
                adjustment: Some((kind, amount_in(operation, settings))),
                ..plain_layer()
            });
            masked.push((layers.len() - 1, layer));
            continue;
        };
        let Some(band) = bands.band_of(&channel.channel) else {
            continue;
        };
        let (state, plane, blended, own_stages, display) =
            band_source(document, bands, band, channel, &mut omitted);
        states[band] = state;
        // The stars a tool took out of an input are stretched as that
        // input is, so the two go back together.
        let (stretched_as, like) = stretch_source(bands, &planned.inputs, band, channel);
        let stages = if stretched_as == band {
            own_stages
        } else {
            let (like_state, _, _, stages, _) =
                band_source(document, bands, stretched_as, like, &mut omitted);
            states[stretched_as] = like_state;
            stages
        };
        // A plane the store stretched has a table row of its own, which
        // leaves its samples as they are.
        let stretched_as = if display { DISPLAY_ROW } else { stretched_as };
        let shown = Layer {
            band: plane,
            blended,
            ..plain_layer()
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
            layers.push(shown);
            layer_bands.push(stretched_as);
            layer_stretches.push(stages.clone());
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
        mask_of.insert(layer.id, mask);
        // A clipped layer shows only inside the mask of the layer it is
        // clipped to, and not at all while that layer is hidden.
        let base = clip.and_then(|base| find(&base));
        let clip_mask = match base {
            None => None,
            Some(base) => match mask_of.get(&base.id) {
                Some(mask) => *mask,
                // The layer it is clipped to is not in this view.
                None => {
                    let mask = mask_for(mask_strokes(&base.mask)).unwrap_or(None);
                    mask_of.insert(base.id, mask);
                    mask
                }
            },
        };
        let hidden = !layer.visible || base.is_some_and(|base| !base.visible);
        // One input's layer on its own is drawn in gray at full strength.
        let gray = planned.gray;
        layers.push(Layer {
            color: if gray {
                [1.0; 3]
            } else {
                layer.color.unwrap_or([1.0; 3])
            },
            opacity: match (gray, hidden) {
                (true, _) => 1.0,
                (false, true) => 0.0,
                (false, false) => layer.opacity,
            },
            blend: if gray {
                Blend::Normal
            } else {
                blend_of(layer.blend)
            },
            mask,
            clip: clip_mask,
            ..shown
        });
        layer_bands.push(stretched_as);
        layer_stretches.push(stages.clone());
        let shown_at = layers.len() - 1;
        masked.push((shown_at, layer));
        for step in input_heals {
            let mask = mask_for(step_strokes(step));
            push_heal(&mut heals, &mut omitted, shown_at, true, step, mask);
        }
        // The layer's own edits change it before it is blended. Its heals
        // change the stretched layer; its adjustments follow.
        let own_heals = layer.steps.iter().filter(|step| {
            planned.own_steps.contains(&step.id) && matches!(step.kind, StepKind::Heal { .. })
        });
        for step in own_heals {
            let mask = mask_for(step_strokes(step));
            push_heal(&mut heals, &mut omitted, shown_at, false, step, mask);
        }
        push_own_adjustments(
            &mut layers,
            &mut omitted,
            &mut mask_for,
            layer,
            &planned.own_steps,
        );
    }

    // The output stretches drawn, resolved in order: each from what the
    // ones before it left.
    let drawn_finish: Vec<&StepRecord> = document
        .stage(StageKind::Finish)
        .steps
        .iter()
        .filter(|step| {
            planned.finish.contains(&step.id) && is_drawn(StageKind::Finish, step, bands)
        })
        .collect();
    let configs: Vec<_> = drawn_finish
        .iter()
        .filter_map(|step| match &step.kind {
            StepKind::Operation {
                operation,
                settings,
                ..
            } if operation == flow_operations::OUTPUT_STRETCH => {
                output_stretch::config_of(settings)
            }
            _ => None,
        })
        .collect();
    let mut resolved = chains.resolve(&configs, picture_samples).into_iter();
    let mut finish = Vec::new();
    for step in drawn_finish {
        let stretch = if step.kind.name() == flow_operations::OUTPUT_STRETCH {
            match resolved.next() {
                Some(Ok(stretch)) => Some(stretch),
                Some(Err(why)) => {
                    omitted.push(match why {
                        output_stretch::NotDrawn::NeedsSamples => {
                            "an output stretch, until the picture has been sampled".to_string()
                        }
                        output_stretch::NotDrawn::Refused(reason) => {
                            format!("an output stretch Seiza could not resolve: {reason}")
                        }
                        output_stretch::NotDrawn::NeedsImage => {
                            "an output stretch that needs the engine".to_string()
                        }
                    });
                    continue;
                }
                None => continue,
            }
        } else {
            None
        };
        if finish.len() == MAX_FINISH_STEPS {
            omitted.push(step.kind.name().to_string());
            continue;
        }
        let Ok(mask) = mask_for(step_strokes(step)) else {
            omitted.push(step.kind.name().to_string());
            continue;
        };
        let kind = match &step.kind {
            _ if let Some(plan) = stretch => FinishKind::Stretch(plan),
            StepKind::Operation {
                operation,
                settings,
                ..
            } if operation == tone_step::OPERATION => {
                let mut tone = tone_step::from_settings(settings);
                // Texture leaves the picture's noise alone: measured from
                // the samples, as the steps before this one leave them.
                if tone.texture != 0.0
                    && let Some((id, samples)) = picture_samples
                {
                    tone.noise = chains.noise(id, &finish, || {
                        crate::canvas::picture_noise(&finish, samples, super::PICTURE_PATCHES)
                    });
                }
                FinishKind::Tone(tone)
            }
            StepKind::Heal { offset, shift, .. } => FinishKind::Heal {
                offset: *offset,
                shift: std::array::from_fn(|channel| shift.get(channel).copied().unwrap_or(0.0)),
            },
            StepKind::Operation { operation, .. } if operation == "Saturation" => {
                FinishKind::Saturation(amount(step))
            }
            StepKind::Operation { .. } => FinishKind::Curves(amount(step).clamp(-1.0, 1.0)),
        };
        // A heal with no stroke heals nothing.
        if matches!(kind, FinishKind::Heal { .. }) && mask.is_none() {
            continue;
        }
        finish.push(FinishStep {
            kind,
            opacity: step.opacity.clamp(0.0, 1.0),
            mask,
        });
    }
    // The shared masks the entries are behind: the entry's place in
    // `layers`, the mask, whether it is inverted, and a name to report.
    let mut wanted: Vec<(usize, Uuid, bool, String)> = masked
        .into_iter()
        .filter_map(|(at, layer)| match &layer.mask {
            Some(MaskRecord::Shared { mask, inverted }) => {
                Some((at, *mask, *inverted, layer.name.clone()))
            }
            _ => None,
        })
        .collect();
    // A view of a mask draws white behind that mask, so the mask shows.
    if let Subject::Mask(id) = &view.subject {
        if layers.len() < MAX_LAYERS {
            layers.push(Layer {
                mark: Mark::Solid,
                ..plain_layer()
            });
            wanted.push((layers.len() - 1, *id, false, "the mask".to_string()));
        } else {
            omitted.push("the mask".to_string());
        }
    }
    // Each input of a mask is read with the stretch the view gives it,
    // from a row of the stretch tables: a layer's row when a layer shows
    // the input with that stretch, or a row of its own.
    let mut computed: Vec<ComputedMask> = Vec::new();
    for (at, id, inverted, name) in wanted {
        let shared = document.mask(id);
        let inputs = shared.map_or(&[][..], |shared| shared.inputs.as_slice());
        let mut read = [None; 2];
        let mut found = 0;
        for (slot, input) in read.iter_mut().zip(inputs) {
            let Some((plan, band)) = planned
                .inputs
                .iter()
                .find(|plan| plan.channel == *input)
                .zip(bands.band_of(input))
            else {
                continue;
            };
            let (state, plane, blended, own_stages, display) =
                band_source(document, bands, band, plan, &mut omitted);
            states[band] = state;
            // The stars a tool took out of an input are stretched as that
            // input is, so the two go back together.
            let (stretched_as, like) = stretch_source(bands, &planned.inputs, band, plan);
            let stages = if stretched_as == band {
                own_stages
            } else {
                let (like_state, _, _, stages, _) =
                    band_source(document, bands, stretched_as, like, &mut omitted);
                states[stretched_as] = like_state;
                stages
            };
            // A plane the store stretched has a table row of its own, which
            // leaves its samples as they are.
            let stretched_as = if display { DISPLAY_ROW } else { stretched_as };
            let known =
                layer_bands
                    .iter()
                    .zip(&layer_stretches)
                    .position(|(row_band, row_stages)| {
                        *row_band == stretched_as && *row_stages == stages
                    });
            let row = match known {
                Some(row) => row,
                None if layer_bands.len() < MAX_LAYERS => {
                    layer_bands.push(stretched_as);
                    layer_stretches.push(stages.clone());
                    layer_bands.len() - 1
                }
                None => continue,
            };
            *slot = Some(MaskBand {
                band: plane,
                blended,
                stretch_row: row,
            });
            found += 1;
        }
        // The touch-ups: strokes that add to the mask, and strokes that
        // take away from it, each as a mask of its own.
        let strokes = |mode: StrokeMode| -> Vec<Segment> {
            shared
                .map_or(&[][..], |shared| shared.touch_ups.as_slice())
                .iter()
                .filter(|stroke| stroke.mode == mode)
                .flat_map(segments)
                .map(|segment| Segment {
                    value: 1.0,
                    ..segment
                })
                .collect()
        };
        let touch_ups = mask_for(strokes(StrokeMode::Add))
            .and_then(|added| Ok((added, mask_for(strokes(StrokeMode::Subtract))?)));
        let (Some(shared), Ok((added, removed))) = (shared, touch_ups) else {
            // A layer is never drawn without its mask.
            layers[at].opacity = 0.0;
            omitted.push(format!("{name}, for its mask"));
            continue;
        };
        if found != inputs.len()
            || inputs.is_empty()
            || inputs.len() > 2
            || computed.len() == MAX_COMPUTED_MASKS
        {
            layers[at].opacity = 0.0;
            omitted.push(format!("{name}, for its mask"));
            continue;
        }
        layers[at].computed = Some(computed.len());
        computed.push(ComputedMask {
            bands: read,
            power_of_inverse: shared.shape == MaskShape::PowerOfInverse,
            reach: shared.reach,
            smoothing: shared.smoothing,
            added,
            removed,
            inverted,
        });
    }
    let scene = Scene {
        computed,
        layers,
        layer_bands,
        layer_stretches,
        finish,
        heals,
        masks,
        omitted,
        stretches: Arc::default(),
        version,
    };
    (scene, states)
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

    /// The selected shared mask.
    pub(super) fn selected_mask(&self) -> Option<&SharedMask> {
        match &self.selected {
            Row::Mask(id) => self.document.mask(*id),
            _ => None,
        }
    }

    /// Whether the mask brush has something to paint: the canvas draws
    /// masks for layers, for a layer's own adjustments and for Finish
    /// steps, and touch-ups for shared masks.
    pub(super) fn can_mask(&self) -> bool {
        match &self.selected {
            // A layer behind a shared mask is not painted on: its mask is
            // touched up on the mask's own row.
            Row::Layer(id) => !self
                .layer(*id)
                .is_some_and(|layer| matches!(layer.mask, Some(MaskRecord::Shared { .. }))),
            Row::Finish(_) | Row::Mask(_) => true,
            Row::LayerStep { step, .. } => self
                .document
                .step(*step)
                .is_some_and(|(_, step)| matches!(step.kind, StepKind::Operation { .. })),
            _ => false,
        }
    }

    /// What the brush paints: the layer, the step, or the shared mask the
    /// selected row is.
    pub(super) fn mask_target(&self) -> Option<Uuid> {
        match &self.selected {
            Row::Layer(id) | Row::Mask(id) => Some(*id),
            Row::LayerStep { step, .. } | Row::Finish(step) => Some(*step),
            _ => None,
        }
    }

    /// The blur of the selected row's painted mask, when it has one.
    pub(super) fn mask_blur(&self) -> Option<f32> {
        let painted = |record: &Option<MaskRecord>| match record {
            Some(MaskRecord::Painted(mask)) => Some(mask.blur),
            _ => None,
        };
        match &self.selected {
            Row::Layer(id) => painted(&self.layer(*id)?.mask),
            Row::LayerStep { step, .. } | Row::Finish(step) => {
                painted(&self.document.step(*step)?.1.mask)
            }
            _ => None,
        }
    }

    /// Whether the brush's mask has a stroke that Undo stroke can remove.
    pub(super) fn can_undo_stroke(&self) -> bool {
        self.mask_target()
            .is_some_and(|target| self.document.stroke_count(target) > 0)
    }

    /// The part of the document the picture keeps, as left, top, right and
    /// bottom: the person's rectangle, the store's automatic crop, or all.
    pub(super) fn crop_rect(&self) -> Option<[f32; 4]> {
        let rect = |x: u32, y: u32, width: u32, height: u32| {
            [
                x as f32,
                y as f32,
                x.saturating_add(width) as f32,
                y.saturating_add(height) as f32,
            ]
        };
        let automatic = self
            .band_info
            .automatic_crop
            .map(|[x, y, width, height]| rect(x, y, width, height));
        match self.document.crop {
            CropRecord::Whole => None,
            CropRecord::Automatic => automatic,
            // A drawn rectangle stays inside what every input covers, even
            // after the store is prepared again.
            CropRecord::Region {
                x,
                y,
                width,
                height,
            } => {
                let drawn = rect(x, y, width, height);
                let inside = automatic.unwrap_or([0.0, 0.0, f32::MAX, f32::MAX]);
                let kept = [
                    drawn[0].max(inside[0]),
                    drawn[1].max(inside[1]),
                    drawn[2].min(inside[2]),
                    drawn[3].min(inside[3]),
                ];
                (kept[0] < kept[2] && kept[1] < kept[3])
                    .then_some(kept)
                    .or(automatic)
            }
        }
    }

    /// The strokes shown over the image while a brush is in hand: the
    /// selected row's mask.
    fn selected_strokes(&self) -> Vec<Segment> {
        if let Some(layer) = self.selected_layer() {
            return mask_strokes(&layer.mask);
        }
        if let Some(mask) = self.selected_mask() {
            return mask.touch_ups.iter().flat_map(segments).collect();
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

    /// What the canvas samples of the combined picture for the output
    /// stretches that Seiza resolves from the picture. `None` leaves the
    /// request the canvas has, because it already asks for the picture as
    /// it is; `Some(None)` withdraws it, because the document has no such
    /// stretch.
    #[allow(clippy::option_option)]
    fn picture_request(&mut self) -> Option<Option<Arc<PictureRequest>>> {
        let wanted = self
            .document
            .stage(StageKind::Finish)
            .steps
            .iter()
            .any(|step| {
                let StepKind::Operation {
                    operation,
                    settings,
                    ..
                } = &step.kind
                else {
                    return false;
                };
                // An output stretch resolved from the picture's statistics,
                // and Texture, which measures the picture's noise, read
                // samples of the picture.
                let stretch = operation == flow_operations::OUTPUT_STRETCH
                    && output_stretch::config_of(settings).is_some_and(|config| {
                        output_stretch::needs_samples(&config)
                            && !matches!(config.model, StretchModel::Layered(_))
                    });
                let texture = operation == tone_step::OPERATION
                    && tone_step::from_settings(settings).texture != 0.0;
                step.enabled && (stretch || texture)
            });
        // The histogram of the final picture is made from the samples too.
        let wanted = wanted || self.histogram_wanted();
        if !wanted {
            self.picture_samples = None;
            return self.picture_wanted.take().map(|_| None);
        }
        let (mut scene, states) = scene(
            &self.document,
            &self.band_info,
            &Place::Final.view(),
            self.scene_version,
            None,
            &mut output_stretch::ResolvedChains::default(),
        );
        scene.stretches = self.analysis(&states);
        let crop = self.crop_rect();
        let (width, height) = self.canvas.read(|shared| shared.canvas.document_size);
        let rect = crop.unwrap_or([0.0, 0.0, width as f32, height as f32]);
        let (patches, size) = picture_patches(rect);
        // The picture is the store's tiles, the scene's layers with their
        // stretches, masks and heals, the inputs' states and the crop.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&self.source_generation, &mut hasher);
        std::hash::Hash::hash(
            &format!(
                "{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
                scene.layers,
                scene.layer_bands,
                scene.layer_stretches,
                scene.computed,
                scene.heals,
                states,
                crop,
            ),
            &mut hasher,
        );
        // A painted mask can have many segments; their numbers are hashed
        // as they are.
        for segment in scene.masks.iter().flatten() {
            let Segment {
                from,
                to,
                radius,
                hardness,
                value,
            } = segment;
            for number in [from[0], from[1], to[0], to[1], *radius, *hardness, *value] {
                std::hash::Hash::hash(&number.to_bits(), &mut hasher);
            }
        }
        let id = std::hash::Hasher::finish(&hasher);
        if self.picture_wanted == Some(id) {
            return None;
        }
        self.picture_wanted = Some(id);
        Some(Some(Arc::new(PictureRequest {
            id,
            scene,
            patches,
            size,
        })))
    }

    /// Turns the selected Tone and color step's HDR on or off.
    pub(super) fn set_tone_hdr(&mut self, on: bool) {
        let Some(id) = self.selected_step() else {
            return;
        };
        let Some((_, step)) = self.document.step(id) else {
            return;
        };
        let StepKind::Operation { settings, .. } = &step.kind else {
            return;
        };
        let mut settings = settings.clone();
        if !settings.is_object() {
            settings = tone_step::new_settings();
        }
        settings["hdr"] = on.into();
        self.document.set_step_settings(id, settings);
    }

    /// How much the selected Finish step's mask covers each of the
    /// picture's `samples` samples, patch after patch, as the sampling read
    /// them. `None` when the selected row is not a Finish step with a mask,
    /// or the samples are not of the current patches.
    fn masked_area(&self, samples: usize) -> Option<Vec<f32>> {
        let Row::Finish(id) = &self.selected else {
            return None;
        };
        // Samples of an earlier sampling stand in while new ones are read;
        // they may be of another crop, so their patches are not these.
        if self.picture_samples.as_ref().map(|(id, _)| *id) != self.picture_wanted {
            return None;
        }
        let (_, step) = self.document.step(*id)?;
        let strokes = step_strokes(step);
        if strokes.is_empty() {
            return None;
        }
        let (width, height) = self.canvas.read(|shared| shared.canvas.document_size);
        let rect = self
            .crop_rect()
            .unwrap_or([0.0, 0.0, width as f32, height as f32]);
        let (patches, size) = picture_patches(rect);
        let size = size as usize;
        let covers: Vec<f32> = patches
            .iter()
            .flat_map(|origin| {
                seiza_compose::render::paint_mask(
                    &strokes,
                    seiza_compose::ReferenceRegion {
                        x: origin[0] as usize,
                        y: origin[1] as usize,
                        width: size,
                        height: size,
                    },
                )
            })
            .collect();
        (covers.len() == samples).then_some(covers)
    }

    /// Whether the inspector shows the histogram of the final picture: for
    /// the Final picture and for a Finish step.
    pub(super) fn histogram_wanted(&self) -> bool {
        matches!(self.selected, Row::Final | Row::Finish(_))
    }

    /// Takes the samples of the combined picture the render thread read,
    /// and draws the output stretches from them.
    pub(super) fn follow_picture(&mut self) {
        let Some((id, samples)) = self.canvas.read(|shared| shared.picture.clone()) else {
            return;
        };
        if self
            .picture_samples
            .as_ref()
            .is_some_and(|(known, _)| *known == id)
        {
            return;
        }
        self.picture_samples = Some((id, samples.clone()));
        if let Some(report) = self.parity_report.take() {
            self.write_parity_report(report, samples);
        }
        self.refresh_canvas();
    }

    /// Each band's stretch analysis at these background strengths. An
    /// analysis is kept, so a pane that returns to a place does not wait.
    pub(super) fn analysis(&mut self, states: &[BandState]) -> Arc<Vec<BandStretch>> {
        if let Some((_, known)) = self.analysed.iter().find(|(at, _)| at == states) {
            return known.clone();
        }
        let stretches = Arc::new(self.band_info.stretches(states).unwrap_or_default());
        self.analysed
            .insert(0, (states.to_vec(), stretches.clone()));
        self.analysed.truncate(8);
        stretches
    }

    /// Sends the canvas what the document now asks it to draw: one scene
    /// per pane.
    pub(super) fn refresh_canvas(&mut self) {
        self.scene_version += 1;
        let probe = self.picture_request();
        // Samples of an earlier picture stand in until the new ones are
        // read, so an output stretch keeps its place while the picture
        // changes.
        let samples = self.picture_samples.clone();
        let mut chains = std::mem::take(&mut self.resolved_chains);
        let selected = self.selected.place(self.alone);
        let mut places = panes(self.split, &selected, &self.document);
        // While a mask is edited a person may watch the final picture in
        // place of the mask, to judge a change by its result.
        if self.mask_shows_picture && self.split == Split::One {
            for place in &mut places {
                if matches!(place, Place::Mask(_)) {
                    *place = Place::Final;
                }
            }
        }
        self.pins.resize(places.len(), None);
        for (place, pin) in places.iter_mut().zip(&self.pins) {
            if let Some(pinned) = pin {
                *place = pinned.clone();
            }
        }
        let scenes: Vec<Scene> = places
            .iter()
            .map(|place| {
                let (mut scene, states) = scene(
                    &self.document,
                    &self.band_info,
                    &place.view(),
                    self.scene_version,
                    samples
                        .as_ref()
                        .map(|(id, samples)| (*id, samples.as_slice())),
                    &mut chains,
                );
                // The stretch is applied to the corrected samples, so each
                // band's analysis follows its background strength.
                scene.stretches = self.analysis(&states);
                scene
            })
            .collect();
        self.histogram = self.histogram_wanted().then_some(()).and_then(|()| {
            let (_, samples) = samples.as_ref()?;
            let (scene, _) = scene(
                &self.document,
                &self.band_info,
                &Place::Final.view(),
                self.scene_version,
                self.picture_samples
                    .as_ref()
                    .map(|(id, samples)| (*id, samples.as_slice())),
                &mut chains,
            );
            // A Finish step with a mask shows the histogram of its area: the
            // samples its strokes cover at least half.
            let covers = self.masked_area(samples.len() / 3);
            let pixels = samples.as_chunks::<3>().0;
            let chosen: Vec<f32> = match &covers {
                Some(covers) => pixels
                    .iter()
                    .zip(covers)
                    .filter(|(_, cover)| **cover >= 0.5)
                    .flat_map(|(pixel, _)| *pixel)
                    .collect(),
                // Every fourth pixel of the samples is plenty for a
                // histogram, and keeps a slider's drag smooth.
                None => pixels.iter().step_by(4).flatten().copied().collect(),
            };
            let finished = crate::canvas::finished_samples(&scene.finish, &chosen);
            // An HDR picture is clipped at its peak, as the canvas shows it.
            let peak = seiza_compose::picture::Picture {
                finish: scene.finish.clone(),
                ..Default::default()
            }
            .peak();
            let mut histogram = crate::histogram::Histogram::of(&finished, 1, peak);
            histogram.masked = covers.is_some();
            Some(Arc::new(histogram))
        });
        self.resolved_chains = chains;
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
        let crop = self.crop_rect();
        self.canvas.edit(|canvas| {
            if let Some(probe) = probe {
                canvas.probe = probe;
            }
            canvas.scenes = scenes;
            canvas.strokes = overlay;
            canvas.strokes_replaced += 1;
            canvas.wipe = wipe;
            canvas.crop = crop;
        });
    }

    /// Ends a brush stroke. A mask stroke joins the selected row's mask. A
    /// heal stroke becomes a step where the selection is: of the selected
    /// input, of the selected layer, or of the combined picture.
    pub(super) fn commit_stroke(&mut self, stroke: Stroke) {
        match self.tool {
            Tool::Pan | Tool::Crop => {}
            Tool::Brush => {
                // The brush adds to a mask, or erases from it, as the
                // person chose.
                let mode = if self.brush_removes {
                    StrokeMode::Subtract
                } else {
                    StrokeMode::Add
                };
                let stroke = Stroke { mode, ..stroke };
                match &self.selected {
                    Row::Layer(id) => {
                        let _ = self.document.paint_layer_mask(*id, stroke);
                    }
                    Row::LayerStep { step, .. } | Row::Finish(step) => {
                        let _ = self.document.paint_step_mask(*step, stroke);
                    }
                    Row::Mask(mask) => {
                        self.document.touch_up_mask(*mask, stroke);
                    }
                    _ => {}
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
                let row = match self.selected.clone() {
                    Row::Input(channel) | Row::InputStep { channel, .. } => self
                        .document
                        .insert_step(StageKind::Linear, usize::MAX, step.for_band(&channel))
                        .ok()
                        .map(|()| Row::InputStep { channel, step: id }),
                    Row::Layer(layer) | Row::LayerStep { layer, .. } => {
                        let shows_pixels = self
                            .layer(layer)
                            .is_some_and(|layer| layer.source.shows_pixels());
                        (shows_pixels && self.document.insert_layer_step(layer, step).is_ok())
                            .then_some(Row::LayerStep { layer, step: id })
                    }
                    Row::Combined | Row::Finish(_) | Row::Final | Row::Mask(_) => self
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
            SliderId::ToolParam(index) => self.tool_slider_moved(index, position),
            SliderId::FinishParam(index) => {
                let Some(id) = self.selected_step() else {
                    return;
                };
                let Some((_, step)) = self.document.step(id) else {
                    return;
                };
                let StepKind::Operation { settings, .. } = &step.kind else {
                    return;
                };
                let Some((key, _, low, high)) = tone_step::control(index) else {
                    return;
                };
                let mut settings = settings.clone();
                if !settings.is_object() {
                    settings = tone_step::new_settings();
                }
                settings[key] = f64::from(low + (high - low) * position).into();
                self.document.set_step_settings(id, settings);
            }
            SliderId::StretchParam(index) => {
                let Some(id) = self.selected_step() else {
                    return;
                };
                let Some((_, step)) = self.document.step(id) else {
                    return;
                };
                let StepKind::Operation { settings, .. } = &step.kind else {
                    return;
                };
                let kind = settings["model"]["type"].as_str().unwrap_or_default();
                let Some((key, _, low, high)) = stretch_settings(kind).get(index) else {
                    return;
                };
                let mut settings = settings.clone();
                settings["model"][*key] = (low + (high - low) * f64::from(position)).into();
                keep_stretch_valid(&mut settings["model"], key);
                self.document.set_step_settings(id, settings);
            }
            SliderId::Amount => {
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
                let amount = amount_range(operation)
                    .map_or(position, |(low, high)| low + (high - low) * position);
                let changed = with_amount(operation, settings, amount);
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
            SliderId::Reach => {
                // The slider covers reaches from a quarter to four, evenly
                // in powers of two.
                if let Row::Mask(id) = &self.selected
                    && let Some(mask) = self.document.mask_mut(*id)
                {
                    mask.reach = 2f32.powf(position * 4.0 - 2.0);
                }
            }
            SliderId::BrushSize => {
                // From a fine brush to a broad one, in points on screen.
                self.brush_size = 2.0 + position * position * 198.0;
            }
            SliderId::MaskBlur => {
                // From none to 64 document pixels.
                if let Some(id) = self.mask_target() {
                    self.document.set_mask_blur(id, position * position * 64.0);
                }
            }
            SliderId::Smoothing => {
                // The slider covers blurs from none to 32 pixels.
                if let Row::Mask(id) = &self.selected
                    && let Some(mask) = self.document.mask_mut(*id)
                {
                    mask.smoothing = position * 32.0;
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
                let id = *id;
                if slider == SliderId::Opacity {
                    if let Some(layer) = self.document.layer_mut(id) {
                        layer.opacity = position;
                    }
                    return;
                }
                self.set_tone(id, slider == SliderId::Hue, position);
            }
            SliderId::ToneHue(index) | SliderId::ToneSaturation(index) => {
                if let Some(id) = self.document.layers().get(index).map(|layer| layer.id) {
                    self.set_tone(id, matches!(slider, SliderId::ToneHue(_)), position);
                }
            }
        }
    }

    /// Sets a layer's hue or its saturation from a slider position, from 0
    /// to 1. The color keeps its brightness, which is the layer's share of
    /// the layers' sum.
    fn set_tone(&mut self, id: Uuid, hue_moved: bool, position: f32) {
        let Some(layer) = self.document.layer_mut(id) else {
            return;
        };
        let (hue, saturation, value) = hue_of(layer.color.unwrap_or([1.0; 3]));
        let (hue, saturation) = if hue_moved {
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
            this.open_menu = None;
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

    /// One line that is always on screen: what is being edited, what it
    /// applies to, and what the tool in hand will change. Editing a mask is
    /// marked, because a stroke then changes where layers show and not the
    /// picture itself.
    fn context_strip(&self) -> impl IntoElement {
        let layer_words = |id: &Uuid| {
            self.layer(*id)
                .map_or("a layer that is gone".to_string(), |layer| {
                    match &layer.source {
                        LayerSource::Adjustment { .. } => {
                            format!("the adjustment {}, of every layer beneath it", layer.name)
                        }
                        LayerSource::Group => format!("the group {}", layer.name),
                        _ => format!("the layer {}", layer.name),
                    }
                })
        };
        let step_name = |id: &Uuid| {
            self.document
                .step(*id)
                .map_or("a step that is gone", |(_, step)| step.kind.name())
                .to_string()
        };
        let what = match &self.selected {
            Row::Input(channel) => format!("the input {}", channel_label(channel)),
            Row::InputStep { channel, step } => {
                let found = self.document.step(*step).map(|(_, step)| step);
                let shared = found.is_some_and(|step| step.band.is_none());
                if let Some(group) = found.and_then(|step| self.group_applies_to(step)) {
                    format!("{}, of {group}", step_name(step))
                } else if shared {
                    format!("{}, of every input", step_name(step))
                } else {
                    format!(
                        "{}, of the input {} only",
                        step_name(step),
                        channel_label(channel)
                    )
                }
            }
            Row::Layer(id) => layer_words(id),
            Row::LayerStep { layer, step } => {
                format!("{}, of {} only", step_name(step), layer_words(layer))
            }
            Row::Mask(id) => format!(
                "the mask {}, shared by {}",
                self.document
                    .mask(*id)
                    .map_or("that is gone", |mask| mask.name.as_str()),
                counted(self.document.layers_using_mask(*id).count(), "layer")
            ),
            Row::Combined => "the combined picture".to_string(),
            Row::Finish(step) => format!("{}, of the whole picture", step_name(step)),
            Row::Final => "the final picture".to_string(),
        };
        let on_a_mask = matches!(self.selected, Row::Mask(_));
        let tool = match (self.tool, &self.selected) {
            (Tool::Pan, _) => None,
            (Tool::Brush, _) if self.brush_removes => {
                Some("The mask brush erases from this mask.".to_string())
            }
            (Tool::Brush, Row::Mask(_)) => Some("The mask brush adds to this mask.".to_string()),
            (Tool::Brush, _) => Some("The mask brush paints where this applies.".to_string()),
            (Tool::Heal, _) => Some("Heal adds a step here.".to_string()),
            (Tool::Crop, _) => {
                Some("Crop sets the rectangle the picture is cropped to.".to_string())
            }
        };
        let marked = on_a_mask || self.tool == Tool::Brush;
        div()
            .flex_none()
            .w_full()
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_1()
            .bg(rgb(if marked { MASK_CONTEXT } else { BACKGROUND }))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(if marked { TEXT } else { QUIET }))
                    .child(if marked { "Editing a mask" } else { "Editing" }),
            )
            .child(what)
            .children(tool.map(|tool| div().text_xs().child(tool)))
    }

    // ---- the bar over the canvas ----------------------------------------

    /// The document's name and how the canvas is divided.
    fn top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let splits = Split::ALL
            .into_iter()
            .enumerate()
            .map(|(index, split)| {
                Self::act(
                    split.name(),
                    ("split", index),
                    self.split == split,
                    cx,
                    move |this| this.set_split(split),
                )
            })
            .collect::<Vec<_>>();
        div()
            .flex_none()
            .h(px(44.0))
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .bg(rgb(PANE))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pr_2()
                    .child(img(brand_mark()).size(px(22.0)))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("PSF Studio"),
                    ),
            )
            .child(div().w(px(1.0)).h(px(20.0)).bg(rgb(CONTROL)))
            .child(div().px_2().child(self.picture_name()))
            .child(
                div()
                    .id("open-files")
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(CONTROL))
                    .child("Open")
                    // Disabled while a picture is prepared or tools run.
                    .when(self.is_running(), |open| open.text_color(rgb(QUIET)))
                    .when(!self.is_running(), |open| {
                        open.cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pick_files(cx);
                                cx.notify();
                            }))
                    }),
            )
            .child(div().pl_3().child(Self::note("View")))
            .child(Self::segmented().children(splits))
            .child(
                div()
                    .flex_1()
                    .pl_3()
                    .overflow_hidden()
                    .text_xs()
                    .text_color(rgb(QUIET))
                    .when_some(self.run_progress(), |status, (what, share, elapsed)| {
                        let seconds = elapsed.as_secs();
                        status
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(120.0))
                                    .h(px(6.0))
                                    .rounded_sm()
                                    .bg(rgb(CONTROL))
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded_sm()
                                            .bg(rgb(SELECTED))
                                            // Without a share, the bar fills
                                            // and empties each second.
                                            .w(relative(share.unwrap_or(
                                                (elapsed.as_millis() % 1000) as f32 / 1000.0,
                                            ))),
                                    ),
                            )
                            .child(format!("{}:{:02}", seconds / 60, seconds % 60))
                            .child(div().overflow_hidden().child(what))
                    })
                    .when(
                        !self.is_running() && self.run_status().starts_with("Not "),
                        |status| status.child(self.run_status().to_string()),
                    ),
            )
            .when(self.is_running(), |bar| {
                bar.child(Self::act("Stop", "stop-run", false, cx, |this| {
                    this.stop_run()
                }))
            })
            .child(Self::act("?", "help", Self::help_shown(), cx, |_| {
                Self::toggle_help()
            }))
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
                let left_out = (!omitted.is_empty())
                    .then(|| format!("Not drawn here: {}.", omitted.join(", ")));
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
        let title = format!("{} — PSF Studio", self.picture_name());
        if self.window_title != title {
            window.set_window_title(&title);
            self.window_title = title;
        }
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
            .child(self.context_strip())
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
