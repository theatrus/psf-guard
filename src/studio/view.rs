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
    /// One of the document's shared masks, drawn in gray: white where a
    /// layer behind it shows.
    Mask(Uuid),
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

/// How many of one layer's own edits a view applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnUntil {
    /// The layer whose own edits are limited.
    pub layer: Uuid,
    /// The last of the layer's own edits the view applies. `None` applies
    /// none of them.
    pub after: Option<Uuid>,
}

/// A person's choice of view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub subject: Subject,
    pub up_to: UpTo,
    pub stretch: DisplayStretch,
    /// A limit on one layer's own edits, for a before and after of one of
    /// them. `None` applies every layer's own edits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own_until: Option<OwnUntil>,
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
    /// The channel's processing tools step, or its deconvolution step, if
    /// the view has reached one that is on. The channel is then shown as the tools left
    /// it.
    pub tools: Option<Uuid>,
    pub stretch: PlannedStretch,
    /// The heals of this channel alone that the view applies, in order.
    /// They change the channel's samples before its stretch.
    pub heals: Vec<Uuid>,
}

/// One layer of a blended view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackEntry {
    /// A layer that shows a band: the layer, its place in
    /// [`ViewPlan::channels`], and the layer it is clipped to, if any. A
    /// clipped layer shows only inside that layer's mask, and not at all
    /// while that layer is hidden.
    Band {
        layer: Uuid,
        channel: usize,
        clip: Option<Uuid>,
    },
    /// A layer that adjusts the layers beneath it. Inside a group, those
    /// are the group's members beneath it.
    Adjustment { layer: Uuid },
    /// The start of a group's members. The entries up to the group's end
    /// blend together on their own, starting from nothing.
    GroupBegin { group: Uuid },
    /// The end of a group: its members' blend is treated as one layer with
    /// the group's opacity, blend, mask and own steps.
    GroupEnd { group: Uuid },
}

/// What a canvas draws for a view.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewPlan {
    /// The channels drawn: one for a channel view, each layer's for the
    /// combined view, bottom layer first.
    pub channels: Vec<ChannelPlan>,
    /// Every input of the document, as the view applies its steps: the
    /// background step and the stretch a computed mask reads an input
    /// with.
    pub inputs: Vec<ChannelPlan>,
    /// Whether the channels are blended with the layers' colors and blend
    /// modes. A channel on its own is drawn in gray.
    pub combined: bool,
    /// The layers drawn, bottom first, when the layers are blended.
    pub stack: Vec<StackEntry>,
    /// The drawn layers' own edits that the view applies: those that are
    /// on, up to the view's limit for a layer that has one.
    pub own_steps: Vec<Uuid>,
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
        // The stars a tool took out of an input are stretched as that
        // input is, so the two go back together.
        // A line with its continuum subtracted is stretched as its channel.
        let stretched_as = channel
            .strip_suffix("@stars")
            .or_else(|| channel.strip_suffix("@line"))
            .unwrap_or(channel);
        let stretches: Vec<Uuid> = steps
            .iter()
            .filter(|(_, step)| step.enabled && step.kind.name() == op::STRETCH)
            .filter(|(_, step)| step.band.as_deref() == Some(stretched_as))
            .map(|(_, step)| step.id)
            .collect();
        ChannelPlan {
            channel: channel.to_string(),
            background: step_for(&steps, op::BACKGROUND, channel).map(|step| step.id),
            // The store holds an input after its deconvolution and its
            // tools together, so either step reached shows that state.
            tools: step_for(&steps, op::RC_ASTRO_INPUT, channel)
                .or_else(|| step_for(&steps, op::DECONVOLUTION, channel))
                .map(|step| step.id),
            stretch: if view.stretch == DisplayStretch::Document && !stretches.is_empty() {
                PlannedStretch::Document(stretches)
            } else {
                PlannedStretch::Preview
            },
            heals: steps
                .iter()
                .filter(|(stage, step)| {
                    *stage == StageKind::Linear
                        && step.enabled
                        && matches!(step.kind, StepKind::Heal { .. })
                        && step.band.as_deref() == Some(channel)
                })
                .map(|(_, step)| step.id)
                .collect(),
        }
    };

    // The layers a blended view draws, bottom first.
    let layers = document.layers();
    let place = |id: &Uuid| layers.iter().position(|layer| layer.id == *id);
    let drawn: &[super::LayerRecord] = match &view.subject {
        Subject::Channel(_) | Subject::Mask(_) => &[],
        Subject::Combined => layers,
        Subject::LayersUpTo(id) => place(id).map_or(&[], |at| &layers[..=at]),
        Subject::Layer(id) => match place(id) {
            // An adjustment has nothing to show alone.
            Some(at) if matches!(layers[at].source, LayerSource::Adjustment { .. }) => {
                &layers[..=at]
            }
            // A group alone is its members and itself.
            Some(at) if layers[at].source == LayerSource::Group => {
                let low = layers[..at]
                    .iter()
                    .rposition(|layer| layer.group != Some(*id))
                    .map_or(0, |other| other + 1);
                &layers[low..=at]
            }
            Some(at) => &layers[at..=at],
            None => &[],
        },
    };
    // One member alone is shown as it is, not as its group blends it.
    let within_groups = drawn.len() != 1 || drawn[0].group.is_none();
    // The layer a clipped layer is clipped to: the nearest layer beneath it
    // in the same group that shows pixels or is a group, and is not itself
    // clipped.
    let clip_of = |layer: &super::LayerRecord| -> Option<Uuid> {
        let at = place(&layer.id)?;
        layers[..at]
            .iter()
            .rev()
            .filter(|other| other.group == layer.group)
            .find(|other| !other.clipped && !matches!(other.source, LayerSource::Adjustment { .. }))
            .map(|other| other.id)
    };
    let combined = !matches!(view.subject, Subject::Channel(_));
    let mut channels = Vec::new();
    let mut stack = Vec::new();
    if let Subject::Channel(channel) = &view.subject {
        channels.push(channel_plan(channel));
    }
    // The group whose members are being listed.
    let mut open: Option<Uuid> = None;
    for layer in drawn {
        let belongs_to = if layer.source == LayerSource::Group {
            Some(layer.id)
        } else {
            layer.group.filter(|_| within_groups)
        };
        if open != belongs_to {
            if let Some(group) = open.take() {
                stack.push(StackEntry::GroupEnd { group });
            }
            if let Some(group) = belongs_to {
                stack.push(StackEntry::GroupBegin { group });
                open = Some(group);
            }
        }
        match &layer.source {
            LayerSource::Band { band } => {
                stack.push(StackEntry::Band {
                    layer: layer.id,
                    channel: channels.len(),
                    clip: layer.clipped.then(|| clip_of(layer)).flatten(),
                });
                channels.push(channel_plan(band));
            }
            LayerSource::Adjustment { .. } => {
                stack.push(StackEntry::Adjustment { layer: layer.id })
            }
            LayerSource::Group => {
                stack.push(StackEntry::GroupEnd { group: layer.id });
                open = None;
            }
            // A layer made from a step's result needs the engine.
            LayerSource::Step { .. } => {}
        }
    }
    // A view that stops inside a group shows the group as far as it goes.
    if let Some(group) = open {
        stack.push(StackEntry::GroupEnd { group });
    }
    let own_steps: Vec<Uuid> = drawn
        .iter()
        .flat_map(|layer| {
            let limit = view.own_until.filter(|limit| limit.layer == layer.id);
            let count = match limit {
                None => layer.steps.len(),
                Some(OwnUntil { after: None, .. }) => 0,
                Some(OwnUntil {
                    after: Some(last), ..
                }) => layer
                    .steps
                    .iter()
                    .position(|step| step.id == last)
                    .map_or(layer.steps.len(), |at| at + 1),
            };
            layer.steps[..count]
                .iter()
                .filter(|step| step.enabled)
                .map(|step| step.id)
        })
        .collect();
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
        Subject::Mask(id) => format!(
            "The mask {}",
            document
                .mask(*id)
                .map_or("that is gone", |mask| mask.name.as_str())
        ),
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
        inputs: document
            .channels()
            .iter()
            .map(|channel| channel_plan(channel))
            .collect(),
        channels,
        combined,
        stack,
        own_steps,
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
    own.opacity = step.opacity;
    let id = own.id;
    document.insert_step(stage, index + 1, own).ok()?;
    Some(id)
}
