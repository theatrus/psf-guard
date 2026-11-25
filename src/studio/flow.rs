//! PSF Guard's color processing flow as a Studio document, and back.
//!
//! A new document starts as the steps PSF Guard's color previews apply. Each
//! step keeps the settings PSF Guard saved, untouched, so the document turns
//! back into the same settings and the existing pipeline gives the same
//! result from them.
//!
//! The settings are those of a Color processing setup: the JSON form of
//! `StackColorProcessing`. This module reads and writes them as JSON, so it
//! does not depend on the preview pipeline. PSF Guard's own tests hold it to
//! the typed form.

use super::{Document, LayerRecord, LayerSource, StageKind, StepKind, StepRecord, StudioError};
use crate::image_steps::Accepts;
use serde_json::{Map, Value};

/// The names of the steps a flow is made of, as a person reads them. They
/// follow the order of PSF Guard's color pipeline.
pub mod flow_operations {
    /// A channel's stack: the source of one band.
    pub const CHANNEL_STACK: &str = "Channel stack";
    /// Smooth background correction of each channel.
    pub const BACKGROUND: &str = "Background extraction";
    /// The inputs take their deconvolution and RC-Astro steps from each
    /// channel's card.
    pub const FOLLOW_CHANNELS: &str = "Inputs follow their channel cards";
    /// Deconvolution of one input channel.
    pub const DECONVOLUTION: &str = "Deconvolution";
    /// RC-Astro tools on one input channel.
    pub const RC_ASTRO_INPUT: &str = "RC-Astro tools";
    /// Continuum subtraction and emission amounts.
    pub const EMISSION: &str = "Subtract continuum";
    /// RC-Astro tools on the color channels together.
    pub const RC_ASTRO_COLOR: &str = "RC-Astro tools on color";
    /// Photometric color calibration against Gaia.
    pub const COLOR_CALIBRATION: &str = "Color calibration";
    /// One display stretch of one channel.
    pub const STRETCH: &str = "Stretch";
    /// Narrowband stretches, balance, star masks and green reduction.
    pub const NARROWBAND: &str = "Narrowband balance and stars";
    /// One display stretch of the composed image.
    pub const OUTPUT_STRETCH: &str = "Output stretch";
    /// Settings this build does not know, kept so they are not lost.
    pub const OTHER: &str = "Other settings";
}
use flow_operations as op;

/// The settings keys this module turns into steps.
const KNOWN_KEYS: [&str; 11] = [
    "input_processing",
    "background_extraction",
    "input_deconvolutions",
    "input_rc_astro",
    "emission",
    "color_rc_astro",
    "color_calibration",
    "input_stretches",
    "narrowband",
    "output_stretches",
    "__unused",
];

fn refuse(reason: impl Into<String>) -> StudioError {
    StudioError::Document(reason.into())
}

fn operation(name: &str, accepts: Accepts, settings: &Value) -> StepRecord {
    StepRecord::new(StepKind::Operation {
        operation: name.to_string(),
        accepts,
        settings: settings.clone(),
    })
}

/// The stretches a channel that the setup did not save gets: the treatment
/// every saved channel shares, when they share one.
fn shared_stretches(stretches: &Map<String, Value>) -> Option<&Value> {
    let mut values = stretches.values();
    let first = values.next()?;
    values.all(|other| other == first).then_some(first)
}

/// Builds a document from a Color processing setup's settings, for the
/// channels of one color card.
///
/// `channels` are the card's channels by PSF Guard's role names, such as
/// `luminance` or `ha`, in the order the layers stack, bottom first.
pub fn document_from_color_setup(
    name: &str,
    settings: &Value,
    channels: &[&str],
) -> Result<Document, StudioError> {
    let settings = settings
        .as_object()
        .ok_or_else(|| refuse("a processing setup's settings are a JSON object"))?;
    let mut document = Document::new(name);
    let mut add = |stage: StageKind, step: StepRecord| {
        document
            .insert_step(stage, usize::MAX, step)
            .map_err(|error| refuse(format!("a setup step does not fit its stage: {error}")))
    };
    let per_channel = |key: &str| -> Result<Vec<(&String, &Value)>, StudioError> {
        match settings.get(key) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Object(map)) => {
                // Sorted by channel, so the order of the steps does not
                // follow the order the settings happened to be written in.
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                Ok(entries)
            }
            Some(_) => Err(refuse(format!("{key} is a map from channel to settings"))),
        }
    };

    for channel in channels {
        add(
            StageKind::Sources,
            operation(op::CHANNEL_STACK, Accepts::Linear, &Value::Null).for_band(channel),
        )?;
    }

    // The Linear stage, in the order the pipeline's phases run.
    if let Some(background) = settings
        .get("background_extraction")
        .filter(|v| !v.is_null())
    {
        add(
            StageKind::Linear,
            operation(op::BACKGROUND, Accepts::Linear, background),
        )?;
    }
    if let Some(mode) = settings.get("input_processing").filter(|v| !v.is_null()) {
        add(
            StageKind::Linear,
            operation(op::FOLLOW_CHANNELS, Accepts::Linear, mode),
        )?;
    }
    for (channel, config) in per_channel("input_deconvolutions")? {
        add(
            StageKind::Linear,
            operation(op::DECONVOLUTION, Accepts::Linear, config).for_band(channel),
        )?;
    }
    for (channel, config) in per_channel("input_rc_astro")? {
        add(
            StageKind::Linear,
            operation(op::RC_ASTRO_INPUT, Accepts::Linear, config).for_band(channel),
        )?;
    }
    for (key, name) in [
        ("emission", op::EMISSION),
        ("color_rc_astro", op::RC_ASTRO_COLOR),
        ("color_calibration", op::COLOR_CALIBRATION),
    ] {
        if let Some(value) = settings.get(key).filter(|v| !v.is_null()) {
            add(StageKind::Linear, operation(name, Accepts::Linear, value))?;
        }
    }

    // The Stretch stage: each channel's stretches in order, then the
    // narrowband balance, which works on the stretched channels.
    let stretches = match settings.get("input_stretches") {
        None | Some(Value::Null) => None,
        Some(Value::Object(map)) => Some(map),
        Some(_) => return Err(refuse("input_stretches is a map from channel to stretches")),
    };
    if let Some(stretches) = stretches {
        let shared = shared_stretches(stretches);
        // A saved channel that this card lacks keeps its stretches, so the
        // setup is not changed by being opened on another card.
        let mut saved: Vec<&str> = stretches
            .keys()
            .map(String::as_str)
            .filter(|channel| !channels.contains(channel))
            .collect();
        saved.sort_unstable();
        let listed: Vec<&str> = channels.iter().copied().chain(saved).collect();
        for channel in listed {
            let Some(list) = stretches.get(channel).or(shared) else {
                continue;
            };
            let list = list
                .as_array()
                .ok_or_else(|| refuse(format!("the stretches of {channel} are a list")))?;
            for (index, stretch) in list.iter().enumerate() {
                // The first stretch takes linear samples. A later one takes
                // what the one before it gave.
                let accepts = if index == 0 {
                    Accepts::Linear
                } else {
                    Accepts::Either
                };
                add(
                    StageKind::Stretch,
                    operation(op::STRETCH, accepts, stretch).for_band(channel),
                )?;
            }
        }
    }
    if let Some(narrowband) = settings.get("narrowband").filter(|v| !v.is_null()) {
        add(
            StageKind::Stretch,
            operation(op::NARROWBAND, Accepts::Either, narrowband),
        )?;
    }

    // The Finish stage: stretches of the composed image.
    match settings.get("output_stretches") {
        None | Some(Value::Null) => {}
        Some(Value::Array(list)) => {
            for stretch in list {
                add(
                    StageKind::Finish,
                    operation(op::OUTPUT_STRETCH, Accepts::Display, stretch),
                )?;
            }
        }
        Some(_) => return Err(refuse("output_stretches is a list")),
    }

    // Settings from a newer PSF Guard are kept, so opening and saving a
    // document here does not drop them.
    let other: Map<String, Value> = settings
        .iter()
        .filter(|(key, _)| !KNOWN_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if !other.is_empty() {
        add(
            StageKind::Export,
            operation(op::OTHER, Accepts::Either, &Value::Object(other)),
        )?;
    }

    // Combine: one layer per channel of the card.
    for channel in channels {
        let layer = LayerRecord::new(
            channel,
            LayerSource::Band {
                band: channel.to_string(),
            },
        );
        document.insert_layer(usize::MAX, layer)?;
    }
    Ok(document)
}

/// The Color processing setup's settings that a document's steps hold.
///
/// A step that is off is left out, as PSF Guard leaves out a tool that is
/// off. Steps this module did not make, such as a heal stroke or a curve,
/// have no place in PSF Guard's settings and are left out too.
pub fn color_setup_from_document(document: &Document) -> Result<Value, StudioError> {
    let mut settings = Map::new();
    let mut per_channel: Map<String, Value> = Map::new();
    let mut stretches: Map<String, Value> = Map::new();
    let mut output_stretches = Vec::new();

    let steps = document
        .stages()
        .iter()
        .flat_map(|stage| &stage.steps)
        .filter(|step| step.enabled);
    for step in steps {
        let StepKind::Operation {
            operation,
            settings: value,
            ..
        } = &step.kind
        else {
            continue;
        };
        let channel = || {
            step.band
                .clone()
                .ok_or_else(|| refuse(format!("{operation} applies to one channel and names none")))
        };
        let mut into_channel_map = |key: &str| -> Result<(), StudioError> {
            let map = per_channel
                .entry(key.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(map) = map {
                map.insert(channel()?, value.clone());
            }
            Ok(())
        };
        match operation.as_str() {
            op::BACKGROUND => {
                settings.insert("background_extraction".to_string(), value.clone());
            }
            op::FOLLOW_CHANNELS => {
                settings.insert("input_processing".to_string(), value.clone());
            }
            op::DECONVOLUTION => into_channel_map("input_deconvolutions")?,
            op::RC_ASTRO_INPUT => into_channel_map("input_rc_astro")?,
            op::EMISSION => {
                settings.insert("emission".to_string(), value.clone());
            }
            op::RC_ASTRO_COLOR => {
                settings.insert("color_rc_astro".to_string(), value.clone());
            }
            op::COLOR_CALIBRATION => {
                settings.insert("color_calibration".to_string(), value.clone());
            }
            op::NARROWBAND => {
                settings.insert("narrowband".to_string(), value.clone());
            }
            op::STRETCH => {
                let list = stretches
                    .entry(channel()?)
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let Value::Array(list) = list {
                    list.push(value.clone());
                }
            }
            op::OUTPUT_STRETCH => output_stretches.push(value.clone()),
            op::OTHER => {
                if let Value::Object(other) = value {
                    settings.extend(other.clone());
                }
            }
            _ => {}
        }
    }

    settings.extend(per_channel);
    settings.insert("input_stretches".to_string(), Value::Object(stretches));
    settings.insert(
        "output_stretches".to_string(),
        Value::Array(output_stretches),
    );
    Ok(Value::Object(settings))
}
