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

use super::{picture_patches, Bands, Drag, Scene, SliderId, Studio, Tool};
use crate::canvas::{Adjustment, ComputedMask, Layer, Mark, MaskBand};
use crate::derived::{DerivedKey, DerivedKind};
use crate::image_steps::Accepts;
use crate::output_stretch::needs_image;
use crate::store_tiles::{BandState, BandStretch, Stretch};
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
pub(super) const ADJUSTMENTS: [(&str, Adjustment); 4] = [
    ("Exposure", Adjustment::Exposure),
    ("Curves", Adjustment::Curves),
    ("Saturation", Adjustment::Saturation),
    (GREEN_REDUCTION, Adjustment::GreenReduction),
];

/// The narrowband balance's settings a person moves: the key, its label,
/// and its range.
pub(super) const BALANCE_CONTROLS: [(&str, &str, f64, f64); 3] = [
    ("shadow_protection", "Shadow protection", 0.0, 1.0),
    ("oiii_boost", "OIII boost", 0.25, 4.0),
    ("sii_boost", "SII boost", 0.25, 4.0),
];

/// The adjustment that takes green down toward the brighter of red and
/// blue, as a narrowband palette's green excess is taken out.
pub(super) const GREEN_REDUCTION: &str = flow_operations::GREEN_REDUCTION;

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
    // Two adjustments on top that PSF Guard's flow does not have, to start
    // the picture's own edits. They stay out of PSF Guard's settings.
    for name in ["Curves", "Saturation"] {
        document.add_picture_step(operation(name, Accepts::Display));
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

/// The most characters a picture's name keeps, as PSF Guard's server does.
const MAX_PICTURE_NAME: usize = 120;

/// The names documents had before they were named after their picture.
const SETUP_NAMES: [&str; 2] = ["PSF Guard default color", "Default"];

impl Studio {
    /// The picture's name: the document's, or for a document named after
    /// the setup it started from, the name of its store.
    pub(super) fn picture_name(&self) -> String {
        let named = &self.document.name;
        if !SETUP_NAMES.contains(&named.as_str()) {
            return crate::studio_model::shown_name(named).to_string();
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
        GREEN_REDUCTION => Some((0.0, 1.0)),
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
pub(super) const STRETCHES: [(&str, &str); 11] = [
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
    ("layered-hdr", "Layered, HDR"),
];

/// The step setting that holds the sky floor of a stretch with none of its
/// own: beside the stretch's model, which Seiza reads, and which PSF Guard's
/// settings leave out.
pub(super) const SKY_FLOOR: &str = "sky_floor";

/// The core compression's threshold, from and to, in noise sigmas.
pub(super) const CORE_THRESHOLD: (f64, f64) = (5.0, 1000.0);

/// The key of a stretch step's core compression, kept beside its model.
pub(super) const CORE_COMPRESSION: &str = "core_compression";

/// A stretch step's core compression, when it takes out any of the core's
/// range.
pub(super) fn core_compression(
    settings: &serde_json::Value,
) -> Option<seiza_stretch::CoreCompression> {
    let core: seiza_stretch::CoreCompression =
        serde_json::from_value(settings.get(CORE_COMPRESSION)?.clone()).ok()?;
    (core.amount > 0.0).then_some(core)
}

/// The highest sky floor, as the layered stretches' own goes.
pub(super) const MAX_SKY_FLOOR: f64 = 0.5;

/// The stage that takes a stretched sky up to `floor` to black and stretches
/// the range above it to reach white again: PixInsight's histogram
/// transformation with its shadows at the floor and midtones at the middle,
/// which is the layered stretches' sky floor in an SDR picture. After an HDR
/// stretch it shapes the range up to white, so the headroom keeps.
pub(super) fn sky_floor_stage(floor: f64) -> Option<StretchModel> {
    (floor > 0.0).then(|| {
        serde_json::from_value(serde_json::json!({
            "type": "ht",
            "shadows": floor.min(MAX_SKY_FLOOR),
            "midtone": 0.5,
            "highlights": 1.0,
        }))
        .ok()
    })?
}

/// The settings of one of Seiza's stretches that a slider sets: Seiza's
/// name for the setting, a label, and the range the slider covers.
pub(super) fn stretch_settings(kind: &str) -> &'static [(&'static str, &'static str, f64, f64)] {
    match kind {
        "layered" => &[
            ("target_background", "Background", 0.05, 0.5),
            ("aggressiveness", "Black point", 0.0, 1.0),
            ("dynamic_range_compression", "Star cores", 0.0, 1.0),
            ("contrast_recovery", "Contrast", 0.0, 1.0),
            ("sky_floor", "Sky floor", 0.0, 0.5),
        ],
        "layered-hdr" => &[
            ("target_background", "Background", 0.05, 0.5),
            ("aggressiveness", "Black point", 0.0, 1.0),
            ("contrast_recovery", "Contrast", 0.0, 1.0),
            ("knee", "Lift above white starts at", 0.5, 0.95),
            ("core_compression", "Core compression", 0.0, 1.0),
            (
                "core_threshold",
                "Compression starts at, sigmas",
                10.0,
                500.0,
            ),
            ("core_scale", "Compressed structure, pixels", 2.0, 64.0),
            ("sky_floor", "Sky floor", 0.0, 0.5),
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
        "layered-hdr" => json!({
            "type": kind, "target_background": 0.15, "aggressiveness": 0.5,
            "contrast_recovery": 0.0, "knee": 0.75,
            "core_compression": 0.7, "core_threshold": 100.0, "core_scale": 12.0,
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

/// Whether an adjustment layer's step changes the picture as a Finish step
/// does, rather than as one of the canvas's adjustments: a stretch, Tone, a
/// heal or a tool.
pub(super) fn is_step_layer(step: &StepRecord) -> bool {
    !matches!(&step.kind, StepKind::Operation { operation, .. } if adjustment_kind(operation).is_some())
}

/// Whether the canvas draws a step, or only keeps it in the document.
pub(super) fn is_drawn(stage: StageKind, step: &StepRecord, bands: &Bands) -> bool {
    match (&step.kind, stage) {
        // A tool on a layer or the picture is drawn once it has run.
        (StepKind::Operation { .. }, StageKind::Combine | StageKind::Finish)
            if crate::studio_model::is_tool_step(step) =>
        {
            tool_plane(bands, step.id, 0, false).is_some()
        }
        (StepKind::Operation { operation, .. }, StageKind::Linear) => {
            let background = operation == flow_operations::BACKGROUND;
            // The store's red, green and blue hold the calibration.
            (operation == flow_operations::COLOR_CALIBRATION && bands.is_calibrated())
                // A step that changes the inputs is drawn when the store
                // holds an input after it.
                || ((background
                    || crate::studio_model::is_tool_step(step)
                    || operation == flow_operations::DECONVOLUTION)
                    && bands.has_step_anywhere(step.id, background))
        }
        // Every stretch of an input is drawn, the layered ones included.
        (StepKind::Operation { .. }, StageKind::Stretch) if stretch_model(step).is_some() => true,
        // The narrowband balance is drawn while it is on.
        (
            StepKind::Operation {
                operation,
                settings,
                ..
            },
            StageKind::Stretch,
        ) if operation == flow_operations::NARROWBAND => settings["balance"].is_object(),
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
                        .is_some_and(|config| !needs_image(&config.model)))
        }
        _ => false,
    }
}

/// The band of a stretch table row that leaves samples as they are: the row
/// of planes worked out already stretched, such as a tool's result.
pub(super) const DISPLAY_ROW: usize = usize::MAX;

/// Rows from here up leave samples as they are.
pub(super) const DISPLAY_FLOOR: usize = DISPLAY_ROW - 65_536;

/// The row of a band whose plane the store holds stretched for display: it
/// leaves samples as they are, and still names the band.
pub(super) fn display_row_of(band: usize) -> usize {
    DISPLAY_ROW - 1 - band.min(65_534)
}

/// The band a stretch table row draws, a band the store stretched among
/// them; `None` for a plane worked out already stretched.
pub(super) fn band_of_row(row: usize) -> Option<usize> {
    match row {
        DISPLAY_ROW => None,
        row if row >= DISPLAY_FLOOR => Some(DISPLAY_ROW - 1 - row),
        row => Some(row),
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

/// The tile channel of one channel of what a late tool step made, when the
/// canvas has it: its result, or the stars it took out.
fn tool_plane(bands: &Bands, step: Uuid, channel: u8, stars: bool) -> Option<usize> {
    bands
        .derived_planes()
        .into_iter()
        .find(|(key, _)| {
            key.kind
                == DerivedKind::Tool {
                    step,
                    channel,
                    stars,
                }
        })
        .map(|(_, plane)| plane)
}

/// The words for a tool step whose result the canvas does not have.
fn not_run(step: &StepRecord, on: &str) -> String {
    format!(
        "{} on {on}, which has not run for these edits",
        step.kind.name()
    )
}

/// What a scene's narrowband balance is resolved from: the balance's
/// settings from the document's Narrowband step, and H-alpha's, OIII's and
/// SII's bands and their rows among the scene's stretches.
pub(super) struct BalanceSource {
    /// Changes when anything the balance is resolved from does.
    pub id: u64,
    pub options: seiza_compose::NarrowbandBalanceOptions,
    /// H-alpha, OIII and SII: each one's band, and its row in the scene.
    pub channels: Vec<(usize, usize)>,
}

/// What `scene`'s narrowband balance is resolved from. `None` when the
/// document balances nothing, or the scene draws no H-alpha and OIII.
pub(super) fn balance_source(
    document: &Document,
    bands: &Bands,
    layer_bands: &[usize],
    layer_stretches: &[Vec<Stretch>],
    states: &[BandState],
) -> Option<BalanceSource> {
    let step = document
        .stage(StageKind::Stretch)
        .steps
        .iter()
        .find(|step| step.enabled && step.kind.name() == flow_operations::NARROWBAND)?;
    let StepKind::Operation { settings, .. } = &step.kind else {
        return None;
    };
    let balance = settings.get("balance").filter(|value| !value.is_null())?;
    let mut options: seiza_compose::NarrowbandBalanceOptions =
        serde_json::from_value(balance.clone()).ok()?;
    let channels: Vec<(usize, usize)> = ["ha", "oiii", "sii"]
        .iter()
        .filter_map(|name| {
            let band = bands.band_of(name)?;
            let row = layer_bands
                .iter()
                .position(|known| band_of_row(*known) == Some(band))?;
            Some((band, row))
        })
        .collect();
    // H-alpha first, then OIII.
    let names: Vec<&str> = channels
        .iter()
        .filter_map(|(band, _)| bands.names.get(*band).map(String::as_str))
        .collect();
    if names.first() != Some(&"ha") || names.get(1) != Some(&"oiii") {
        return None;
    }
    options.has_sii = names.len() == 3;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(
        &format!(
            "{options:?}{:?}{:?}",
            channels
                .iter()
                .map(|(band, row)| (band, &layer_stretches[*row], states.get(*band)))
                .collect::<Vec<_>>(),
            bands.names
        ),
        &mut hasher,
    );
    Some(BalanceSource {
        id: std::hash::Hasher::finish(&hasher),
        options,
        channels,
    })
}

/// The output stretches of adjustment layers that have other layers above
/// them, by their steps' ids: each changes only what is beneath it, so it
/// is resolved from samples of that. A stretch with only other steps on the
/// picture above it is on top.
pub(super) fn beneath_other_layers(document: &Document) -> std::collections::HashSet<Uuid> {
    let layers = document.layers();
    let mut found = std::collections::HashSet::new();
    let mut above_only_steps = true;
    for layer in layers.iter().rev() {
        let step = layer.source.adjustment_step();
        let on_picture = layer.group.is_none() && step.is_some_and(is_step_layer);
        if !layer.visible {
            continue;
        }
        if let Some(step) = step
            && output_stretch_config(step).is_some()
            && (!above_only_steps || layer.group.is_some())
        {
            found.insert(step.id);
        }
        if !on_picture {
            above_only_steps = false;
        }
    }
    found
}

/// An output stretch step's settings, when it is one.
fn output_stretch_config(step: &StepRecord) -> Option<seiza_stretch::StretchConfig> {
    match &step.kind {
        StepKind::Operation {
            operation,
            settings,
            ..
        } if operation == flow_operations::OUTPUT_STRETCH => output_stretch::config_of(settings),
        _ => None,
    }
}

/// What a Finish step reads besides itself, wherever it sits.
struct StepContext<'a> {
    bands: &'a Bands,
    /// Samples of the combined picture, with the id of their sampling.
    picture_samples: Option<(u64, &'a [f32])>,
    chains: &'a mut output_stretch::ResolvedChains,
}

/// What the canvas draws for a Finish step, or `None`, with the reason in
/// `omitted` when there is one. `stretches` holds the output stretches
/// resolved for the picture; `before` are the steps drawn before this one.
fn finish_kind(
    step: &StepRecord,
    stretches: &mut HashMap<Uuid, Result<seiza_stretch::StretchPlan, output_stretch::NotDrawn>>,
    context: StepContext,
    before: &[FinishStep],
    omitted: &mut Vec<String>,
) -> Option<FinishKind> {
    if output_stretch_config(step).is_some() {
        return match stretches.remove(&step.id)? {
            Ok(plan) => Some(FinishKind::Stretch(plan)),
            Err(why) => {
                omitted.push(match why {
                    output_stretch::NotDrawn::NeedsSamples => {
                        "an output stretch, until the picture has been sampled".to_string()
                    }
                    output_stretch::NotDrawn::NeedsSamplesBeneath => {
                        "an output stretch, until the layers beneath it have been sampled"
                            .to_string()
                    }
                    output_stretch::NotDrawn::Refused(reason) => {
                        format!("an output stretch Seiza could not resolve: {reason}")
                    }
                    output_stretch::NotDrawn::NeedsImage => {
                        "an output stretch that needs the engine".to_string()
                    }
                });
                None
            }
        };
    }
    Some(match &step.kind {
        StepKind::Operation {
            operation,
            settings,
            ..
        } if operation == tone_step::OPERATION => {
            let mut tone = tone_step::from_settings(settings);
            // Texture leaves the picture's noise alone: measured from the
            // samples, as the steps before this one leave them.
            if tone.texture != 0.0
                && let Some((id, samples)) = context.picture_samples
            {
                tone.noise = context.chains.noise(id, before, || {
                    crate::canvas::picture_noise(before, samples, super::PICTURE_PATCHES)
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
    })
}

/// The lists of a scene that an adjustment layer adds to.
struct StackParts<'a> {
    layers: &'a mut Vec<Layer>,
    layer_bands: &'a mut Vec<usize>,
    layer_stretches: &'a mut Vec<Vec<Stretch>>,
    heals: &'a mut Vec<LayerHeal>,
    steps: &'a mut Vec<FinishKind>,
    stacked: &'a mut Vec<FinishStep>,
}

/// What an adjustment layer added to the scene.
enum Pushed {
    Nothing,
    /// One entry, at the end of the layers.
    Layer,
    /// This many entries, at the end of the layers, each limited by the
    /// adjustment layer's shared mask.
    Channels(usize),
    /// The image a tool made, in place of every layer beneath it.
    Replaced,
}

/// Adds an adjustment layer that holds a Finish step or a tool: the step
/// changes the layers beneath it. A tool's layer is drawn from the image
/// the tool made of them, once it has run.
fn push_step_layer(
    parts: &mut StackParts,
    omitted: &mut Vec<String>,
    mask_for: &mut impl FnMut(Vec<Segment>) -> Option<usize>,
    stretches: &mut HashMap<Uuid, Result<seiza_stretch::StretchPlan, output_stretch::NotDrawn>>,
    context: StepContext,
    layer: &LayerRecord,
    step: &StepRecord,
) -> Pushed {
    let bands = context.bands;
    if crate::studio_model::is_tool_step(step) {
        if tool_plane(bands, step.id, 0, false).is_none() {
            omitted.push(not_run(step, "the layers beneath it"));
            return Pushed::Nothing;
        }
        let mask = mask_for(layer_strokes(layer));
        // In full, at the top, the tool's image covers every layer beneath
        // it.
        let covers = layer.opacity >= 1.0 && layer.mask.is_none() && layer.group.is_none();
        if covers {
            parts.layers.clear();
            parts.layer_bands.clear();
            parts.layer_stretches.clear();
            parts.heals.clear();
            parts.steps.clear();
            parts.stacked.clear();
            push_tool_image(
                parts.layers,
                parts.layer_bands,
                parts.layer_stretches,
                bands,
                step.id,
                false,
            );
            return Pushed::Replaced;
        }
        // Otherwise, its first channel replaces what is beneath and the
        // others add to it, each at the layer's opacity within its mask:
        // the image blended over what is beneath, in a group over its
        // members beneath.
        let first = parts.layers.len();
        push_tool_image(
            parts.layers,
            parts.layer_bands,
            parts.layer_stretches,
            bands,
            step.id,
            false,
        );
        for channel in &mut parts.layers[first..] {
            channel.opacity = layer.opacity;
            channel.mask = mask;
        }
        return Pushed::Channels(parts.layers.len() - first);
    }
    if !is_drawn(StageKind::Finish, step, bands) {
        return Pushed::Nothing;
    }
    // Every kind of mask on a layer is drawn: painted and shared here, a
    // range mask on the entry once the stack is built.
    let before = parts.stacked.clone();
    let Some(kind) = finish_kind(step, stretches, context, &before, omitted) else {
        return Pushed::Nothing;
    };
    // A heal is limited to its stroke, then to its layer's painted mask, as
    // a clip, and its shared mask.
    let heal = matches!(step.kind, StepKind::Heal { .. });

    let (mask, clip) = if heal {
        let stroke = mask_for(step_strokes(step));
        (stroke, mask_for(layer_strokes(layer)))
    } else {
        (mask_for(layer_strokes(layer)), None)
    };
    // A heal with no stroke heals nothing.
    if heal && mask.is_none() {
        return Pushed::Nothing;
    }
    parts.stacked.push(FinishStep {
        kind: kind.clone(),
        opacity: layer.opacity,
        mask: None,
        range: None,
    });
    parts.layers.push(Layer {
        mark: Mark::Step(parts.steps.len()),
        opacity: layer.opacity,
        mask,
        clip,
        ..plain_layer()
    });
    parts.steps.push(kind);
    Pushed::Layer
}

/// Adds the image a tool made, channel by channel over black in the
/// working space: three channels in their primaries, or one in white.
/// Stars are screened over what is beneath them.
fn push_tool_image(
    layers: &mut Vec<Layer>,
    layer_bands: &mut Vec<usize>,
    layer_stretches: &mut Vec<Vec<Stretch>>,
    bands: &Bands,
    step: Uuid,
    stars: bool,
) {
    let primaries = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let channels: Vec<usize> = (0..3)
        .map_while(|channel| tool_plane(bands, step, channel, stars))
        .collect();
    for (index, channel) in channels.iter().enumerate() {
        let color = if channels.len() == 3 {
            primaries[index]
        } else {
            [1.0; 3]
        };
        layers.push(Layer {
            band: *channel,
            color,
            working: true,
            blend: match (stars, index) {
                (true, _) => Blend::Screen,
                (false, 0) => Blend::Normal,
                (false, _) => Blend::Add,
            },
            ..plain_layer()
        });
        layer_bands.push(DISPLAY_ROW);
        layer_stretches.push(Vec::new());
    }
}

/// Whether the canvas draws a step's mask: none, a painted one or a range
/// mask. A shared mask on a step is kept in the document, and the step is
/// left out rather than drawn over the whole picture.
fn draws_step_mask(mask: &Option<MaskRecord>) -> bool {
    matches!(
        mask,
        None | Some(MaskRecord::Painted(_)) | Some(MaskRecord::Range { .. })
    )
}

/// The high end of a range mask that selects up to the brightest, however
/// far above SDR white an HDR picture goes.
pub(super) const RANGE_BRIGHTEST: f32 = 1.0e6;
/// Where the To slider means the brightest.
const RANGE_TOP_POSITION: f32 = 0.995;
/// The widest feather the Softness slider gives.
pub(super) const RANGE_MAX_FEATHER: f32 = 0.5;

/// A record's range mask as Seiza draws it.
fn range_of(mask: &Option<MaskRecord>) -> Option<seiza_compose::picture::RangeMask> {
    match mask {
        Some(MaskRecord::Range { low, high, feather }) => Some(seiza_compose::picture::RangeMask {
            low: *low,
            high: *high,
            feather: *feather,
        }),
        _ => None,
    }
}

/// The planes of the stars of red, green or blue stretched by the document's
/// star stretch, and of them reduced in full with `reduction`, when the
/// stars layer `channel` shows has them: worked out from the whole image,
/// so a plane not done yet is asked for in `wanted` and the stars are drawn
/// as their input is stretched until then. `None` for any other channel,
/// with the star stretch off, or for a store that stretched its stars.
fn stretched_stars_planes(
    document: &Document,
    bands: &Bands,
    channel: &str,
    reduction: Option<f32>,
    wanted: &mut Vec<DerivedKey>,
) -> Option<(usize, Option<(usize, f32)>)> {
    let colors = ["red", "green", "blue"];
    let pick = colors
        .iter()
        .position(|color| crate::studio_model::stars_channel(color) == channel)?;
    let step = document
        .stage(StageKind::Stretch)
        .steps
        .iter()
        .find(|step| step.enabled && step.kind.name() == flow_operations::STAR_STRETCH)?;
    let strength = amount(step).clamp(0.0, 8.0);
    let starless: Vec<usize> = colors
        .iter()
        .map(|color| bands.band_of(color))
        .collect::<Option<_>>()?;
    let stars: Vec<usize> = colors
        .iter()
        .map(|color| bands.band_of(&crate::studio_model::stars_channel(color)))
        .collect::<Option<_>>()?;
    if stars.iter().any(|band| bands.is_display(*band)) {
        return None;
    }
    // Each channel as its star removal left it: the Linear steps up to and
    // including that one.
    let states: Vec<BandState> = colors
        .iter()
        .zip(&starless)
        .map(|(color, band)| {
            let mut reached = Vec::new();
            for listed in crate::studio_model::input_steps(document, color) {
                let Some((stage, step)) = document.step(listed.id) else {
                    continue;
                };
                if stage != StageKind::Linear || !step.enabled {
                    continue;
                }
                let background = step.kind.name() == flow_operations::BACKGROUND;
                reached.push((listed.id, background.then(|| amount(step))));
                if step.kind.name() == flow_operations::STAR_REMOVAL {
                    break;
                }
            }
            bands.state_after(*band, &reached).0
        })
        .chain(stars.iter().map(|_| BandState::default()))
        .collect();
    let key = |reduced: bool| DerivedKey {
        kind: DerivedKind::StretchedStars {
            pick,
            amount_milli: (strength * 1000.0).round() as u32,
            reduced,
        },
        bands: starless.iter().chain(&stars).copied().collect(),
        states: states.clone(),
        range: (0.0, 1.0),
    };
    let mut find = |key: DerivedKey| {
        let found = bands
            .derived_planes()
            .into_iter()
            .find(|(known, _)| *known == key)
            .map(|(_, plane)| plane);
        if found.is_none() {
            wanted.push(key);
        }
        found
    };
    let plane = find(key(false))?;
    let reduced = reduction
        .filter(|amount| *amount > 0.0)
        .and_then(|amount| Some((find(key(true))?, amount)));
    Some((plane, reduced))
}

/// The layer a clipped layer or group is clipped to: the nearest layer
/// beneath it, or beneath a group's members, in the same group, that is not
/// clipped and is not an adjustment.
fn clip_base(document: &Document, id: Uuid) -> Option<&LayerRecord> {
    let layers = document.layers();
    let low = *document.block_of(id)?.start();
    let group = layers.iter().find(|layer| layer.id == id)?.group;
    layers[..low]
        .iter()
        .rev()
        .filter(|other| other.group == group)
        .find(|other| !other.clipped && !other.source.is_adjustment())
}

/// The strokes that limit a layer: its painted mask, or the strokes painted
/// behind its shared mask.
fn layer_strokes(layer: &LayerRecord) -> Vec<Segment> {
    match (&layer.mask, &layer.painted_too) {
        (Some(MaskRecord::Shared { .. }), Some(too)) => {
            mask_strokes(&Some(MaskRecord::Painted(too.clone())))
        }
        _ => mask_strokes(&layer.mask),
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

/// What kind of mask a step has, in a word or two.
fn mask_kind(mask: &Option<MaskRecord>) -> &'static str {
    match mask {
        Some(MaskRecord::Painted(_)) => "painted mask",
        Some(MaskRecord::Range { .. }) => "range mask",
        Some(MaskRecord::Shared { .. }) => "shared mask",
        None => "mask",
    }
}

/// A mask in a few words, for a row or a note.
pub(super) fn mask_words(document: &Document, mask: &Option<MaskRecord>) -> Option<String> {
    match mask {
        Some(MaskRecord::Painted(mask)) => Some(format!(
            "painted mask, {}",
            counted(mask.strokes.len(), "stroke")
        )),
        Some(MaskRecord::Range { low, high, .. }) => Some(if *high >= RANGE_BRIGHTEST {
            format!("brightness above {low:.2}")
        } else {
            format!("brightness {low:.2} to {high:.2}")
        }),
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
        working: false,
        range: None,
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
/// entry they change.
fn push_own_adjustments(
    layers: &mut Vec<Layer>,
    omitted: &mut Vec<String>,
    mask_for: &mut impl FnMut(Vec<Segment>) -> Option<usize>,
    shared: &mut Vec<(usize, Uuid, bool, String)>,
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
        // A shared mask is computed from its inputs.
        let behind = match &step.mask {
            Some(MaskRecord::Shared { mask, inverted }) => Some((*mask, *inverted)),
            _ => None,
        };
        if !draws_step_mask(&step.mask) && behind.is_none() {
            omitted.push(format!(
                "{operation} of {}, whose {} the canvas does not draw",
                owner.name,
                mask_kind(&step.mask)
            ));
            continue;
        }
        let mask = mask_for(mask_strokes(&step.mask));
        layers.push(Layer {
            opacity: step.opacity,
            mask,
            adjustment: Some((kind, amount(step))),
            own: true,
            range: range_of(&step.mask),
            ..plain_layer()
        });
        if let Some((mask, inverted)) = behind {
            shared.push((
                layers.len() - 1,
                mask,
                inverted,
                format!("{operation} of {}", owner.name),
            ));
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
/// What the canvas cannot draw is recorded as left out: a Linear step the
/// store was prepared without, and a stretch the canvas does not know.
fn band_source(
    document: &Document,
    bands: &Bands,
    band: usize,
    plan: &ChannelPlan,
    view_stretch: &[StretchModel],
    omitted: &mut Vec<String>,
) -> (BandState, usize, Option<(usize, f32)>, Vec<Stretch>, bool) {
    let label = channel_label(&plan.channel);
    // The steps the view reached that change the input, each with its
    // strength when it is a background extraction.
    let reached: Vec<(Uuid, Option<f32>)> = plan
        .linear
        .iter()
        .filter_map(|id| {
            let (_, step) = document.step(*id)?;
            let background = step.kind.name() == flow_operations::BACKGROUND;
            Some((*id, background.then(|| amount(step))))
        })
        .collect();
    let (mut state, missing) = bands.state_after(band, &reached);
    for id in missing {
        if let Some((_, step)) = document.step(id) {
            omitted.push(format!(
                "the {} of {label}, which the store was not prepared with in this order",
                step.kind.name().to_lowercase()
            ));
        }
    }
    let (plane, corrected) = bands.planes(band, state);
    if corrected.is_none() {
        state.background = 0.0;
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
        // A channel nothing stretches yet is drawn with the view stretch,
        // and an output leaves it linear.
        PlannedStretch::Preview => view_stretch.iter().cloned().map(Stretch::from).collect(),
        PlannedStretch::Document(steps) => steps
            .iter()
            .filter_map(|id| document.step(*id))
            .flat_map(|(_, step)| match stretch_model(step) {
                None => {
                    omitted.push(format!("a stretch of {label} the canvas does not know"));
                    Vec::new()
                }
                // A stretch with no sky floor of its own takes one after it,
                // and core compression before it.
                Some(model) => {
                    let layered = matches!(
                        model,
                        StretchModel::Layered(_) | StretchModel::LayeredHdr(_)
                    );
                    let settings = match &step.kind {
                        StepKind::Operation { settings, .. } if !layered => Some(settings),
                        _ => None,
                    };
                    let floor = settings
                        .and_then(|settings| settings[SKY_FLOOR].as_f64())
                        .and_then(sky_floor_stage);
                    let core = settings.and_then(core_compression);
                    core.map(Stretch::Core)
                        .into_iter()
                        .chain(std::iter::once(Stretch::Model(model)))
                        .chain(floor.map(Stretch::Model))
                        .collect::<Vec<_>>()
                }
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

/// How much a band layer's stars are reduced, and the inputs reduced
/// together with them: the layer's own amount with its own input, or else
/// its group's with the stars of every layer in the group, so their colors
/// keep. Only stars a tool took out of an input are reduced.
fn star_reduction(document: &Document, layer: &LayerRecord) -> Option<(f32, Vec<String>)> {
    let is_stars = |layer: &LayerRecord| match &layer.source {
        LayerSource::Band { band } if band.ends_with("@stars") => Some(band.clone()),
        _ => None,
    };
    let own = is_stars(layer)?;
    if layer.star_reduction > 0.0 {
        return Some((layer.star_reduction.min(1.0), vec![own]));
    }
    let group = document
        .layers()
        .iter()
        .find(|group| Some(group.id) == layer.group)?;
    if group.star_reduction <= 0.0 {
        return None;
    }
    let inputs = document
        .layers()
        .iter()
        .filter(|member| member.group == Some(group.id))
        .filter_map(is_stars)
        .collect();
    Some((group.star_reduction.min(1.0), inputs))
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

/// Adds a heal of one layer to a scene.
fn push_heal(
    heals: &mut Vec<LayerHeal>,
    layer: usize,
    before_stretch: bool,
    step: &StepRecord,
    mask: Option<usize>,
) {
    let StepKind::Heal { offset, shift, .. } = &step.kind else {
        return;
    };
    // A heal with no stroke heals nothing.
    if let Some(mask) = mask {
        heals.push(LayerHeal {
            layer,
            before_transfer: before_stretch,
            offset: *offset,
            shift: shift.first().copied().unwrap_or(0.0),
            mask,
        });
    }
}

/// What a pane's label names: the place the pane draws, which is the place
/// its scene was built for, and the place a newer scene is being built for,
/// if the pane is on its way there. A pane with no scene yet names the place
/// wanted.
pub(super) fn pane_caption(wanted: &Place, drawn: Option<Place>) -> (Place, Option<Place>) {
    match drawn {
        Some(drawn) if drawn != *wanted => (drawn, Some(wanted.clone())),
        _ => (wanted.clone(), None),
    }
}

/// Whether the document reads samples of its picture: an output stretch
/// resolved from the picture's statistics, or Texture, which measures the
/// picture's noise.
pub(super) fn needs_picture(document: &Document) -> bool {
    let shown = document
        .layers()
        .iter()
        .filter(|layer| layer.visible)
        .filter_map(|layer| layer.source.adjustment_step())
        .chain(
            document
                .stage(StageKind::Finish)
                .steps
                .iter()
                .filter(|step| step.enabled),
        );
    shown.into_iter().any(|step| {
        let StepKind::Operation {
            operation,
            settings,
            ..
        } = &step.kind
        else {
            return false;
        };
        let stretch = operation == flow_operations::OUTPUT_STRETCH
            && output_stretch::config_of(settings).is_some_and(|config| {
                output_stretch::needs_samples(&config) && !needs_image(&config.model)
            });
        let texture =
            operation == tone_step::OPERATION && tone_step::from_settings(settings).texture != 0.0;
        stretch || texture
    })
}

impl Scene {
    /// What the combined picture is made of, as a number that changes when
    /// it does: the store's tiles, the layers with their stretches, masks
    /// and heals, the inputs' states and the crop.
    pub(super) fn picture_id(
        &self,
        source_generation: u64,
        states: &[BandState],
        crop: Option<[f32; 4]>,
    ) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&source_generation, &mut hasher);
        std::hash::Hash::hash(
            &format!(
                "{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
                self.headroom,
                self.knee,
                self.layers,
                self.steps,
                self.layer_bands,
                self.layer_stretches,
                self.computed,
                self.heals,
                states,
                crop,
            ),
            &mut hasher,
        );
        // A painted mask can have many segments; their numbers are hashed
        // as they are.
        for segment in self.masks.iter().flatten() {
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
        std::hash::Hasher::finish(&hasher)
    }
}

/// What the document asks the canvas to draw for a view. Returns the scene,
/// without its stretch analyses, and each band's background strength in it.
/// What the canvas cannot draw is listed in the scene, and a layer whose
/// mask cannot be drawn is not drawn at all. Seiza draws a scene with more
/// than one GPU draw holds in batches, so nothing is left out for its size.
/// `picture_samples` are RGB samples of the combined picture, with the id of
/// the sampling they came from, for the output stretches Seiza resolves from
/// the picture. `chains` keeps output stretches resolved before.
/// `view_stretch` stretches a channel nothing in the view stretches yet; an
/// export gives none, so the view stretch never reaches a file.
pub(super) fn scene(
    document: &Document,
    bands: &Bands,
    view: &View,
    version: u64,
    picture_samples: Option<(u64, &[f32])>,
    chains: &mut output_stretch::ResolvedChains,
    view_stretch: &[StretchModel],
) -> (Scene, Vec<BandState>) {
    let planned = plan(document, view);
    let mut states = vec![BandState::default(); bands.len()];
    let mut layers: Vec<Layer> = Vec::new();
    let mut layer_bands = Vec::new();
    let mut layer_stretches: Vec<Vec<Stretch>> = Vec::new();
    let mut masks: Vec<Vec<Segment>> = Vec::new();
    let mut heals: Vec<LayerHeal> = Vec::new();
    let mut omitted: Vec<String> = Vec::new();
    // Planes worked out from the whole image that layers read and that are
    // not done yet.
    let mut wanted_planes: Vec<DerivedKey> = Vec::new();
    // Strokes as a place in `masks`; `None` when there are none. Seiza
    // draws a picture with more masks than one GPU draw holds in batches.
    let mut mask_for = |strokes: Vec<Segment>| -> Option<usize> {
        if strokes.is_empty() {
            return None;
        }
        masks.push(strokes);
        Some(masks.len() - 1)
    };
    let find = |id: &Uuid| document.layers().iter().find(|layer| layer.id == *id);
    // The mask of each layer drawn so far, for the layers clipped to it.
    let mut mask_of: HashMap<Uuid, Option<usize>> = HashMap::new();
    // The entries whose layer may have a mask computed from inputs: the
    // entry's place in `layers`, and the layer.
    let mut masked: Vec<(usize, &LayerRecord)> = Vec::new();
    // Entries behind a shared mask that is not their layer's own: an own
    // adjustment's, a pass-through group's, or that of the layer they are
    // clipped to. The entry's place, the mask, whether it is inverted, and
    // a name to report.
    let mut shared_wanted: Vec<(usize, Uuid, bool, String)> = Vec::new();
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
    // The Finish stage's steps the canvas draws: those after its last tool
    // step whose result the canvas has, which holds the ones before.
    let finish_steps = &document.stage(StageKind::Finish).steps;
    let mut baked_finish: Option<usize> = None;
    for (at, step) in finish_steps.iter().enumerate() {
        if !crate::studio_model::is_tool_step(step) || !planned.finish.contains(&step.id) {
            continue;
        }
        if tool_plane(bands, step.id, 0, false).is_some() {
            baked_finish = Some(at);
        } else {
            omitted.push(not_run(step, "the picture"));
        }
    }
    let drawn_finish: Vec<&StepRecord> = finish_steps
        .iter()
        .enumerate()
        .filter(|(at, _)| baked_finish.is_none_or(|baked| *at > baked))
        .map(|(_, step)| step)
        .filter(|step| {
            !crate::studio_model::is_tool_step(step)
                && planned.finish.contains(&step.id)
                && is_drawn(StageKind::Finish, step, bands)
        })
        .collect();
    // The output stretches drawn, resolved in the order the picture is
    // made, each from what the ones before it left: those of adjustment
    // layers, beneath the Finish stage, first.
    let stretch_steps: Vec<&StepRecord> = entries
        .iter()
        .filter_map(|entry| match entry {
            StackEntry::Adjustment { layer } => find(layer)
                .filter(|layer| layer.visible)
                .and_then(|layer| layer.source.adjustment_step()),
            _ => None,
        })
        .chain(drawn_finish.iter().copied())
        .filter(|step| {
            is_drawn(StageKind::Finish, step, bands) && output_stretch_config(step).is_some()
        })
        .collect();
    // A stretch with other layers above it is resolved alone, from samples
    // of what is beneath it; the stretches on top, as a chain, from samples
    // of the picture.
    let beneath_others = beneath_other_layers(document);
    let (alone, chained): (Vec<&StepRecord>, Vec<&StepRecord>) = stretch_steps
        .into_iter()
        .partition(|step| beneath_others.contains(&step.id));
    let configs: Vec<_> = chained
        .iter()
        .filter_map(|step| output_stretch_config(step))
        .collect();
    let mut stretches: HashMap<Uuid, Result<_, output_stretch::NotDrawn>> = chained
        .iter()
        .map(|step| step.id)
        .zip(chains.resolve(&configs, picture_samples))
        .collect();
    for step in alone {
        let Some(config) = output_stretch_config(step) else {
            continue;
        };
        let samples = chains.beneath.get(&step.id).cloned();
        let resolved = chains
            .resolve(
                std::slice::from_ref(&config),
                samples
                    .as_ref()
                    .map(|(id, samples)| (*id, samples.as_slice())),
            )
            .pop();
        if let Some(resolved) = resolved {
            stretches.insert(
                step.id,
                resolved.map_err(|why| match why {
                    output_stretch::NotDrawn::NeedsSamples => {
                        output_stretch::NotDrawn::NeedsSamplesBeneath
                    }
                    other => other,
                }),
            );
        }
    }
    // What each layer marked as a step does.
    let mut steps: Vec<FinishKind> = Vec::new();
    // The steps in the stack so far, for Texture, which measures the
    // picture's noise as the steps before it leave it.
    let mut stacked: Vec<FinishStep> = Vec::new();
    for entry in entries {
        let name = |layer: Option<&LayerRecord>| {
            layer.map_or("a layer".to_string(), |layer| layer.name.clone())
        };
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
                let mask = found.and_then(|group| mask_for(layer_strokes(group)));
                mask_of.insert(group, mask);
                // A clipped group shows only inside the mask of the layer
                // beneath its members.
                let base = found
                    .filter(|group| group.clipped)
                    .and_then(|group| clip_base(document, group.id));
                let clip = base.and_then(|base| match mask_of.get(&base.id) {
                    Some(mask) => *mask,
                    None => mask_for(layer_strokes(base)),
                });
                if let Some(base) = base
                    && let Some(MaskRecord::Shared { mask, inverted }) = &base.mask
                {
                    shared_wanted.push((layers.len(), *mask, *inverted, name(found)));
                }
                let hidden_base = base.is_some_and(|base| !base.visible);
                layers.push(Layer {
                    mark: Mark::GroupEnd,
                    clip,
                    opacity: found.map_or(1.0, |group| {
                        if group.visible && !hidden_base {
                            group.opacity
                        } else {
                            0.0
                        }
                    }),
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
                        &mut shared_wanted,
                        group,
                        &planned.own_steps,
                    );
                }
                continue;
            }
            StackEntry::Made { layer, .. } => {
                // The stars a star removal took out, screened back.
                let Some(made) = find(&layer) else { continue };
                let LayerSource::Step { step, .. } = &made.source else {
                    continue;
                };
                if !made.visible || tool_plane(bands, *step, 0, true).is_none() {
                    continue;
                }
                let mask = mask_for(layer_strokes(made));
                // Its own heals fix the stars alone, before they are
                // screened over the picture.
                let own_heals: Vec<&StepRecord> = made
                    .steps
                    .iter()
                    .filter(|own| own.enabled && matches!(own.kind, StepKind::Heal { .. }))
                    .collect();
                layers.push(Layer {
                    mark: Mark::GroupBegin,
                    ..plain_layer()
                });
                push_tool_image(
                    &mut layers,
                    &mut layer_bands,
                    &mut layer_stretches,
                    bands,
                    *step,
                    true,
                );
                for own in own_heals {
                    let StepKind::Heal { offset, shift, .. } = &own.kind else {
                        continue;
                    };
                    // A heal with no stroke heals nothing.
                    if let Some(stroke) = mask_for(step_strokes(own)) {
                        layers.push(Layer {
                            mark: Mark::Step(steps.len()),
                            mask: Some(stroke),
                            ..plain_layer()
                        });
                        steps.push(FinishKind::Heal {
                            offset: *offset,
                            shift: std::array::from_fn(|channel| {
                                shift.get(channel).copied().unwrap_or(0.0)
                            }),
                        });
                    }
                }
                layers.push(Layer {
                    mark: Mark::GroupEnd,
                    opacity: made.opacity,
                    blend: blend_of(made.blend),
                    mask,
                    ..plain_layer()
                });
                masked.push((layers.len() - 1, made));
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
            let (operation, settings) = match &layer.source {
                LayerSource::Adjustment {
                    operation,
                    settings,
                } => (operation, settings),
                LayerSource::Adjust {
                    step:
                        StepRecord {
                            kind:
                                StepKind::Operation {
                                    operation,
                                    settings,
                                    ..
                                },
                            ..
                        },
                } if adjustment_kind(operation).is_some() => (operation, settings),
                LayerSource::Adjust { step } => {
                    if layer.visible {
                        let mut parts = StackParts {
                            layers: &mut layers,
                            layer_bands: &mut layer_bands,
                            layer_stretches: &mut layer_stretches,
                            heals: &mut heals,
                            steps: &mut steps,
                            stacked: &mut stacked,
                        };
                        let pushed = push_step_layer(
                            &mut parts,
                            &mut omitted,
                            &mut mask_for,
                            &mut stretches,
                            StepContext {
                                bands,
                                picture_samples,
                                chains,
                            },
                            layer,
                            step,
                        );
                        match pushed {
                            Pushed::Layer => masked.push((layers.len() - 1, layer)),
                            Pushed::Channels(count) => masked
                                .extend((layers.len() - count..layers.len()).map(|at| (at, layer))),
                            // Every index into the old layers goes with them.
                            Pushed::Replaced => {
                                masked.clear();
                                shared_wanted.clear();
                            }
                            Pushed::Nothing => {}
                        }
                    }
                    continue;
                }
                _ => continue,
            };
            let Some(kind) = adjustment_kind(operation) else {
                continue;
            };
            let mask = mask_for(layer_strokes(layer));
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
            band_source(document, bands, band, channel, view_stretch, &mut omitted);
        states[band] = state;
        // The stars of red, green and blue under the star stretch are drawn
        // from the planes of them stretched, worked out from the whole
        // image at the amount set, and blended toward them reduced.
        let stretched = layer.filter(|_| !display).and_then(|layer| {
            stretched_stars_planes(
                document,
                bands,
                &channel.channel,
                star_reduction(document, layer).map(|(amount, _)| amount),
                &mut wanted_planes,
            )
        });
        // Stars a layer reduces are read blended toward the plane of them
        // reduced, which is worked out from the whole image.
        // A background correction blended in by nothing leaves the slot free.
        let blended = blended.filter(|(_, amount)| *amount > 0.0);
        let blended = match (
            blended,
            layer
                .filter(|_| stretched.is_none())
                .and_then(|layer| star_reduction(document, layer)),
        ) {
            (blended, None) => blended,
            (Some(blended), Some(_)) => {
                omitted.push(format!(
                    "the star reduction of {} with its background corrected",
                    channel_label(&channel.channel)
                ));
                Some(blended)
            }
            (None, Some((amount, inputs))) => {
                let linked: Vec<(usize, BandState)> = inputs
                    .iter()
                    .filter_map(|input| {
                        let other = bands.band_of(input)?;
                        let state = if other == band {
                            state
                        } else {
                            planned
                                .channels
                                .iter()
                                .find(|plan| &plan.channel == input)
                                .map_or_else(BandState::default, |plan| {
                                    band_source(
                                        document,
                                        bands,
                                        other,
                                        plan,
                                        view_stretch,
                                        &mut Vec::new(),
                                    )
                                    .0
                                })
                        };
                        Some((other, state))
                    })
                    .collect();
                linked
                    .iter()
                    .position(|(other, _)| *other == band)
                    .and_then(|pick| {
                        let key = DerivedKey {
                            kind: DerivedKind::ReducedStars(pick),
                            bands: linked.iter().map(|(band, _)| *band).collect(),
                            states: linked.iter().map(|(_, state)| *state).collect(),
                            range: (0.0, 1.0),
                        };
                        let found = bands
                            .derived_planes()
                            .into_iter()
                            .find(|(known, _)| *known == key)
                            .map(|(_, plane)| (plane, amount));
                        if found.is_none() {
                            wanted_planes.push(key);
                        }
                        found
                    })
            }
        };
        // The stars a tool took out of an input are stretched as that
        // input is, so the two go back together.
        let (stretched_as, like) = stretch_source(bands, &planned.inputs, band, channel);
        let stages = if stretched_as == band {
            own_stages
        } else {
            let (like_state, _, _, stages, _) = band_source(
                document,
                bands,
                stretched_as,
                like,
                view_stretch,
                &mut omitted,
            );
            states[stretched_as] = like_state;
            stages
        };
        // A plane the store stretched has a table row of its own, which
        // leaves its samples as they are, and still says whose band it is.
        let stretched_as = if display {
            display_row_of(band)
        } else {
            stretched_as
        };
        // Stretched stars are display samples already.
        let (plane, blended, stretched_as, stages) = match stretched {
            Some((stars, reduced)) => (stars, reduced, DISPLAY_ROW, Vec::new()),
            None => (plane, blended, stretched_as, stages),
        };
        // A layer's own tool step whose result the canvas has: the layer is
        // drawn from that result, which holds its stretch and every own step
        // before the tool.
        let mut baked: Option<(usize, Uuid)> = None;
        if let Some(owner) = layer {
            for (at, step) in owner.steps.iter().enumerate() {
                if !crate::studio_model::is_tool_step(step) || !planned.own_steps.contains(&step.id)
                {
                    continue;
                }
                if tool_plane(bands, step.id, 0, false).is_some() {
                    baked = Some((at, step.id));
                } else {
                    omitted.push(not_run(step, &owner.name));
                }
            }
        }
        let (shown, stretched_as, stages) = match baked {
            Some((_, step)) => (
                Layer {
                    band: tool_plane(bands, step, 0, false).expect("a result just found"),
                    ..plain_layer()
                },
                DISPLAY_ROW,
                Vec::new(),
            ),
            None => (
                Layer {
                    band: plane,
                    blended,
                    ..plain_layer()
                },
                stretched_as,
                stages,
            ),
        };
        // The own steps after the tool's: those before it are in its result.
        let after_baked: Vec<Uuid> = match (baked, layer) {
            (Some((at, _)), Some(owner)) => owner.steps[at + 1..]
                .iter()
                .map(|step| step.id)
                .filter(|id| planned.own_steps.contains(id))
                .collect(),
            _ => planned.own_steps.clone(),
        };
        // The heals of the input alone change its samples before the
        // stretch, wherever the input is shown.
        let input_heals: Vec<&StepRecord> = channel
            .heals
            .iter()
            .filter(|_| baked.is_none())
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
                push_heal(&mut heals, layers.len() - 1, true, step, mask);
            }
            continue;
        };
        let mask = mask_for(layer_strokes(layer));
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
                    let mask = mask_for(layer_strokes(base));
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
        // Clipped to a layer behind a shared mask, it is behind that mask
        // too.
        if let Some(base) = base
            && !gray
            && let Some(MaskRecord::Shared { mask, inverted }) = &base.mask
        {
            shared_wanted.push((shown_at, *mask, *inverted, layer.name.clone()));
        }
        for step in input_heals {
            let mask = mask_for(step_strokes(step));
            push_heal(&mut heals, shown_at, true, step, mask);
        }
        // The layer's own edits change it before it is blended. Its heals
        // change the stretched layer; its adjustments follow.
        let own_heals = layer.steps.iter().filter(|step| {
            after_baked.contains(&step.id) && matches!(step.kind, StepKind::Heal { .. })
        });
        for step in own_heals {
            let mask = mask_for(step_strokes(step));
            push_heal(&mut heals, shown_at, false, step, mask);
        }
        push_own_adjustments(
            &mut layers,
            &mut omitted,
            &mut mask_for,
            &mut shared_wanted,
            layer,
            &after_baked,
        );
        // The stars the layer's star removal took out go back right after
        // it, screened over the stack in the layer's color.
        let stars = baked.and_then(|(at, step)| {
            let record = &layer.steps[at];
            (super::late_tools::stars_shown(record) && !gray)
                .then(|| tool_plane(bands, step, 0, true))
                .flatten()
        });
        if let Some(stars) = stars {
            let shown = &layers[shown_at];
            layers.push(Layer {
                band: stars,
                blend: Blend::Screen,
                color: shown.color,
                opacity: shown.opacity,
                mask: shown.mask,
                clip: shown.clip,
                ..plain_layer()
            });
            layer_bands.push(DISPLAY_ROW);
            layer_stretches.push(Vec::new());
        }
    }

    // A Finish tool step whose result the canvas has: the picture after it
    // is that result, which holds every layer and Finish step before it,
    // with the stars a star removal took out screened back over it.
    if let Some(at) = baked_finish {
        let step = &finish_steps[at];
        layers.clear();
        layer_bands.clear();
        layer_stretches.clear();
        heals.clear();
        masked.clear();
        shared_wanted.clear();
        steps.clear();
        push_tool_image(
            &mut layers,
            &mut layer_bands,
            &mut layer_stretches,
            bands,
            step.id,
            false,
        );
        if super::late_tools::stars_shown(step) {
            push_tool_image(
                &mut layers,
                &mut layer_bands,
                &mut layer_stretches,
                bands,
                step.id,
                true,
            );
        }
    }
    let mut finish = Vec::new();
    for step in drawn_finish {
        let Some(kind) = finish_kind(
            step,
            &mut stretches,
            StepContext {
                bands,
                picture_samples,
                chains,
            },
            &finish,
            &mut omitted,
        ) else {
            continue;
        };
        if !draws_step_mask(&step.mask) {
            omitted.push(format!(
                "{}, whose {} the canvas does not draw",
                step.kind.name(),
                mask_kind(&step.mask)
            ));
            continue;
        }
        let mask = mask_for(step_strokes(step));
        // A heal with no stroke heals nothing.
        if matches!(kind, FinishKind::Heal { .. }) && mask.is_none() {
            continue;
        }
        finish.push(FinishStep {
            kind,
            opacity: step.opacity.clamp(0.0, 1.0),
            mask,
            range: range_of(&step.mask),
        });
    }
    // The shared masks the entries are behind: the entry's place in
    // `layers`, the mask, whether it is inverted, and a name to report.
    // The picture an external editor saved, once it is read: it stands for
    // every layer and step Studio drew, as the editor saw them.
    if document
        .stage(StageKind::Finish)
        .junction
        .edited_file()
        .is_some()
    {
        let channels: Vec<usize> = (0..3)
            .map_while(|channel| {
                bands
                    .derived_planes()
                    .into_iter()
                    .find(|(key, _)| key.kind == DerivedKind::Edited { channel })
                    .map(|(_, plane)| plane)
            })
            .collect();
        if channels.is_empty() {
            omitted.push("the edited file, until it is read".to_string());
        } else {
            layers.clear();
            layer_bands.clear();
            layer_stretches.clear();
            heals.clear();
            masked.clear();
            shared_wanted.clear();
            finish.clear();
            let primaries = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
            for (index, plane) in channels.iter().enumerate() {
                layers.push(Layer {
                    band: *plane,
                    color: if channels.len() == 3 {
                        primaries[index]
                    } else {
                        [1.0; 3]
                    },
                    working: true,
                    blend: if index == 0 {
                        Blend::Normal
                    } else {
                        Blend::Add
                    },
                    ..plain_layer()
                });
                layer_bands.push(DISPLAY_ROW);
                layer_stretches.push(Vec::new());
            }
        }
    }
    // The layers of a group whose layers pass through blend straight onto
    // what is beneath them, each limited by the group's opacity and masks.
    for (at, layer) in masked.clone() {
        for group in document.ancestors(layer.id) {
            let Some(group) = find(&group).filter(|group| group.pass_through) else {
                continue;
            };
            let entry = &mut layers[at];
            entry.opacity *= if group.visible { group.opacity } else { 0.0 };
            let strokes = layer_strokes(group);
            if !strokes.is_empty() {
                if entry.clip.is_none() {
                    entry.clip = mask_for(strokes);
                } else {
                    entry.opacity = 0.0;
                    omitted.push(format!(
                        "{}, clipped and in {}, whose layers pass through with a mask",
                        layer.name, group.name
                    ));
                }
            }
            if let Some(MaskRecord::Shared { mask, inverted }) = &group.mask {
                shared_wanted.push((at, *mask, *inverted, layer.name.clone()));
            }
            // Its own adjustments apply to a group that blends on its own.
            if group.steps.iter().any(|step| step.enabled)
                && !omitted
                    .iter()
                    .any(|words| words.starts_with(&format!("the adjustments of {}", group.name)))
            {
                omitted.push(format!(
                    "the adjustments of {}, whose layers pass through",
                    group.name
                ));
            }
        }
    }
    // A layer's range mask limits its entry: a band layer by the band, an
    // adjustment or a step by what it changes, a group by what it made.
    for (at, layer) in &masked {
        if let Some(range) = range_of(&layer.mask) {
            layers[*at].range = Some(range);
        }
    }
    let mut wanted: Vec<(usize, Uuid, bool, String)> = masked
        .into_iter()
        .filter_map(|(at, layer)| match &layer.mask {
            Some(MaskRecord::Shared { mask, inverted }) => {
                Some((at, *mask, *inverted, layer.name.clone()))
            }
            _ => None,
        })
        .chain(shared_wanted)
        .collect();
    // A view of a mask draws white behind that mask, so the mask shows.
    if let Subject::Mask(id) = &view.subject {
        layers.push(Layer {
            mark: Mark::Solid,
            ..plain_layer()
        });
        wanted.push((layers.len() - 1, *id, false, "the mask".to_string()));
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
                band_source(document, bands, band, plan, view_stretch, &mut omitted);
            states[band] = state;
            // The stars a tool took out of an input are stretched as that
            // input is, so the two go back together.
            let (stretched_as, like) = stretch_source(bands, &planned.inputs, band, plan);
            let stages = if stretched_as == band {
                own_stages
            } else {
                let (like_state, _, _, stages, _) = band_source(
                    document,
                    bands,
                    stretched_as,
                    like,
                    view_stretch,
                    &mut omitted,
                );
                states[stretched_as] = like_state;
                stages
            };
            // A plane the store stretched has a table row of its own, which
            // leaves its samples as they are.
            let stretched_as = if display {
                display_row_of(band)
            } else {
                stretched_as
            };
            let known =
                layer_bands
                    .iter()
                    .zip(&layer_stretches)
                    .position(|(row_band, row_stages)| {
                        *row_band == stretched_as && *row_stages == stages
                    });
            let row = match known {
                Some(row) => row,
                None => {
                    layer_bands.push(stretched_as);
                    layer_stretches.push(stages.clone());
                    layer_bands.len() - 1
                }
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
        let added = mask_for(strokes(StrokeMode::Add));
        let removed = mask_for(strokes(StrokeMode::Subtract));
        let Some(shared) = shared else {
            // A layer is never drawn without its mask.
            layers[at].opacity = 0.0;
            omitted.push(format!("{name}, for its mask"));
            continue;
        };
        if found != inputs.len() || inputs.is_empty() || inputs.len() > 2 {
            layers[at].opacity = 0.0;
            omitted.push(format!("{name}, for its mask"));
            continue;
        }
        // An entry has one computed mask; one behind two is not drawn.
        if layers[at].computed.is_some() {
            layers[at].opacity = 0.0;
            omitted.push(format!("{name}, behind two shared masks"));
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
    // The steps on top of the stack are drawn as Finish steps, which the
    // GPU draws with the pixels around each one: a heal, or Tone's local
    // contrast.
    let mut on_top = Vec::new();
    while let Some(last) = layers.last()
        && let Mark::Step(index) = last.mark
        && last.computed.is_none()
        && last.clip.is_none()
    {
        on_top.push(FinishStep {
            kind: steps[index].clone(),
            opacity: last.opacity,
            mask: last.mask,
            range: last.range,
        });
        layers.pop();
    }
    on_top.reverse();
    // Steps of the stack drawn as Finish steps are part of the combined
    // picture; the Finish stage's own come after them.
    let stacked_finish = on_top.len();
    on_top.append(&mut finish);
    let finish = on_top;
    // The narrowband balance, once resolved for these stretches.
    let balance = balance_source(document, bands, &layer_bands, &layer_stretches, &states)
        .and_then(|source| {
            chains
                .balance
                .as_ref()
                .filter(|(id, _)| *id == source.id)
                .map(|(_, list)| list.clone())
        })
        .unwrap_or_default();
    let scene = Scene {
        computed,
        layers,
        layer_bands,
        layer_stretches,
        finish,
        stacked_finish,
        steps,
        balance,
        heals,
        masks,
        omitted,
        stretches: Arc::default(),
        version,
        headroom: document.headroom.unwrap_or(0.0),
        knee: document.hdr_knee.unwrap_or(tone_step::DEFAULT_HDR_KNEE),
        planes: Arc::new(bands.derived_planes()),
        wanted_planes,
        place: None,
    };
    (scene, states)
}

impl Studio {
    // ---- what the selection is ------------------------------------------

    /// The selected step: an input's, a layer's own, or a Finish step.
    /// The mask the range controls change: the selected step's (an
    /// adjustment layer's step's is its layer's), or the selected layer's.
    pub(super) fn range_mask_mut(&mut self) -> Option<&mut Option<MaskRecord>> {
        if let Some(step) = self.selected_step() {
            return self.document.step_mask_mut(step);
        }
        match self.selected {
            Row::Layer(id) => self.document.layer_mut(id).map(|layer| &mut layer.mask),
            _ => None,
        }
    }

    pub(super) fn selected_step(&self) -> Option<Uuid> {
        match &self.selected {
            Row::InputStep { step, .. } | Row::LayerStep { step, .. } | Row::Finish(step) => {
                Some(*step)
            }
            // An adjustment layer's step.
            Row::Layer(layer) => self
                .layer(*layer)?
                .source
                .adjustment_step()
                .map(|step| step.id),
            _ => None,
        }
    }

    /// The selected adjustment layer that changes the picture as a Finish
    /// step does, with its step.
    pub(super) fn selected_step_layer(&self) -> Option<(&LayerRecord, &StepRecord)> {
        let Row::Layer(id) = &self.selected else {
            return None;
        };
        let layer = self.layer(*id)?;
        let step = layer.source.adjustment_step()?;
        is_step_layer(step).then_some((layer, step))
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
            // Behind a shared mask, painting limits the layer as well; the
            // shared mask is touched up on its own row.
            Row::Layer(id) => self.layer(*id).is_some(),
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
            return layer_strokes(layer);
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
        // Any row of the tree shows the canvas again.
        self.setup_open = false;
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

    /// The view stretch, as the stretches of a channel nothing stretches
    /// yet. Seiza's automatic midtones stretch if its settings do not read.
    pub(super) fn view_stretch_stages(&self) -> Vec<StretchModel> {
        vec![serde_json::from_value(self.view_stretch.clone())
            .unwrap_or(StretchModel::AutoMtf(Default::default()))]
    }

    /// Makes the whole picture HDR, at the headroom it had or two stops, or
    /// SDR.
    /// Moves an HDR picture's headroom by `stops`, within 0.5 and 4.
    pub(super) fn step_headroom(&mut self, stops: f32) {
        if let Some(headroom) = self.document.headroom {
            let headroom = (headroom + stops).clamp(0.5, 4.0);
            self.document.headroom = Some(headroom);
            self.last_headroom = headroom;
        }
    }

    pub(super) fn set_hdr(&mut self, on: bool) {
        self.last_headroom = self.document.headroom.unwrap_or(self.last_headroom);
        self.document.headroom = on.then_some(self.last_headroom);
    }

    /// How much the selected Finish step's mask covers each of the
    /// picture's `samples` samples, patch after patch, as the sampling read
    /// them. `None` when the selected row is not a Finish step with a mask,
    /// or the samples are not of the current patches.
    fn masked_area(&self, samples: usize) -> Option<Vec<f32>> {
        // Samples of an earlier sampling stand in while new ones are read;
        // they may be of another crop, so their patches are not these.
        if self.picture_samples.as_ref().map(|(id, _)| *id) != self.picture_wanted {
            return None;
        }
        let strokes = match (&self.selected, self.selected_step_layer()) {
            (_, Some((layer, step))) => match step.kind {
                StepKind::Heal { .. } => step_strokes(step),
                StepKind::Operation { .. } => layer_strokes(layer),
            },
            (Row::Finish(id), None) => step_strokes(self.document.step(*id)?.1),
            _ => return None,
        };
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
    /// the Final picture, a Finish step, and an adjustment layer that
    /// changes the picture as a Finish step does.
    pub(super) fn histogram_wanted(&self) -> bool {
        matches!(self.selected, Row::Final | Row::Finish(_)) || self.selected_step_layer().is_some()
    }

    /// The histogram of the final picture's samples, once the GPU has read
    /// the ones the picture now asks for. Until then the last one stays.
    pub(super) fn update_histogram(&mut self) {
        let Some((id, samples)) = self.final_samples.clone() else {
            return;
        };
        if self.final_wanted != Some(id) {
            return;
        }
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
            None => samples.to_vec(),
        };
        let mut histogram = crate::histogram::Histogram::of(&chosen, 1, self.final_peak);
        histogram.masked = covers.is_some();
        self.histogram = Some(Arc::new(histogram));
    }

    pub(super) fn follow_picture(&mut self) {
        let sampled = self.canvas.read(|shared| shared.final_picture.clone());
        if let Some((id, samples)) = sampled
            && self
                .final_samples
                .as_ref()
                .is_none_or(|(known, _)| *known != id)
        {
            self.final_samples = Some((id, samples));
            self.update_histogram();
        }
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
        let started = std::time::Instant::now();
        self.note_edit();
        self.scene_version += 1;
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
        // The scenes are built off this thread; samples of an earlier
        // picture stand in until the new ones are read, so an output
        // stretch keeps its place while the picture changes.
        let (width, height) = self.canvas.read(|shared| shared.canvas.document_size);
        self.scene_worker.submit(super::scene_worker::SceneJob {
            document: self.document.clone(),
            band_info: self.band_info.clone(),
            bands_generation: self.bands_generation,
            places: places.clone(),
            view_stretch: self.view_stretch_stages(),
            picture_samples: self.picture_samples.clone(),
            scene_version: self.scene_version,
            histogram_wanted: self.histogram_wanted(),
            source_generation: self.source_generation,
            crop: self.crop_rect(),
            document_size: (width, height),
            store: self.store_path().map(std::path::Path::to_path_buf),
            planes: self.derived.clone(),
        });
        self.places = places;
        // The selected row's mask shows over the image while a person has
        // it switched on, so they see what they have painted.
        let mut overlay = if self.mask_shown {
            self.selected_strokes()
        } else {
            Vec::new()
        };
        // A selected background extraction's protected regions show as
        // outlines.
        if self.selected_background_step().is_some() {
            let zoom = self.canvas.read(|shared| shared.canvas.view.zoom);
            overlay.extend(self.protected_outlines((1.5 * self.scale_factor as f64 / zoom) as f32));
        }
        // Where a heal copies from: the source an Option-click set, as a
        // cross, and the selected heal's source area at half strength.
        let zoom = self.canvas.read(|shared| shared.canvas.view.zoom);
        let line = (1.5 * self.scale_factor as f64 / zoom) as f32;
        if self.tool == Tool::Heal
            && let Some(source) = self.heal_source
        {
            overlay.extend(super::heal::source_cross(source, self.brush_radius(), line));
        }
        if let Some((_, step)) = self.selected_step().and_then(|id| self.document.step(id))
            && let StepKind::Heal { offset, .. } = &step.kind
        {
            overlay.extend(step_strokes(step).into_iter().map(|segment| Segment {
                from: [
                    segment.from[0] + offset[0] as f32,
                    segment.from[1] + offset[1] as f32,
                ],
                to: [
                    segment.to[0] + offset[0] as f32,
                    segment.to[1] + offset[1] as f32,
                ],
                value: 0.5,
                ..segment
            }));
        }
        let wipe = (self.split == Split::Wipe).then_some(self.wipe);
        let crop = self.crop_rect();
        self.canvas.edit(|canvas| {
            canvas.strokes = overlay;
            canvas.strokes_replaced += 1;
            canvas.wipe = wipe;
            canvas.crop = crop;
        });
        if super::timing() {
            eprintln!(
                "refresh: {:.1} ms on the UI thread",
                started.elapsed().as_secs_f64() * 1e3
            );
        }
    }

    /// Takes in what the scene worker built: which samplings the scenes
    /// ask for, and the histogram once its samples are read.
    pub(super) fn follow_scenes(&mut self) {
        // Samples of what is beneath a stretch, or a balance, came in: the
        // scenes are built again with them.
        if self.scene_worker.take_beneath_fresh() {
            self.refresh_canvas();
        }
        // A sampling or a heal's measure builds its scene from what the
        // worker resolved.
        let resolved = self.scene_worker.resolved();
        self.resolved_chains.beneath = resolved.beneath;
        self.resolved_chains.balance = resolved.balance;
        let Some(outcome) = self.scene_worker.take_outcome() else {
            return;
        };
        if outcome.picture_wanted.is_none() {
            self.picture_samples = None;
        }
        self.picture_wanted = outcome.picture_wanted;
        if outcome.final_wanted.is_none() {
            self.histogram = None;
        }
        self.final_wanted = outcome.final_wanted;
        self.final_peak = outcome.final_peak;
        self.update_histogram();
    }

    /// Ends a brush stroke. A mask stroke joins the selected row's mask. A
    /// heal stroke becomes a step where the selection is: of the selected
    /// input, of the selected layer, or of the combined picture.
    pub(super) fn commit_stroke(&mut self, stroke: Stroke) {
        match self.tool {
            Tool::Pan | Tool::Crop | Tool::Protect => {}
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
                // Copy from where an Option-click set, or from three radii
                // to the right.
                let offset = self
                    .heal_offset
                    .unwrap_or([(stroke.radius * 3.0) as i64, 0]);
                if let Some(row) = self.add_heal(stroke, offset) {
                    self.selected = row;
                }
            }
        }
        self.refresh_canvas();
    }

    /// Adds a heal of `stroke`, copying from `offset` away, where the
    /// selection is: to the selected input, to the selected layer, above the
    /// selected adjustment, or on top of the picture. Returns the row it
    /// added, or `None` when the selection has nothing to heal.
    pub(super) fn add_heal(&mut self, stroke: Stroke, offset: [i64; 2]) -> Option<Row> {
        // Matched to its surroundings: the difference between the ring
        // around the stroke and the ring around its source, measured on the
        // picture the heal reads before it is added.
        let shift = self.measure_heal_shift(&stroke, offset);
        let step = StepRecord::new(StepKind::Heal {
            stroke,
            offset,
            shift,
        });
        let id = step.id;
        match self.selected.clone() {
            Row::Input(channel) | Row::InputStep { channel, .. } => self
                .document
                .insert_step(StageKind::Linear, usize::MAX, step.for_band(&channel))
                .ok()
                .map(|()| Row::InputStep { channel, step: id }),
            Row::Layer(layer) | Row::LayerStep { layer, .. } => {
                let shows_pixels = self
                    .layer(layer)
                    .is_some_and(|layer| layer.source.shows_pixels());
                if shows_pixels {
                    self.document
                        .insert_layer_step(layer, step)
                        .is_ok()
                        .then_some(Row::LayerStep { layer, step: id })
                } else {
                    // An adjustment, such as a star removal, has no pixels
                    // of its own: the heal is a layer right above it, on the
                    // picture as it leaves it.
                    self.heal_above(layer, step).map(Row::Layer)
                }
            }
            Row::Combined | Row::Finish(_) | Row::Final | Row::Mask(_) => {
                Some(Row::Layer(self.document.add_picture_step(step)))
            }
        }
    }

    /// Puts a heal layer right above `layer`, in its group, and returns the
    /// heal layer's id. A group's heal is its top member, on what its
    /// members make. `None` when `layer` is gone.
    fn heal_above(&mut self, layer: Uuid, step: StepRecord) -> Option<Uuid> {
        let layers = self.document.layers();
        let at = layers.iter().position(|other| other.id == layer)?;
        let mut heal = crate::studio_model::adjustment_layer(step);
        let at = if layers[at].source == LayerSource::Group {
            // A group's members are directly beneath it.
            heal.group = Some(layer);
            at
        } else {
            heal.group = layers[at].group;
            at + 1
        };
        let id = heal.id;
        self.document.insert_layer(at, heal).ok()?;
        Some(id)
    }

    /// Applies a slider's new position, from 0 to 1, to the selected row.
    pub(super) fn slider_moved(&mut self, slider: SliderId, position: f32) {
        match slider {
            SliderId::ToolParam(index) => self.tool_slider_moved(index, position),
            SliderId::ViewStretchParam(index) => {
                let kind = self.view_stretch["type"].as_str().unwrap_or_default();
                let Some((key, _, low, high)) = stretch_settings(kind).get(index) else {
                    return;
                };
                self.view_stretch[*key] = (low + (high - low) * f64::from(position)).into();
            }
            SliderId::Headroom => {
                let (_, _, low, high) = tone_step::HEADROOM;
                self.document.headroom = Some(low + (high - low) * position);
            }
            SliderId::HdrKnee => {
                let (_, _, low, high) = tone_step::HDR_KNEE;
                self.document.hdr_knee = Some(low + (high - low) * position);
            }
            SliderId::CoreAmount | SliderId::CoreThreshold | SliderId::CoreScale => {
                let Some(id) = self.selected_step() else {
                    return;
                };
                let Some((
                    _,
                    StepRecord {
                        kind: StepKind::Operation { settings, .. },
                        ..
                    },
                )) = self.document.step(id)
                else {
                    return;
                };
                let mut settings = settings.clone();
                let mut core: seiza_stretch::CoreCompression = settings
                    .get(CORE_COMPRESSION)
                    .and_then(|value| serde_json::from_value(value.clone()).ok())
                    .unwrap_or_default();
                let position = f64::from(position);
                match slider {
                    SliderId::CoreAmount => core.amount = (position * 100.0).round() / 100.0,
                    // From 5 to 1000 sigmas, evenly in their log.
                    SliderId::CoreThreshold => {
                        core.threshold = (CORE_THRESHOLD.0
                            * (CORE_THRESHOLD.1 / CORE_THRESHOLD.0).powf(position))
                        .round();
                    }
                    // From 1 to 256 pixels, evenly in their log.
                    _ => core.scale = 256f64.powf(position).round().clamp(1.0, 256.0),
                }
                settings[CORE_COMPRESSION] = serde_json::to_value(core).unwrap_or_default();
                self.document.set_step_settings(id, settings);
            }
            SliderId::SkyFloor => {
                let Some(id) = self.selected_step() else {
                    return;
                };
                let Some((
                    _,
                    StepRecord {
                        kind: StepKind::Operation { settings, .. },
                        ..
                    },
                )) = self.document.step(id)
                else {
                    return;
                };
                let mut settings = settings.clone();
                // Two decimals, as the layered stretches' own.
                let floor = (MAX_SKY_FLOOR * f64::from(position) * 100.0).round() / 100.0;
                settings[SKY_FLOOR] = floor.into();
                self.document.set_step_settings(id, settings);
            }
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
            SliderId::BalanceParam(index) => {
                let Some(id) = self.selected_step() else {
                    return;
                };
                let Some((_, step)) = self.document.step(id) else {
                    return;
                };
                let StepKind::Operation { settings, .. } = &step.kind else {
                    return;
                };
                let Some((key, _, low, high)) = BALANCE_CONTROLS.get(index) else {
                    return;
                };
                let mut settings = settings.clone();
                if !settings["balance"].is_object() {
                    return;
                }
                let value = low + (high - low) * f64::from(position);
                settings["balance"][*key] = ((value * 100.0).round() / 100.0).into();
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
                let Some(StepRecord {
                    id,
                    kind:
                        StepKind::Operation {
                            operation,
                            settings,
                            ..
                        },
                    ..
                }) = layer.source.adjustment_step()
                else {
                    return;
                };
                let id = *id;
                let Some((low, high)) = amount_range(operation) else {
                    return;
                };
                let changed = with_amount(operation, settings, low + (high - low) * position);
                self.document.set_step_settings(id, changed);
            }
            SliderId::RangeLow | SliderId::RangeHigh | SliderId::RangeFeather => {
                if let Some(Some(MaskRecord::Range { low, high, feather })) = self.range_mask_mut()
                {
                    match slider {
                        SliderId::RangeLow => *low = position,
                        SliderId::RangeHigh => {
                            *high = if position >= RANGE_TOP_POSITION {
                                RANGE_BRIGHTEST
                            } else {
                                position
                            };
                        }
                        _ => *feather = position * RANGE_MAX_FEATHER,
                    }
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
            SliderId::StarReduction => {
                if let Row::Layer(id) = &self.selected
                    && let Some(layer) = self.document.layer_mut(*id)
                {
                    layer.star_reduction = position;
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
        let bounds = self
            .slider_bounds
            .borrow_mut()
            .entry(id)
            .or_default()
            .clone();
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
                        LayerSource::Adjust { .. } | LayerSource::Adjustment { .. } => {
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
            (Tool::Protect, _) => {
                Some("Protect draws a region the background extraction keeps out.".to_string())
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
    /// Undo or Redo in the top bar, disabled while there is nothing to
    /// undo or redo.
    fn history_button(
        label: &'static str,
        id: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
        action: fn(&mut Self),
    ) -> impl IntoElement {
        div()
            .id(id)
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(CONTROL))
            .child(label)
            .when(!enabled, |button| button.text_color(rgb(QUIET)))
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        action(this);
                        cx.notify();
                    }))
            })
    }

    /// Starts renaming the picture: the name in the top bar becomes a field
    /// holding the current name.
    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self
            .rename_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        // A picture named from a hash starts with an empty field.
        self.renaming = Some(
            if crate::studio_model::is_generated_name(&self.document.name) {
                String::new()
            } else {
                self.picture_name()
            },
        );
        focus.focus(window, cx);
    }

    /// Renames the picture, as the person typed it. An empty name keeps
    /// the old one. The rename is an edit: it can be undone, and it is
    /// saved, to the server too.
    pub(super) fn rename_picture(&mut self, name: &str) -> Result<(), String> {
        let name: String = name
            .chars()
            .filter(|letter| !letter.is_control())
            .collect::<String>()
            .trim()
            .chars()
            .take(MAX_PICTURE_NAME)
            .collect();
        if name.is_empty() {
            return Err("A picture needs a name".to_string());
        }
        if name != self.document.name {
            self.document.name = name;
            self.note_edit();
            self.save_document();
        }
        Ok(())
    }

    /// The picture's name, which a click turns into a field to rename it.
    fn name_field(&self, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(typed) = self.renaming.clone() else {
            return div()
                .id("picture-name")
                .px_2()
                .py_1()
                .rounded_sm()
                .cursor_text()
                .hover(|name| name.bg(rgb(CONTROL)))
                .child(self.picture_name())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.start_rename(window, cx);
                    cx.notify();
                }))
                .into_any_element();
        };
        let focus = self
            .rename_focus
            .clone()
            .unwrap_or_else(|| cx.focus_handle());
        let focused = focus.is_focused(window);
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("picture-name-field")
                    .track_focus(&focus)
                    .min_w(px(220.0))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(CONTROL))
                    .border_1()
                    .border_color(rgb(SELECTED))
                    .cursor_text()
                    .child(format!("{typed}{}", if focused { "▏" } else { "" }))
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        let keystroke = &event.keystroke;
                        let Some(field) = this.renaming.as_mut() else {
                            return;
                        };
                        if keystroke.modifiers.platform && keystroke.key == "v" {
                            if let Some(text) =
                                cx.read_from_clipboard().and_then(|item| item.text())
                            {
                                field.push_str(text.trim());
                            }
                        } else if keystroke.key == "backspace" {
                            if keystroke.modifiers.alt || keystroke.modifiers.platform {
                                field.clear();
                            } else {
                                field.pop();
                            }
                        } else if keystroke.key == "enter" {
                            let typed = field.clone();
                            match this.rename_picture(&typed) {
                                Ok(()) => this.renaming = None,
                                Err(reason) => this.say(&reason),
                            }
                        } else if keystroke.key == "escape" {
                            this.renaming = None;
                        } else if !keystroke.modifiers.platform
                            && !keystroke.modifiers.control
                            && let Some(letters) = &keystroke.key_char
                        {
                            field.push_str(letters);
                        } else {
                            return;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    })),
            )
            .child(Self::note(
                "Press Return to rename, or Esc to cancel".to_string(),
            ))
            .into_any_element()
    }

    fn top_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(self.name_field(window, cx))
            .when_some(self.server_save_line(), |bar, line| {
                bar.child(div().max_w(px(360.0)).child(Self::note(line)))
            })
            .when(self.server_save.conflict(), |bar| {
                bar.child(Self::act(
                    "Save as a copy",
                    "server-copy",
                    false,
                    cx,
                    |this| this.save_as_copy(),
                ))
                .child(Self::act(
                    "Open the newer version",
                    "server-newer",
                    false,
                    cx,
                    |this| this.open_newer_version(),
                ))
            })
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
            .child(
                div()
                    .id("open-server")
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(if self.servers.open { SELECTED } else { CONTROL }))
                    .cursor_pointer()
                    .child("PSF Guard")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_server_panel(window, cx);
                        cx.notify();
                    })),
            )
            .child(Self::history_button(
                "Undo",
                "undo",
                self.history_can(false),
                cx,
                Self::undo,
            ))
            .child(Self::history_button(
                "Redo",
                "redo",
                self.history_can(true),
                cx,
                Self::redo,
            ))
            .child(div().pl_3().child(Self::note("View")))
            .child(Self::segmented().children(splits))
            .child(div().pl_2().child(Self::act(
                "View stretch",
                "view-stretch",
                self.view_stretch_open,
                cx,
                |this| this.view_stretch_open = !this.view_stretch_open,
            )))
            // HDR belongs to the whole picture, so its switch and headroom
            // sit with the view, wherever the work is.
            .child(div().pl_3().child(Self::note("Picture")))
            .child(div().pl_2().child(Self::act(
                "HDR",
                "toolbar-hdr",
                self.document.headroom.is_some(),
                cx,
                |this| {
                    let on = this.document.headroom.is_none();
                    this.set_hdr(on);
                },
            )))
            .when_some(self.document.headroom, |bar, headroom| {
                bar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .pl_1()
                        .child(Self::act("−", "headroom-less", false, cx, |this| {
                            this.step_headroom(-0.5)
                        }))
                        .child(Self::note(format!("{headroom:.1} stops")))
                        .child(Self::act("+", "headroom-more", false, cx, |this| {
                            this.step_headroom(0.5)
                        })),
                )
            })
            .child(div().pl_2().child(Self::act(
                "Clipping",
                "show-clipping",
                self.canvas.read(|shared| shared.canvas.show_clipping),
                cx,
                |this| {
                    this.canvas
                        .edit(|canvas| canvas.show_clipping = !canvas.show_clipping)
                },
            )))
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
            // Steps changed while the tools run: one more run can follow.
            .when(self.is_running() && self.run_is_queued(), |bar| {
                bar.child(
                    div()
                        .pr_2()
                        .text_xs()
                        .whitespace_nowrap()
                        .child("Runs again when this run ends"),
                )
            })
            .when(
                self.is_running() && !self.run_is_queued() && self.changed_during_run(),
                |bar| {
                    bar.child(
                        div()
                            .pr_2()
                            .text_xs()
                            .whitespace_nowrap()
                            .child("Changed since this run started"),
                    )
                    .child(Self::act(
                        "Run again after",
                        "run-again-bar",
                        false,
                        cx,
                        |this| this.run_tools(),
                    ))
                },
            )
            // Tools changed since the store was prepared: the picture shows
            // the store's tools until they run.
            .when_some(
                (!self.is_running() && self.cannot_run().is_none())
                    .then(|| {
                        let mut names: Vec<String> = self
                            .inputs_to_run()
                            .iter()
                            .map(|input| channel_label(input))
                            .collect();
                        names.extend(self.late_to_run().into_iter().map(|step| step.words));
                        names
                    })
                    .filter(|names| !names.is_empty()),
                |bar, names| {
                    bar.child(
                        div()
                            .pr_2()
                            .text_xs()
                            .whitespace_nowrap()
                            .child(format!("Tools to run: {}", names.join(", "))),
                    )
                    .child(Self::act(
                        "Run tools",
                        "run-tools-bar",
                        true,
                        cx,
                        |this| this.run_tools(),
                    ))
                },
            )
            .child(Self::act("?", "help", Self::help_shown(), cx, |_| {
                Self::toggle_help()
            }))
    }

    /// One label per pane, drawn in the pane's top left corner: what the
    /// pane shows, what it leaves out, and a button that keeps it there
    /// while the selection moves.
    fn pane_labels(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let scale = self.scale_factor.max(1.0);
        type Drawn = ([u32; 4], Vec<String>, Option<Place>);
        let drawn: Vec<Drawn> = self.canvas.read(|shared| {
            let canvas = &shared.canvas;
            (0..canvas.scenes.len())
                .map(|index| {
                    let scene = &canvas.scenes[index];
                    (
                        canvas.pane_rects(index).1,
                        scene.omitted.clone(),
                        scene.place.clone(),
                    )
                })
                .collect()
        });
        self.places
            .iter()
            .zip(drawn)
            .enumerate()
            .map(|(index, (wanted, (rect, omitted, drawn)))| {
                let pinned = self.pins.get(index).is_some_and(Option::is_some);
                let place = wanted.clone();
                let (shown, coming) = pane_caption(wanted, drawn);
                let coming = coming.map(|next| format!("Drawing {}…", next.label(&self.document)));
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
                            .child(shown.label(&self.document))
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
                    .children(coming.map(Self::note))
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

        let server_panel = self.server_panel(window, cx);
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(BACKGROUND))
            .text_color(rgb(TEXT))
            .text_sm()
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &super::Undo, _, cx| {
                this.undo();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &super::Redo, _, cx| {
                this.redo();
                cx.notify();
            }))
            .child(self.top_bar(window, cx))
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
                                )
                                .when(self.setup_open, |area| area.child(self.setup_view(cx))),
                        ),
                    )
                    .child(self.inspector(cx)),
            )
            .children(server_panel)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_sky_floor_takes_the_sky_to_black_and_keeps_white() {
        use seiza_stretch::{StretchAnalysis, StretchConfig};
        assert!(super::sky_floor_stage(0.0).is_none());
        let model = super::sky_floor_stage(0.1).unwrap();
        let samples: Vec<f32> = (0..1000).map(|at| at as f32 / 999.0).collect();
        let analysis = StretchAnalysis::analyze(&samples, 1, samples.len()).unwrap();
        let plan = StretchConfig {
            model,
            color_strategy: Default::default(),
            max_analysis_samples: samples.len(),
        }
        .resolve(&analysis)
        .unwrap();
        let curve = plan.curves()[0];
        assert_eq!(curve.map(0.05), 0.0);
        assert_eq!(curve.map(0.1), 0.0);
        assert!((curve.map(0.55) - 0.5).abs() < 1e-5);
        assert!((curve.map(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_groups_stars_are_reduced_together_unless_a_layer_has_its_own_amount() {
        use super::{flow_document, star_reduction};
        use crate::studio_model::{LayerRecord, LayerSource};
        let mut document = flow_document(&["red".into(), "green".into()]);
        let mut group = LayerRecord::new("RGB stars", LayerSource::Group);
        let band = |name: &str, band: &str| {
            let mut layer = LayerRecord::new(
                name,
                LayerSource::Band {
                    band: band.to_string(),
                },
            );
            layer.group = Some(group.id);
            layer
        };
        let red = band("R stars", "red@stars");
        let green = band("G stars", "green@stars");
        let nebula = band("Red", "red");
        group.star_reduction = 0.75;
        // The group goes in first: a layer whose group is not in the stack
        // is a member of none.
        for (at, layer) in [group, nebula.clone(), red.clone(), green.clone()]
            .into_iter()
            .enumerate()
        {
            document.insert_layer(at, layer).expect("a layer");
        }
        let stars = vec!["red@stars".to_string(), "green@stars".to_string()];
        // The group's amount, with every stars input of the group.
        assert_eq!(star_reduction(&document, &red), Some((0.75, stars.clone())));
        assert_eq!(star_reduction(&document, &green), Some((0.75, stars)));
        // Only stars a tool took out of an input are reduced.
        assert_eq!(star_reduction(&document, &nebula), None);
        // A layer's own amount is used alone, with its own input.
        let own = LayerRecord {
            star_reduction: 0.25,
            ..red
        };
        assert_eq!(
            star_reduction(&document, &own),
            Some((0.25, vec!["red@stars".to_string()]))
        );
    }

    #[test]
    fn range_masks_are_drawn_on_layers_and_on_picture_steps() {
        use super::{flow_document, scene, Bands};
        use crate::studio_model::{LayerSource, MaskRecord, Place, StepKind, StepRecord};
        let inputs = ["ha".to_string(), "oiii".to_string()];
        let mut document = flow_document(&inputs);
        let adjustments: Vec<_> = document
            .layers()
            .iter()
            .filter(|layer| layer.source.is_adjustment())
            .map(|layer| layer.id)
            .collect();
        for id in adjustments {
            document.remove_layer(id);
        }
        let range = MaskRecord::Range {
            low: 0.2,
            high: 0.7,
            feather: 0.05,
        };
        let band = document
            .layers()
            .iter()
            .find(|layer| matches!(layer.source, LayerSource::Band { .. }))
            .map(|layer| layer.id)
            .unwrap();
        document.layer_mut(band).unwrap().mask = Some(range.clone());
        let tone = StepRecord::new(StepKind::Operation {
            operation: crate::tone_step::OPERATION.to_string(),
            accepts: crate::image_steps::Accepts::Display,
            settings: crate::tone_step::new_settings(),
        });
        let tone_layer = document.add_picture_step(tone);
        document.layer_mut(tone_layer).unwrap().mask = Some(range);
        let mut bands = Bands::default();
        bands.names = inputs.to_vec();
        bands.tile_band = vec![0, 1];
        bands.steps = vec![Vec::new(), Vec::new()];
        let (drawn, _) = scene(
            &document,
            &bands,
            &Place::Final.view(),
            0,
            None,
            &mut crate::output_stretch::ResolvedChains::default(),
            &[],
        );
        let expected = seiza_compose::picture::RangeMask {
            low: 0.2,
            high: 0.7,
            feather: 0.05,
        };
        assert!(
            drawn.omitted.iter().all(|words| !words.contains("range")),
            "{:?}",
            drawn.omitted
        );
        assert!(drawn
            .layers
            .iter()
            .any(|layer| layer.range == Some(expected)));
        assert_eq!(drawn.finish.len(), 1);
        assert_eq!(drawn.finish[0].range, Some(expected));
    }

    #[test]
    fn a_document_past_one_gpu_draw_leaves_nothing_out() {
        use super::{flow_document, scene, Bands};
        use crate::canvas::Mark;
        use crate::studio_model::{
            MaskRecord, PaintedMask, Place, StepKind, StepRecord, Stroke, StrokeMode,
        };
        use seiza_compose::gpu::{MAX_FINISH_STEPS, MAX_HEALS, MAX_LAYERS, MAX_MASKS};
        let inputs = ["ha".to_string(), "oiii".to_string()];
        let mut document = flow_document(&inputs);
        let mut bands = Bands::default();
        bands.names = inputs.to_vec();
        bands.tile_band = vec![0, 1];
        bands.steps = vec![Vec::new(), Vec::new()];
        let draw = |document: &crate::studio_model::Document| {
            let mut drawn = scene(
                document,
                &bands,
                &Place::Final.view(),
                0,
                None,
                &mut crate::output_stretch::ResolvedChains::default(),
                &[],
            )
            .0;
            drawn.omitted.sort();
            drawn
        };
        let stroke = |at: f32| Stroke {
            points: vec![[at, at]],
            radius: 4.0,
            hardness: 0.5,
            mode: StrokeMode::Add,
        };
        // What the document's own steps leave out with these test bands.
        let before = draw(&document).omitted;
        // More Tone steps, each with a painted mask, and heals than one
        // draw holds.
        let steps = MAX_LAYERS + MAX_FINISH_STEPS;
        for at in 0..steps {
            let tone = StepRecord::new(StepKind::Operation {
                operation: crate::tone_step::OPERATION.to_string(),
                accepts: crate::image_steps::Accepts::Display,
                settings: crate::tone_step::new_settings(),
            });
            let layer = document.add_picture_step(tone);
            document.layer_mut(layer).unwrap().mask = Some(MaskRecord::Painted(PaintedMask {
                strokes: vec![stroke(at as f32)],
                ..Default::default()
            }));
        }
        for at in 0..=MAX_HEALS {
            document.add_picture_step(StepRecord::new(StepKind::Heal {
                stroke: stroke(at as f32 + 40.0),
                offset: [8, 0],
                shift: vec![0.0; 3],
            }));
        }
        let on_top = draw(&document);
        assert_eq!(on_top.omitted, before);
        assert!(on_top.finish.len() > MAX_FINISH_STEPS + MAX_HEALS);
        assert!(on_top.masks.len() > MAX_MASKS);
        // With an input on top of them, the steps are layers of the stack.
        let input = document
            .layers()
            .iter()
            .find(|layer| layer.source.shows_pixels())
            .unwrap()
            .id;
        document.place_layer(input, document.layers().len() - 1, None);
        let beneath = draw(&document);
        assert_eq!(beneath.omitted, before);
        let marked = beneath
            .layers
            .iter()
            .filter(|layer| matches!(layer.mark, Mark::Step(_)))
            .count();
        assert!(marked > MAX_LAYERS, "{marked} steps in the stack");
    }

    #[test]
    fn a_picture_step_on_top_is_a_finish_step_and_beneath_a_layer_one() {
        use super::{flow_document, scene, Bands};
        use crate::canvas::Mark;
        use crate::studio_model::{Place, StepKind, StepRecord, Stroke, StrokeMode};
        use seiza_compose::picture::FinishKind;
        let inputs = ["ha".to_string(), "oiii".to_string()];
        let mut document = flow_document(&inputs);
        // Only the input layers, with nothing on the picture.
        let adjustments: Vec<_> = document
            .layers()
            .iter()
            .filter(|layer| layer.source.is_adjustment())
            .map(|layer| layer.id)
            .collect();
        for id in adjustments {
            document.remove_layer(id);
        }
        let mut bands = Bands::default();
        bands.names = inputs.to_vec();
        bands.tile_band = vec![0, 1];
        bands.steps = vec![Vec::new(), Vec::new()];
        let draw = |document: &crate::studio_model::Document| {
            scene(
                document,
                &bands,
                &Place::Final.view(),
                0,
                None,
                &mut crate::output_stretch::ResolvedChains::default(),
                &[],
            )
            .0
        };
        let tone = StepRecord::new(StepKind::Operation {
            operation: crate::tone_step::OPERATION.to_string(),
            accepts: crate::image_steps::Accepts::Display,
            settings: crate::tone_step::new_settings(),
        });
        let tone_layer = document.add_picture_step(tone);
        let on_top = draw(&document);
        assert!(
            matches!(on_top.finish[..], [ref step] if matches!(step.kind, FinishKind::Tone(_)))
        );
        assert!(!on_top
            .layers
            .iter()
            .any(|layer| matches!(layer.mark, Mark::Step(_))));

        // Under a Curves layer, the Tone changes only the layers beneath it.
        let curves = document.add_picture_step(super::operation(
            "Curves",
            crate::image_steps::Accepts::Display,
        ));
        let beneath = draw(&document);
        assert!(beneath.finish.is_empty());
        let marked: Vec<_> = beneath
            .layers
            .iter()
            .filter_map(|layer| match layer.mark {
                Mark::Step(index) => Some(&beneath.steps[index]),
                _ => None,
            })
            .collect();
        assert!(matches!(marked[..], [FinishKind::Tone(_)]));

        // A heal on top is a Finish step; beneath another layer it is a step
        // layer, which Seiza draws in stages.
        let heal = StepRecord::new(StepKind::Heal {
            stroke: Stroke {
                points: vec![[10.0, 10.0]],
                radius: 4.0,
                hardness: 0.5,
                mode: StrokeMode::Add,
            },
            offset: [8, 0],
            shift: vec![0.0; 3],
        });
        let heal_layer = document.add_picture_step(heal);
        let healed = draw(&document);
        assert!(
            matches!(healed.finish[..], [ref step] if matches!(step.kind, FinishKind::Heal { .. }))
        );
        let at = document
            .layers()
            .iter()
            .position(|layer| layer.id == curves)
            .unwrap();
        document.place_layer(heal_layer, at, None);
        let under = draw(&document);
        assert!(under.finish.is_empty());
        assert!(
            !under.omitted.iter().any(|words| words.contains("heal")),
            "{:?}",
            under.omitted
        );
        assert!(under.layers.iter().any(|layer| matches!(
            layer.mark,
            Mark::Step(index) if matches!(under.steps[index], FinishKind::Heal { .. })
        )));
        // With a mask of its layer, it is limited by that mask too: Seiza
        // works out a staged step's whole cover.
        document.layer_mut(heal_layer).unwrap().mask = Some(
            crate::studio_model::MaskRecord::Painted(crate::studio_model::PaintedMask {
                strokes: vec![Stroke {
                    points: vec![[12.0, 12.0]],
                    radius: 6.0,
                    hardness: 0.5,
                    mode: StrokeMode::Add,
                }],
                ..Default::default()
            }),
        );
        let masked = draw(&document);
        assert!(masked.omitted.iter().all(|words| !words.contains("heal")));
        assert!(masked.layers.iter().any(|layer| matches!(
            layer.mark,
            Mark::Step(index) if matches!(masked.steps[index], FinishKind::Heal { .. })
        ) && layer.clip.is_some()));
        // The hidden layer's step is drawn nowhere.
        document.layer_mut(tone_layer).unwrap().visible = false;
        let hidden = draw(&document);
        assert!(!hidden
            .steps
            .iter()
            .any(|kind| matches!(kind, FinishKind::Tone(_))));
    }

    /// A tool's image that covers the stack leaves nothing behind that
    /// pointed at the layers it replaced: an own adjustment behind a shared
    /// mask on a layer beneath it gets no mask on the tool's image.
    #[test]
    fn a_tool_image_over_the_stack_drops_the_shared_masks_beneath_it() {
        use super::{flow_document, operation, scene, Bands};
        use crate::derived::{DerivedKey, DerivedKind, DerivedPlane};
        use crate::studio_model::{tool_step, LayerSource, MaskRecord, Place, SharedMask};
        let inputs = ["ha".to_string(), "oiii".to_string()];
        let mut document = flow_document(&inputs);
        let mask = document.add_mask(SharedMask::new(vec!["ha".to_string()]));
        // The last band layer, so its entry sits past the tool image's three.
        let band = document
            .layers()
            .iter()
            .rev()
            .find(|layer| matches!(layer.source, LayerSource::Band { .. }))
            .map(|layer| layer.id)
            .unwrap();
        let mut curves = operation("Curves", crate::image_steps::Accepts::Display);
        curves.mask = Some(MaskRecord::Shared {
            mask,
            inverted: false,
        });
        document.insert_layer_step(band, curves).unwrap();
        let removal = tool_step(serde_json::json!({ "provider": "sxt" }));
        let removal_id = removal.id;
        document.add_picture_step(removal);
        let mut bands = Bands::default();
        bands.names = inputs.to_vec();
        bands.tile_band = vec![0, 1];
        bands.steps = vec![Vec::new(), Vec::new()];
        let planes: Vec<std::sync::Arc<DerivedPlane>> = [false, true]
            .into_iter()
            .flat_map(|stars| (0..3).map(move |channel| (stars, channel)))
            .map(|(stars, channel)| {
                std::sync::Arc::new(DerivedPlane::of_samples(
                    DerivedKey {
                        kind: DerivedKind::Tool {
                            step: removal_id,
                            channel,
                            stars,
                        },
                        bands: Vec::new(),
                        states: Vec::new(),
                        range: (0.0, 1.0),
                    },
                    16,
                    16,
                    vec![0.5; 256],
                    1,
                ))
            })
            .collect();
        let bands = bands.with_derived(&planes);
        let (drawn, _) = scene(
            &document,
            &bands,
            &Place::Final.view(),
            0,
            None,
            &mut crate::output_stretch::ResolvedChains::default(),
            &[],
        );
        assert!(
            drawn.layers.iter().all(|layer| layer.computed.is_none()),
            "a layer of the tool's image took a shared mask from beneath it"
        );
    }

    /// Steps at the top of the stack are drawn as Finish steps, but they are
    /// still part of the combined picture an output stretch is resolved from.
    #[test]
    fn the_combined_picture_keeps_steps_moved_off_the_top_of_the_stack() {
        use super::{flow_document, scene, Bands};
        use crate::studio_model::Place;
        let inputs = ["ha".to_string(), "oiii".to_string()];
        let mut document = flow_document(&inputs);
        let adjustments: Vec<_> = document
            .layers()
            .iter()
            .filter(|layer| layer.source.is_adjustment())
            .map(|layer| layer.id)
            .collect();
        for id in adjustments {
            document.remove_layer(id);
        }
        document.add_picture_step(crate::studio_model::StepRecord::new(
            crate::studio_model::StepKind::Operation {
                operation: crate::tone_step::OPERATION.to_string(),
                accepts: crate::image_steps::Accepts::Display,
                settings: crate::tone_step::new_settings(),
            },
        ));
        let mut bands = Bands::default();
        bands.names = inputs.to_vec();
        bands.tile_band = vec![0, 1];
        bands.steps = vec![Vec::new(), Vec::new()];
        let (drawn, _) = scene(
            &document,
            &bands,
            &Place::Final.view(),
            0,
            None,
            &mut crate::output_stretch::ResolvedChains::default(),
            &[],
        );
        assert!(
            !drawn.finish.is_empty(),
            "the Tone step is drawn as a Finish step"
        );
        assert_eq!(drawn.combined(&[]).finish.len(), drawn.finish.len());
    }

    #[test]
    fn a_stars_layer_heals_its_stars_inside_its_group() {
        use super::{flow_document, scene, Bands};
        use crate::canvas::Mark;
        use crate::derived::{DerivedKey, DerivedKind, DerivedPlane};
        use crate::studio_model::{
            stars_layer, tool_step, Place, StepKind, StepRecord, Stroke, StrokeMode,
        };
        use seiza_compose::picture::FinishKind;
        let inputs = ["ha".to_string(), "oiii".to_string()];
        let mut document = flow_document(&inputs);
        let removal = tool_step(serde_json::json!({ "provider": "sxt" }));
        let removal_id = removal.id;
        document.add_picture_step(removal);
        let stars = document.layers().last().unwrap().id;
        let heal = StepRecord::new(StepKind::Heal {
            stroke: Stroke {
                points: vec![[10.0, 10.0]],
                radius: 4.0,
                hardness: 0.6,
                mode: StrokeMode::Add,
            },
            offset: [12, 0],
            shift: Vec::new(),
        });
        document.insert_layer_step(stars, heal).unwrap();
        assert_eq!(
            document.layers().last().unwrap().name,
            stars_layer(removal_id).name
        );
        let mut bands = Bands::default();
        bands.names = inputs.to_vec();
        bands.tile_band = vec![0, 1];
        bands.steps = vec![Vec::new(), Vec::new()];
        // The tool's result and its stars, as a run leaves them.
        let planes: Vec<std::sync::Arc<DerivedPlane>> = [false, true]
            .into_iter()
            .flat_map(|stars| (0..3).map(move |channel| (stars, channel)))
            .map(|(stars, channel)| {
                std::sync::Arc::new(DerivedPlane::of_samples(
                    DerivedKey {
                        kind: DerivedKind::Tool {
                            step: removal_id,
                            channel,
                            stars,
                        },
                        bands: Vec::new(),
                        states: Vec::new(),
                        range: (0.0, 1.0),
                    },
                    16,
                    16,
                    vec![0.5; 256],
                    1,
                ))
            })
            .collect();
        let bands = bands.with_derived(&planes);
        let (drawn, _) = scene(
            &document,
            &bands,
            &Place::Final.view(),
            0,
            None,
            &mut crate::output_stretch::ResolvedChains::default(),
            &[],
        );
        let marks: Vec<Mark> = drawn.layers.iter().map(|layer| layer.mark).collect();
        let heal_at = marks
            .iter()
            .position(|mark| matches!(mark, Mark::Step(index) if matches!(drawn.steps[*index], FinishKind::Heal { .. })))
            .unwrap_or_else(|| panic!("no heal among {marks:?}; left out {:?}", drawn.omitted));
        // Between the stars group's start and its end.
        assert!(matches!(
            marks[..heal_at]
                .iter()
                .rev()
                .find(|mark| !matches!(mark, Mark::Layer)),
            Some(Mark::GroupBegin)
        ));
        assert!(matches!(
            marks[heal_at + 1..]
                .iter()
                .find(|mark| !matches!(mark, Mark::Layer)),
            Some(Mark::GroupEnd)
        ));
    }

    #[test]
    fn a_stretch_beneath_other_layers_is_resolved_from_what_is_beneath_it() {
        use super::{beneath_other_layers, flow_document, new_stretch, scene, Bands};
        use crate::canvas::Mark;
        use crate::studio_model::{Place, StepKind, StepRecord};
        use seiza_compose::picture::FinishKind;
        let inputs = ["ha".to_string(), "oiii".to_string()];
        let mut document = flow_document(&inputs);
        let adjustments: Vec<_> = document
            .layers()
            .iter()
            .filter(|layer| layer.source.is_adjustment())
            .map(|layer| layer.id)
            .collect();
        for id in adjustments {
            document.remove_layer(id);
        }
        let stretch = StepRecord::new(StepKind::Operation {
            operation: crate::studio_model::flow_operations::OUTPUT_STRETCH.to_string(),
            accepts: crate::image_steps::Accepts::Display,
            settings: serde_json::json!({ "model": new_stretch("auto-mtf"), "color_strategy": "linked" }),
        });
        let stretch_id = stretch.id;
        document.add_picture_step(stretch);
        // On top, it is resolved from samples of the picture.
        assert!(beneath_other_layers(&document).is_empty());
        // With a layer above it, from samples of what is beneath it.
        let top = document.layers()[0].clone();
        let mut above = top.clone();
        above.id = uuid::Uuid::new_v4();
        let count = document.layers().len();
        document.insert_layer(count, above).unwrap();
        assert!(beneath_other_layers(&document).contains(&stretch_id));
        let mut bands = Bands::default();
        bands.names = inputs.to_vec();
        bands.tile_band = vec![0, 1];
        bands.steps = vec![Vec::new(), Vec::new()];
        let draw = |chains: &mut crate::output_stretch::ResolvedChains| {
            scene(
                &document,
                &bands,
                &Place::Final.view(),
                0,
                None,
                chains,
                &[],
            )
            .0
        };
        let waiting = draw(&mut Default::default());
        assert!(waiting
            .omitted
            .iter()
            .any(|words| words.contains("until the layers beneath it have been sampled")));
        let mut chains = crate::output_stretch::ResolvedChains::default();
        let samples: Vec<f32> = (0..3000).map(|at| (at % 97) as f32 / 400.0).collect();
        chains
            .beneath
            .insert(stretch_id, (7, std::sync::Arc::new(samples)));
        let drawn = draw(&mut chains);
        assert!(
            drawn.layers.iter().any(|entry| matches!(
                entry.mark,
                Mark::Step(index) if matches!(drawn.steps[index], FinishKind::Stretch(_))
            )),
            "{:?}",
            drawn.omitted
        );
    }

    #[test]
    fn groups_pass_through_clip_and_keep_painted_masks_behind_shared_ones() {
        use super::{flow_document, scene, Bands};
        use crate::canvas::Mark;
        use crate::studio_model::{MaskRecord, PaintedMask, Place, SharedMask, Stroke, StrokeMode};
        let inputs = ["ha".to_string(), "oiii".to_string()];
        let mut document = flow_document(&inputs);
        let adjustments: Vec<_> = document
            .layers()
            .iter()
            .filter(|layer| layer.source.is_adjustment())
            .map(|layer| layer.id)
            .collect();
        for id in adjustments {
            document.remove_layer(id);
        }
        let ids: Vec<_> = document.layers().iter().map(|layer| layer.id).collect();
        let mut bands = Bands::default();
        bands.names = inputs.to_vec();
        bands.tile_band = vec![0, 1];
        bands.steps = vec![Vec::new(), Vec::new()];
        let draw = |document: &crate::studio_model::Document| {
            scene(
                document,
                &bands,
                &Place::Final.view(),
                0,
                None,
                &mut Default::default(),
                &[],
            )
            .0
        };
        let marks = |drawn: &super::Scene| -> Vec<Mark> {
            drawn.layers.iter().map(|layer| layer.mark).collect()
        };
        // The top layer in a group at half opacity.
        let group = document.group_layer(ids[1]).unwrap();
        document.layer_mut(group).unwrap().opacity = 0.5;
        let grouped = draw(&document);
        assert!(marks(&grouped).contains(&Mark::GroupBegin));
        // Passing through, the group is not drawn; its layer takes its
        // opacity.
        document.layer_mut(group).unwrap().pass_through = true;
        let passed = draw(&document);
        assert!(!marks(&passed).contains(&Mark::GroupBegin));
        let top = passed.layers.last().unwrap();
        assert!((top.opacity - 0.5 * document.layers()[1].opacity).abs() < 1e-6);
        // Clipped, the group shows within the mask of the layer beneath.
        document.layer_mut(group).unwrap().pass_through = false;
        let stroke = Stroke {
            points: vec![[5.0, 5.0]],
            radius: 3.0,
            hardness: 0.5,
            mode: StrokeMode::Add,
        };
        document.layer_mut(ids[0]).unwrap().mask = Some(MaskRecord::Painted(PaintedMask {
            strokes: vec![stroke.clone()],
            ..Default::default()
        }));
        document.layer_mut(group).unwrap().clipped = true;
        let clipped = draw(&document);
        let end = clipped
            .layers
            .iter()
            .find(|layer| layer.mark == Mark::GroupEnd)
            .unwrap();
        assert!(end.clip.is_some());
        // A painted mask stays when a shared mask is chosen, and limits the
        // layer as well.
        let shared = document.add_mask(SharedMask::new(vec!["ha".to_string()]));
        assert!(document.set_layer_shared_mask(ids[0], Some((shared, false))));
        let layer = &document.layers()[0];
        assert!(matches!(layer.mask, Some(MaskRecord::Shared { .. })));
        assert!(layer.painted_too.is_some());
        let behind = draw(&document);
        assert!(behind.layers.first().unwrap().mask.is_some());
        assert!(document.set_layer_shared_mask(ids[0], None));
        assert!(matches!(
            document.layers()[0].mask,
            Some(MaskRecord::Painted(_))
        ));
    }

    #[test]
    fn a_pane_label_names_what_the_pane_draws() {
        use super::pane_caption;
        use crate::studio_model::Place;
        let stacked = Place::Input {
            channel: "oiii".into(),
            after: None,
        };
        let tools = Place::Input {
            channel: "oiii".into(),
            after: Some(uuid::Uuid::new_v4()),
        };
        // The scene for the tools step is still being built: the label
        // names the stacked view on screen, and the tools step as next.
        assert_eq!(
            pane_caption(&tools, Some(stacked.clone())),
            (stacked.clone(), Some(tools.clone()))
        );
        assert_eq!(
            pane_caption(&tools, Some(tools.clone())),
            (tools.clone(), None)
        );
        assert_eq!(pane_caption(&tools, None), (tools, None));
    }
}
