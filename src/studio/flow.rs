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
    /// One processing tool that sharpens, by a provider such as RC-Astro's
    /// BlurXTerminator. Its settings are one step of PSF Guard's tool
    /// chains: `{"provider": "bxt", "parameters": {...}}`. A tool step's
    /// band is an input, or a color group ([`super::COLOR_GROUPS`]), whose
    /// registered inputs it runs on combined into one color image.
    pub const SHARPENING: &str = "Sharpening";
    /// One processing tool that reduces noise, such as RC-Astro's
    /// NoiseXTerminator or Seiza's noise model.
    pub const NOISE_REDUCTION: &str = "Noise reduction";
    /// One processing tool that removes the stars, such as RC-Astro's
    /// StarXTerminator. The stars it removes are kept as an input.
    pub const STAR_REMOVAL: &str = "Star removal";
    /// One processing tool whose provider this build does not know.
    pub const PROCESSING_TOOL: &str = "Processing tool";
    /// Every tool step's name.
    pub const TOOLS: [&str; 4] = [SHARPENING, NOISE_REDUCTION, STAR_REMOVAL, PROCESSING_TOOL];
    /// Documents written before each tool was a step of its own kept an
    /// input's tools in one step of this name, as a chain
    /// (`{"steps": [...]}`). Opening such a document splits it into one
    /// step per tool.
    pub const RC_ASTRO_INPUT: &str = "Processing tools";
    /// Continuum subtraction and emission amounts.
    pub const EMISSION: &str = "Subtract continuum";
    /// The older step of a color group's tools, or with no band of PSF
    /// Guard's color tools, as a chain. Opening a document splits it into
    /// one tool step per tool, with the group's band.
    pub const RC_ASTRO_COLOR: &str = "Processing tools on color";
    /// Photometric color calibration against Gaia.
    pub const COLOR_CALIBRATION: &str = "Color calibration";
    /// One display stretch of one channel.
    pub const STRETCH: &str = "Stretch";
    /// The stretch of the stars of red, green and blue, as one linked
    /// display layer: Seiza's star stretch, as PSF Guard's RGB stars use.
    /// Its `amount` is from 0 to 8.
    pub const STAR_STRETCH: &str = "Star stretch";
    /// Narrowband stretches, balance, star masks and green reduction.
    pub const NARROWBAND: &str = "Narrowband balance and stars";
    /// One display stretch of the composed image.
    pub const OUTPUT_STRETCH: &str = "Output stretch";
    /// Settings this build does not know, kept so they are not lost.
    pub const OTHER: &str = "Other settings";
}
use flow_operations as op;

/// The name of the channel that holds `channel` after one Linear step
/// that changes its samples, in a store prepared with that step: a tool, a
/// deconvolution, or a background extraction, which is kept corrected in
/// full. The store's record lists each channel's planes in the order the
/// steps ran.
pub fn step_channel(channel: &str, step: uuid::Uuid) -> String {
    format!("{channel}@step-{step}")
}

/// The step each provider of a processing tool makes, by the provider's
/// key. PSF Guard's providers (`remote_work::steps::PROVIDERS`) list the
/// same, and a test there holds the two together. The order is the order
/// one chain runs its tools in.
pub const TOOL_PROVIDERS: [(&str, &str); 4] = [
    ("bxt", op::SHARPENING),
    ("nxt", op::NOISE_REDUCTION),
    ("noise-model", op::NOISE_REDUCTION),
    ("sxt", op::STAR_REMOVAL),
];

/// The name of the step that runs a provider's tool.
pub fn tool_operation(provider: &str) -> &'static str {
    TOOL_PROVIDERS
        .iter()
        .find(|(key, _)| *key == provider)
        .map_or(op::PROCESSING_TOOL, |(_, operation)| operation)
}

/// The key of a tool step's setting that runs the tool a second time on the
/// input shifted by half a tile and averages the two, against the seams
/// BlurXTerminator leaves between its tiles. Studio's own: PSF Guard's
/// settings read past it.
pub const SHIFTED_PASS_KEY: &str = "shifted_pass";

/// Whether a step runs one processing tool.
pub fn is_tool_step(step: &StepRecord) -> bool {
    op::TOOLS.contains(&step.kind.name())
}

/// The tool a tool step runs, as one step of PSF Guard's tool chains:
/// `{"provider": ..., "parameters": {...}}`.
pub fn tool_of(step: &StepRecord) -> Option<&Value> {
    match &step.kind {
        StepKind::Operation { settings, .. } if is_tool_step(step) => {
            settings.get("provider").is_some().then_some(settings)
        }
        _ => None,
    }
}

/// A step that runs `tool`, one step of PSF Guard's tool chains. A tool
/// with no settings gets an empty set, so every tool step reads alike.
pub fn tool_step(mut tool: Value) -> StepRecord {
    // Chains written by older builds name the provider `tool`.
    if tool.get("provider").is_none()
        && let Some(provider) = tool.get("tool").cloned()
    {
        tool["provider"] = provider;
    }
    if let Some(tool) = tool.as_object_mut() {
        tool.remove("tool");
        tool.entry("parameters")
            .or_insert_with(|| Value::Object(Map::new()));
    }
    let provider = tool["provider"].as_str().unwrap_or_default().to_string();
    operation(tool_operation(&provider), Accepts::Linear, &tool)
}

/// Whether a Linear step changes the samples of `channel` when a store is
/// prepared: a tool step or a deconvolution of the input or of its color
/// group, or a Background extraction step for every input or for this
/// one. The stars a tool took out, and the other states a store keeps of
/// an input, are changed by none.
pub fn changes_input(step: &StepRecord, channel: &str) -> bool {
    if channel.contains('@') {
        return false;
    }
    let band = step.band.as_deref();
    if step.kind.name() == op::BACKGROUND {
        return band.is_none_or(|band| band == channel);
    }
    if !is_tool_step(step) && step.kind.name() != op::DECONVOLUTION {
        return false;
    }
    band == Some(channel)
        || band.is_some_and(|band| color_group_of(channel).is_some_and(|(group, _)| group == band))
}

/// The Linear steps that preparing a store runs for `channel`, in order:
/// each that is on and [`changes_input`]. Each is listed with its id,
/// name, band and settings, so a store's record of them tells whether the
/// document has moved on since. The strength of a Background extraction
/// step is left out when no step after it changes the input: the canvas
/// applies that strength itself.
pub fn input_program(document: &Document, channel: &str) -> Vec<Value> {
    let steps: Vec<&StepRecord> = document
        .stage(StageKind::Linear)
        .steps
        .iter()
        .filter(|step| step.enabled && changes_input(step, channel))
        .collect();
    steps
        .iter()
        .enumerate()
        .map(|(at, step)| {
            let mut settings = match &step.kind {
                StepKind::Operation { settings, .. } => settings.clone(),
                StepKind::Heal { .. } => Value::Null,
            };
            if step.kind.name() == op::BACKGROUND
                && at + 1 == steps.len()
                && let Some(settings) = settings.as_object_mut()
            {
                settings.remove("strength");
            }
            serde_json::json!({
                "step": step.id.to_string(),
                "operation": step.kind.name(),
                "band": step.band,
                "settings": settings,
            })
        })
        .collect()
}

/// The color group PSF Guard's color tools run on in a document with
/// `channels`: red, green and blue when it has two of them, else the
/// narrowband lines.
fn color_tools_group(channels: &[String]) -> &'static str {
    let present = |members: &[&str; 3]| {
        members
            .iter()
            .filter(|member| channels.iter().any(|channel| channel == *member))
            .count()
    };
    match COLOR_GROUPS
        .iter()
        .find(|(_, members)| present(members) >= 2)
    {
        Some((group, _)) => group,
        None => COLOR_GROUPS[0].0,
    }
}

/// The tool steps one chain of PSF Guard's settings makes, in its order.
fn tool_steps(chain: &Value, band: Option<&str>) -> Vec<StepRecord> {
    chain["steps"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .map(|tool| {
                    let step = tool_step(tool.clone());
                    match band {
                        Some(band) => step.for_band(band),
                        None => step,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The id of the `index`th tool step split from the step `old`: the old
/// id for the first, and for each later one the old id with its last bytes
/// changed by the index. Every program that opens the document gives the
/// split steps the same ids, so a store prepared from a document finds the
/// steps the app split from it.
fn split_id(old: uuid::Uuid, index: usize) -> uuid::Uuid {
    if index == 0 {
        return old;
    }
    let mut bytes = *old.as_bytes();
    bytes[14] ^= 0xa5;
    bytes[15] ^= index as u8;
    uuid::Uuid::from_bytes(bytes)
}

/// Splits each step that kept a chain of tools, as documents did before
/// each tool was a step, into one step per tool, in its place and in the
/// chain's order. The first keeps the old step's id, so a selection or a
/// record of it still finds it, and the others get ids made from it
/// ([`split_id`]); each keeps whether it was on. PSF Guard's
/// color tools, which had no band, take the color group they run on.
pub fn split_tool_chains(document: &mut Document) {
    let channels = document.channels();
    let legacy: Vec<(usize, StepRecord)> = document
        .stage(StageKind::Linear)
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| [op::RC_ASTRO_INPUT, op::RC_ASTRO_COLOR].contains(&step.kind.name()))
        .map(|(at, step)| (at, step.clone()))
        .collect();
    for (at, old) in legacy.into_iter().rev() {
        let StepKind::Operation { settings, .. } = &old.kind else {
            continue;
        };
        let band = match (&old.band, old.kind.name()) {
            (Some(band), _) => band.clone(),
            (None, _) => color_tools_group(&channels).to_string(),
        };
        let mut steps = tool_steps(settings, Some(&band));
        for (index, step) in steps.iter_mut().enumerate() {
            step.enabled = old.enabled;
            step.id = split_id(old.id, index);
        }
        document.remove_step(old.id);
        for (offset, step) in steps.into_iter().enumerate() {
            let _ = document.insert_step(StageKind::Linear, at + offset, step);
        }
    }
}

/// Moves a Linear step one place earlier or later among the steps listed
/// for `channel`: past the step before or after it there. A store prepared
/// after the move runs the steps in the new order, so a tool can run
/// before or after the background extraction. Returns whether the step
/// moved; the first step does not move earlier, nor the last later.
pub fn move_linear_step(
    document: &mut Document,
    step: uuid::Uuid,
    channel: &str,
    later: bool,
) -> bool {
    let listed: Vec<uuid::Uuid> = super::input_steps(document, channel)
        .into_iter()
        .map(|listed| listed.id)
        .collect();
    let steps = &document.stage(StageKind::Linear).steps;
    let positions: Vec<usize> = steps
        .iter()
        .enumerate()
        .filter(|(_, other)| listed.contains(&other.id))
        .map(|(at, _)| at)
        .collect();
    let Some(here) = positions.iter().position(|at| steps[*at].id == step) else {
        return false;
    };
    let target = if later {
        positions.get(here + 1).copied()
    } else {
        here.checked_sub(1).map(|before| positions[before])
    };
    match target {
        Some(target) => document.move_step(step, target),
        None => false,
    }
}

/// The order of what tools do, as one chain runs them: sharpening, noise
/// reduction, star removal.
fn tool_rank(operation: &str) -> usize {
    op::TOOLS
        .iter()
        .position(|other| *other == operation)
        .unwrap_or(op::TOOLS.len())
}

/// Where a new tool step of `operation` for `band` goes in the Linear
/// stage. Among the band's tool steps, it goes before the first that does
/// something later in a chain's order, and else after the last. A band with
/// none takes it after its deconvolution, and else before the first
/// Background extraction step, so a new tool runs on the stack as it came,
/// as PSF Guard's color previews run them. A person can move it after.
pub fn new_tool_step_index(document: &Document, band: &str, operation: &str) -> usize {
    let steps = &document.stage(StageKind::Linear).steps;
    let own: Vec<usize> = steps
        .iter()
        .enumerate()
        .filter(|(_, step)| is_tool_step(step) && step.band.as_deref() == Some(band))
        .map(|(at, _)| at)
        .collect();
    if let Some(later) = own
        .iter()
        .find(|at| tool_rank(steps[**at].kind.name()) > tool_rank(operation))
    {
        return *later;
    }
    if let Some(last) = own.last() {
        return last + 1;
    }
    // A group's tool follows its members' own tools.
    let members: Vec<&str> = COLOR_GROUPS
        .iter()
        .find(|(group, _)| *group == band)
        .map(|(_, members)| members.to_vec())
        .unwrap_or_else(|| vec![band]);
    let after_members = steps
        .iter()
        .rposition(|step| {
            (is_tool_step(step) || step.kind.name() == op::DECONVOLUTION)
                && step
                    .band
                    .as_deref()
                    .is_some_and(|other| members.contains(&other))
        })
        .map(|at| at + 1);
    let background = steps
        .iter()
        .position(|step| step.kind.name() == op::BACKGROUND);
    match (after_members, background) {
        (Some(after), _) => after,
        (None, Some(background)) => background,
        (None, None) => steps
            .iter()
            .position(|step| [op::EMISSION, op::COLOR_CALIBRATION].contains(&step.kind.name()))
            .unwrap_or(steps.len()),
    }
}

/// The groups whose inputs' tools can run together, as one color image:
/// the band of the group's tools step and its inputs in the order the
/// color image holds them. PSF Guard removes stars on each channel stack
/// alone, so these steps are Studio's only.
pub const COLOR_GROUPS: [(&str, [&str; 3]); 2] = [
    ("rgb", ["red", "green", "blue"]),
    ("lines", ["sii", "ha", "oiii"]),
];

/// The color group an input belongs to, if any.
pub fn color_group_of(channel: &str) -> Option<(&'static str, [&'static str; 3])> {
    COLOR_GROUPS
        .iter()
        .find(|(_, inputs)| inputs.contains(&channel))
        .copied()
}

/// The name of the channel that holds `channel` after its stretches, for a
/// channel whose stretches need the whole image, as the layered stretch
/// does. A store holds it so a canvas can draw the result as it is.
pub fn stretched_channel(channel: &str) -> String {
    format!("{channel}@stretched")
}

/// The name of the channel that holds the stars a star removal tool took
/// out of `channel`.
pub fn stars_channel(channel: &str) -> String {
    format!("{channel}@stars")
}

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

    // The Linear stage, in the order the pipeline's phases run: each
    // input's own tools first, then the background, which is fitted on
    // their result.
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
        for step in tool_steps(config, Some(channel)) {
            add(StageKind::Linear, step)?;
        }
    }
    if let Some(background) = settings
        .get("background_extraction")
        .filter(|v| !v.is_null())
    {
        add(
            StageKind::Linear,
            operation(op::BACKGROUND, Accepts::Linear, background),
        )?;
    }
    if let Some(value) = settings.get("emission").filter(|v| !v.is_null()) {
        add(
            StageKind::Linear,
            operation(op::EMISSION, Accepts::Linear, value),
        )?;
    }
    // PSF Guard's color tools run on the color channels together: a color
    // group's tool steps.
    if let Some(chain) = settings.get("color_rc_astro").filter(|v| !v.is_null()) {
        let names: Vec<String> = channels.iter().map(|channel| channel.to_string()).collect();
        for step in tool_steps(chain, Some(color_tools_group(&names))) {
            add(StageKind::Linear, step)?;
        }
    }
    if let Some(value) = settings.get("color_calibration").filter(|v| !v.is_null()) {
        add(
            StageKind::Linear,
            operation(op::COLOR_CALIBRATION, Accepts::Linear, value),
        )?;
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
    // The layers get the colors of the palette that uses the most of the
    // card's channels. A card no palette fits keeps one plain layer per
    // channel.
    let names: Vec<String> = channels.iter().map(|channel| channel.to_string()).collect();
    if let Some(palette) = super::default_palette(&names) {
        document.apply_palette(palette);
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
        .filter(|step| step.enabled)
        // A channel that is another's in another state, such as the stars
        // a tool took out of it, is Studio's: PSF Guard's settings have no
        // place for it.
        .filter(|step| step.band.as_deref().is_none_or(|band| !band.contains('@')));
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
            step.band.clone().ok_or_else(|| {
                refuse(format!(
                    "{operation} applies to one channel, but no channel is set"
                ))
            })
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
            // PSF Guard's settings hold one background for every channel. A
            // channel's own background step is Studio's.
            op::BACKGROUND if step.band.is_none() => {
                settings.insert("background_extraction".to_string(), value.clone());
            }
            op::BACKGROUND => {}
            op::FOLLOW_CHANNELS => {
                settings.insert("input_processing".to_string(), value.clone());
            }
            op::DECONVOLUTION => into_channel_map("input_deconvolutions")?,
            // An input's tool steps are its chain, in their order; a color
            // group's are PSF Guard's color tools, except star removal,
            // which PSF Guard runs on channel stacks only.
            name if op::TOOLS.contains(&name) => {
                let band = channel()?;
                let group = COLOR_GROUPS.iter().any(|(group, _)| *group == band);
                if group && name == op::STAR_REMOVAL {
                    continue;
                }
                let chain = if group {
                    settings
                        .entry("color_rc_astro".to_string())
                        .or_insert_with(|| serde_json::json!({ "steps": [] }))
                } else {
                    let map = per_channel
                        .entry("input_rc_astro".to_string())
                        .or_insert_with(|| Value::Object(Map::new()));
                    match map {
                        Value::Object(map) => map
                            .entry(band)
                            .or_insert_with(|| serde_json::json!({ "steps": [] })),
                        _ => continue,
                    }
                };
                // A chain of PSF Guard's runs one tool of each kind, so a
                // second of a kind is Studio's alone.
                if let Some(tools) = chain["steps"].as_array_mut() {
                    let kind =
                        |tool: &Value| tool_operation(tool["provider"].as_str().unwrap_or(""));
                    if !tools.iter().any(|tool| kind(tool) == kind(value)) {
                        tools.push(value.clone());
                    }
                }
            }
            op::EMISSION => {
                settings.insert("emission".to_string(), value.clone());
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

impl Document {
    /// Gives every input the layered stretch in place of the automatic
    /// midtones stretch: Studio's default. A stretch a person chose, of any
    /// other kind, is left as it is.
    pub fn use_layered_stretch(&mut self) {
        let automatic: Vec<(uuid::Uuid, Value)> = self
            .stage(StageKind::Stretch)
            .steps
            .iter()
            .filter(|step| step.kind.name() == flow_operations::STRETCH)
            .filter_map(|step| match &step.kind {
                StepKind::Operation { settings, .. } if settings["model"]["type"] == "auto-mtf" => {
                    Some((step.id, settings.clone()))
                }
                _ => None,
            })
            .collect();
        for (id, mut settings) in automatic {
            settings["model"] = serde_json::json!({ "type": "layered" });
            self.set_step_settings(id, settings);
        }
    }
}
