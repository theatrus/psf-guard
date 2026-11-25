//! What a view of a document shows.
//!
//! A person picks three things, and each is always on screen:
//!
//! - **What:** one channel on its own, or the combined image.
//! - **Up to where:** the steps applied, from none to all of them. This is
//!   how a person sees an intermediate result.
//! - **With what stretch:** the stretch the document gives each channel, or
//!   a preview stretch that is for looking only and is never saved.
//!
//! [`plan`] turns the choice and the document into what a canvas draws.
//! Nothing here knows of a toolkit or a GPU, so a canvas in a native window
//! and one in a web page draw the same thing from the same plan.
//!
//! Channels are never assumed to share settings. A step with a channel
//! applies to that channel; a step without one applies to every channel that
//! has no step of its own of the same kind.

use super::flow::flow_operations as op;
use super::{Document, LayerSource, StageKind, StepKind, StepRecord};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What the view shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    /// One channel, drawn in gray.
    Channel(String),
    /// The layers blended, as the Combine stage does.
    Combined,
    /// One layer on its own, with its color. An adjustment layer has no
    /// pixels, so it is shown with the layers beneath it.
    Layer(Uuid),
    /// The layers from the bottom up to and including this one, blended.
    LayersUpTo(Uuid),
}

/// The last step the view applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpTo {
    /// No step of this stage, and every step of the stages before it.
    StartOf(StageKind),
    /// This step and every step before it.
    Step(Uuid),
    /// Every step of this stage and of the stages before it.
    EndOf(StageKind),
    /// Every step of the document.
    Final,
}

/// The stretch a view draws linear samples with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayStretch {
    /// Each channel's own stretch, from the document. A channel the view
    /// has not stretched yet falls back to the preview stretch.
    #[default]
    Document,
    /// An automatic stretch of each channel on its own, for looking only.
    /// It is not part of the document.
    Preview,
}

/// A person's choice of view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub subject: Subject,
    pub up_to: UpTo,
    pub stretch: DisplayStretch,
}

/// The stretch one channel is drawn with.
#[derive(Clone, Debug, PartialEq)]
pub enum PlannedStretch {
    /// The channel's stretch steps the view applies, in order.
    Document(Vec<Uuid>),
    /// The preview stretch.
    Preview,
}

/// How one channel is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelPlan {
    pub channel: String,
    /// The background step that applies to the channel, if the view has
    /// reached one that is on.
    pub background: Option<Uuid>,
    pub stretch: PlannedStretch,
}

/// One layer of a blended view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackEntry {
    /// A layer that shows a band: the layer, and its place in
    /// [`ViewPlan::channels`].
    Band { layer: Uuid, channel: usize },
    /// A layer that adjusts the layers beneath it.
    Adjustment { layer: Uuid },
}

/// What a canvas draws for a view.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewPlan {
    /// The channels drawn: one for a channel view, each layer's for the
    /// combined view, bottom layer first.
    pub channels: Vec<ChannelPlan>,
    /// Whether the channels are blended with the layers' colors and blend
    /// modes. A channel on its own is drawn in gray.
    pub combined: bool,
    /// The layers drawn, bottom first, when the layers are blended.
    pub stack: Vec<StackEntry>,
    /// The Finish steps the view applies, in order. They act on the
    /// combined image, so a channel view applies none.
    pub finish: Vec<Uuid>,
    /// What the view shows, in a sentence a person reads above the canvas.
    pub description: String,
}

impl Document {
    /// The document's channels, in the order of its sources.
    pub fn channels(&self) -> Vec<String> {
        self.stage(StageKind::Sources)
            .steps
            .iter()
            .filter_map(|step| step.band.clone())
            .collect()
    }

    /// Every step with its stage, in the order the steps apply.
    fn steps_in_order(&self) -> impl Iterator<Item = (StageKind, &StepRecord)> {
        self.stages()
            .iter()
            .flat_map(|stage| stage.steps.iter().map(|step| (stage.kind, step)))
    }
}

/// The steps a view reaches: those up to its last step, on or off.
fn applied(document: &Document, up_to: UpTo) -> Vec<(StageKind, &StepRecord)> {
    let position = |stage: StageKind| StageKind::ALL.iter().position(|kind| *kind == stage);
    let mut steps = Vec::new();
    for (stage, step) in document.steps_in_order() {
        let within = match up_to {
            UpTo::Final => true,
            UpTo::StartOf(end) => position(stage) < position(end),
            UpTo::EndOf(end) => position(stage) <= position(end),
            UpTo::Step(_) => true,
        };
        if !within {
            break;
        }
        steps.push((stage, step));
        if up_to == UpTo::Step(step.id) {
            break;
        }
    }
    steps
}

/// The step named `operation` that applies to `channel`, if it is on. The
/// channel's own step decides when it has one, on or off: a channel whose
/// own step is off gets none, and does not fall back to the shared step.
/// Otherwise the step for every channel decides. The last such step counts.
fn step_for<'a>(
    steps: &[(StageKind, &'a StepRecord)],
    operation: &str,
    channel: &str,
) -> Option<&'a StepRecord> {
    let named = steps
        .iter()
        .map(|(_, step)| *step)
        .filter(|step| step.kind.name() == operation);
    named
        .clone()
        .rfind(|step| step.band.as_deref() == Some(channel))
        .or_else(|| named.rev().find(|step| step.band.is_none()))
        .filter(|step| step.enabled)
}

fn step_name(document: &Document, id: Uuid) -> String {
    document.step(id).map_or_else(
        || "a step that is gone".to_string(),
        |(_, step)| match &step.band {
            Some(band) => format!("{} ({band})", step.kind.name()),
            None => step.kind.name().to_string(),
        },
    )
}

fn stage_word(stage: StageKind) -> &'static str {
    match stage {
        StageKind::Sources => "Sources",
        StageKind::Linear => "Linear",
        StageKind::Stretch => "Stretch",
        StageKind::Combine => "Combine",
        StageKind::Finish => "Finish",
        StageKind::Export => "Export",
    }
}

/// What a canvas draws for `view` of `document`.
pub fn plan(document: &Document, view: &View) -> ViewPlan {
    let steps = applied(document, view.up_to);
    let channel_plan = |channel: &str| {
        let stretches: Vec<Uuid> = steps
            .iter()
            .filter(|(_, step)| step.enabled && step.kind.name() == op::STRETCH)
            .filter(|(_, step)| step.band.as_deref() == Some(channel))
            .map(|(_, step)| step.id)
            .collect();
        ChannelPlan {
            channel: channel.to_string(),
            background: step_for(&steps, op::BACKGROUND, channel).map(|step| step.id),
            stretch: if view.stretch == DisplayStretch::Document && !stretches.is_empty() {
                PlannedStretch::Document(stretches)
            } else {
                PlannedStretch::Preview
            },
        }
    };

    // The layers a blended view draws, bottom first.
    let layers = document.layers();
    let place = |id: &Uuid| layers.iter().position(|layer| layer.id == *id);
    let drawn: &[super::LayerRecord] = match &view.subject {
        Subject::Channel(_) => &[],
        Subject::Combined => layers,
        Subject::LayersUpTo(id) => place(id).map_or(&[], |at| &layers[..=at]),
        Subject::Layer(id) => match place(id) {
            // An adjustment has nothing to show alone.
            Some(at) if matches!(layers[at].source, LayerSource::Adjustment { .. }) => {
                &layers[..=at]
            }
            Some(at) => &layers[at..=at],
            None => &[],
        },
    };
    let combined = !matches!(view.subject, Subject::Channel(_));
    let mut channels = Vec::new();
    let mut stack = Vec::new();
    if let Subject::Channel(channel) = &view.subject {
        channels.push(channel_plan(channel));
    }
    for layer in drawn {
        match &layer.source {
            LayerSource::Band { band } => {
                stack.push(StackEntry::Band {
                    layer: layer.id,
                    channel: channels.len(),
                });
                channels.push(channel_plan(band));
            }
            LayerSource::Adjustment { .. } => {
                stack.push(StackEntry::Adjustment { layer: layer.id })
            }
            // A layer made from a step's result needs the engine.
            LayerSource::Step { .. } => {}
        }
    }
    // The Finish steps act on the whole combined image.
    let finish: Vec<Uuid> = if view.subject == Subject::Combined {
        steps
            .iter()
            .filter(|(stage, step)| *stage == StageKind::Finish && step.enabled)
            .map(|(_, step)| step.id)
            .collect()
    } else {
        Vec::new()
    };

    let layer_name = |id: &Uuid| {
        place(id).map_or_else(
            || "a layer that is gone".to_string(),
            |at| layers[at].name.clone(),
        )
    };
    let what = match &view.subject {
        Subject::Channel(channel) => format!("{channel} on its own"),
        Subject::Combined => "The combined image".to_string(),
        Subject::Layer(id) => match place(id) {
            Some(at) if matches!(layers[at].source, LayerSource::Adjustment { .. }) => {
                format!("{} over the layers beneath it", layer_name(id))
            }
            _ => format!("The layer {} on its own", layer_name(id)),
        },
        Subject::LayersUpTo(id) => format!("The layers up to {}", layer_name(id)),
    };
    let reached = match view.up_to {
        UpTo::Final => "with every step".to_string(),
        UpTo::StartOf(stage) => format!("before the {} stage", stage_word(stage)),
        UpTo::EndOf(stage) => format!("after the {} stage", stage_word(stage)),
        UpTo::Step(id) => format!("after {}", step_name(document, id)),
    };
    let previewed = channels
        .iter()
        .filter(|channel| channel.stretch == PlannedStretch::Preview)
        .count();
    let stretch = if previewed == 0 {
        "the document's stretch".to_string()
    } else if view.stretch == DisplayStretch::Preview {
        "a preview stretch, not saved".to_string()
    } else if previewed == channels.len() {
        "a preview stretch, not saved, because the view stops before the stretch".to_string()
    } else {
        format!(
            "the document's stretch, and a preview stretch, not saved, for the {previewed} channel(s) not stretched yet"
        )
    };
    ViewPlan {
        channels,
        combined,
        stack,
        finish,
        description: format!("{what}, {reached}, drawn with {stretch}."),
    }
}

/// Whether a step belongs in the list a person sees while viewing
/// `subject`: every step for the combined view, and for a channel its own
/// steps and the steps for every channel.
pub fn step_shown_for(step: &StepRecord, subject: &Subject) -> bool {
    match (subject, &step.band) {
        (Subject::Channel(channel), Some(band)) => band == channel,
        _ => true,
    }
}

/// Gives `channel` its own copy of a step that applies to every channel,
/// placed right after it, and returns the copy's id. The channel's own step
/// then applies to it in place of the shared one, so its settings can
/// differ from the other channels'.
///
/// Returns the existing step when the channel already has one.
pub fn own_step_for_channel(document: &mut Document, shared: Uuid, channel: &str) -> Option<Uuid> {
    let (stage, step) = document.step(shared)?;
    if step.band.is_some() {
        return None;
    }
    let StepKind::Operation { operation, .. } = &step.kind else {
        return None;
    };
    let existing = document
        .stage(stage)
        .steps
        .iter()
        .find(|other| other.kind.name() == operation && other.band.as_deref() == Some(channel))
        .map(|other| other.id);
    if existing.is_some() {
        return existing;
    }
    let index = document
        .stage(stage)
        .steps
        .iter()
        .position(|other| other.id == shared)?;
    let mut own = StepRecord::new(step.kind.clone()).for_band(channel);
    own.enabled = step.enabled;
    own.mask = step.mask.clone();
    let id = own.id;
    document.insert_step(stage, index + 1, own).ok()?;
    Some(id)
}
