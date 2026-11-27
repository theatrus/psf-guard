//! One tree for a whole document, and the places in it a canvas can show.
//!
//! The tree reads top to bottom in the order the picture is made:
//!
//! - **Inputs.** Each channel stack, with the steps that apply to it.
//! - **Combine.** The layers, each showing one input in one color, with the
//!   edits that belong to that layer alone listed under it, and the
//!   adjustments that change every layer beneath them listed between layers.
//! - **Finish.** Steps on the combined picture.
//!
//! A person selects a [`Row`]. A canvas pane shows a [`Place`]: the result
//! up to and including a row. By default the pane shows the selected row's
//! place, so there is one choice to make, not one for what to edit and
//! another for what to look at. A pane may be pinned to keep its place while
//! the selection moves, and a canvas may be split into several panes
//! ([`Split`]).
//!
//! Nothing here knows of a toolkit or a GPU.

use super::palette::channel_label;
use super::view::{plan, DisplayStretch, OwnUntil, PlannedStretch, Subject, UpTo, View};
use super::{Document, StageKind};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A row of the tree: what a person selects and edits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Row {
    /// An input, as it was stacked.
    Input(String),
    /// A step, as it applies to one input.
    InputStep { channel: String, step: Uuid },
    /// A layer of the stack: one that shows an input, or an adjustment of
    /// the layers beneath it.
    Layer(Uuid),
    /// An edit that belongs to one layer: an adjustment or a heal.
    LayerStep { layer: Uuid, step: Uuid },
    /// One of the masks layers share.
    Mask(Uuid),
    /// Every layer, blended.
    Combined,
    /// A step on the combined picture.
    Finish(Uuid),
    /// The picture with every step.
    Final,
}

/// What a canvas pane shows: the result up to and including a row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    /// One input in gray, after `after` and the steps before it. `None` is
    /// the input as it was stacked.
    Input {
        channel: String,
        after: Option<Uuid>,
    },
    /// The stack up to and including a layer, or that layer alone.
    Layer { layer: Uuid, alone: bool },
    /// One layer alone, with its own edits up to and including `after`.
    /// `None` is the layer before any of its own edits.
    LayerEdit { layer: Uuid, after: Option<Uuid> },
    /// One of the masks layers share, drawn in gray.
    Mask(Uuid),
    /// Every layer, blended, before the Finish steps.
    Combined,
    /// The combined picture after this Finish step.
    Finish(Uuid),
    /// The picture with every step.
    Final,
}

/// One step in an input's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputStep {
    pub id: Uuid,
    /// Whether the step applies to every input that has no step of its own
    /// of the same kind. A step that is not shared is this input's own.
    pub shared: bool,
}

/// The steps that apply to one input, in order: its own, and the shared
/// steps it has no own step in place of.
pub fn input_steps(document: &Document, channel: &str) -> Vec<InputStep> {
    [StageKind::Linear, StageKind::Stretch]
        .into_iter()
        .flat_map(|stage| {
            let steps = &document.stage(stage).steps;
            steps.iter().filter_map(move |step| match &step.band {
                // A color group's tools belong to the group's inputs.
                Some(band)
                    if super::flow::is_tool_step(step)
                        && super::COLOR_GROUPS.iter().any(|(group, _)| group == band) =>
                {
                    let (group, _) = super::flow::color_group_of(channel)?;
                    (group == band).then_some(InputStep {
                        id: step.id,
                        shared: true,
                    })
                }
                // The star stretch belongs to the stars of red, green and
                // blue, and to no other input.
                None if step.kind.name() == super::flow_operations::STAR_STRETCH
                    && !["red@stars", "green@stars", "blue@stars"].contains(&channel) =>
                {
                    None
                }
                Some(band) if band == channel => Some(InputStep {
                    id: step.id,
                    shared: false,
                }),
                Some(_) => None,
                None => {
                    let replaced = steps.iter().any(|other| {
                        other.kind.name() == step.kind.name()
                            && other.band.as_deref() == Some(channel)
                    });
                    (!replaced).then_some(InputStep {
                        id: step.id,
                        shared: true,
                    })
                }
            })
        })
        .collect()
}

impl Row {
    /// What a pane shows for this row. `alone` shows a selected layer
    /// without the layers beneath it.
    pub fn place(&self, alone: bool) -> Place {
        match self {
            Row::Input(channel) => Place::Input {
                channel: channel.clone(),
                after: None,
            },
            Row::InputStep { channel, step } => Place::Input {
                channel: channel.clone(),
                after: Some(*step),
            },
            Row::Layer(layer) => Place::Layer {
                layer: *layer,
                alone,
            },
            // An edit of one layer is judged on that layer.
            Row::LayerStep { layer, step } => Place::LayerEdit {
                layer: *layer,
                after: Some(*step),
            },
            Row::Mask(mask) => Place::Mask(*mask),
            Row::Combined => Place::Combined,
            Row::Finish(step) => Place::Finish(*step),
            Row::Final => Place::Final,
        }
    }
}

impl Place {
    /// The view a canvas draws for this place. A place before an input's
    /// stretch is drawn with the view stretch, which is not saved.
    pub fn view(&self) -> View {
        let own_until = match self {
            Place::LayerEdit { layer, after } => Some(OwnUntil {
                layer: *layer,
                after: *after,
            }),
            _ => None,
        };
        let (subject, up_to) = match self {
            Place::Input {
                channel,
                after: None,
            } => (
                Subject::Channel(channel.clone()),
                UpTo::StartOf(StageKind::Linear),
            ),
            Place::Input {
                channel,
                after: Some(step),
            } => (Subject::Channel(channel.clone()), UpTo::Step(*step)),
            Place::Layer { layer, alone: true } | Place::LayerEdit { layer, .. } => {
                (Subject::Layer(*layer), UpTo::EndOf(StageKind::Combine))
            }
            Place::Layer {
                layer,
                alone: false,
            } => (Subject::LayersUpTo(*layer), UpTo::EndOf(StageKind::Combine)),
            Place::Mask(mask) => (Subject::Mask(*mask), UpTo::EndOf(StageKind::Stretch)),
            Place::Combined => (Subject::Combined, UpTo::EndOf(StageKind::Combine)),
            Place::Finish(step) => (Subject::Combined, UpTo::Step(*step)),
            Place::Final => (Subject::Combined, UpTo::Final),
        };
        View {
            subject,
            up_to,
            stretch: DisplayStretch::Document,
            own_until,
        }
    }

    /// The place one row earlier, for a before and after comparison. A
    /// place with nothing before it returns itself.
    pub fn before(&self, document: &Document) -> Place {
        let layers = document.layers();
        let finish = &document.stage(StageKind::Finish).steps;
        match self {
            Place::Input { after: None, .. } => self.clone(),
            Place::Input {
                channel,
                after: Some(step),
            } => {
                let steps = input_steps(document, channel);
                let at = steps.iter().position(|row| row.id == *step);
                Place::Input {
                    channel: channel.clone(),
                    after: at
                        .and_then(|at| at.checked_sub(1))
                        .map(|before| steps[before].id),
                }
            }
            Place::Layer { layer, .. } => layers
                .iter()
                .position(|other| other.id == *layer)
                // Before a group is the stack beneath its members.
                .map(|at| {
                    layers[..at]
                        .iter()
                        .rposition(|other| other.group != Some(*layer))
                        .map_or(0, |below| below + 1)
                })
                .and_then(|at| at.checked_sub(1))
                .map_or_else(
                    || self.clone(),
                    |below| Place::Layer {
                        layer: layers[below].id,
                        // A layer alone has nothing beneath it to compare
                        // with, so the comparison is with the stack.
                        alone: false,
                    },
                ),
            Place::LayerEdit { after: None, .. } => self.clone(),
            Place::LayerEdit {
                layer,
                after: Some(step),
            } => {
                let own = layers
                    .iter()
                    .find(|other| other.id == *layer)
                    .map_or(&[][..], |layer| &layer.steps);
                Place::LayerEdit {
                    layer: *layer,
                    after: own
                        .iter()
                        .position(|other| other.id == *step)
                        .and_then(|at| at.checked_sub(1))
                        .map(|before| own[before].id),
                }
            }
            Place::Combined | Place::Mask(_) => self.clone(),
            Place::Finish(step) => finish
                .iter()
                .position(|other| other.id == *step)
                .and_then(|at| at.checked_sub(1))
                .map_or(Place::Combined, |before| Place::Finish(finish[before].id)),
            Place::Final => Place::Combined,
        }
    }

    /// A short label for the pane: what it shows, up to where, and whether
    /// the stretch is the document's or a preview.
    pub fn label(&self, document: &Document) -> String {
        let step_name = |id: &Uuid| {
            document
                .step(*id)
                .map_or("a step that is gone", |(_, step)| step.kind.name())
                .to_string()
        };
        let layer_name = |id: &Uuid| {
            document
                .layers()
                .iter()
                .find(|layer| layer.id == *id)
                .map_or("a layer that is gone".to_string(), |layer| {
                    layer.name.clone()
                })
        };
        let what = match self {
            Place::Input {
                channel,
                after: None,
            } => format!("{} · as stacked", channel_label(channel)),
            Place::Input {
                channel,
                after: Some(step),
            } => {
                // An input may have several steps of one name, such as two
                // stretches: the label then says which.
                let name = step_name(step);
                let alike: Vec<Uuid> = input_steps(document, channel)
                    .into_iter()
                    .map(|row| row.id)
                    .filter(|id| step_name(id) == name)
                    .collect();
                let which = alike.iter().position(|id| id == step);
                match which {
                    Some(at) if alike.len() > 1 => {
                        format!("{} · after {name} {}", channel_label(channel), at + 1)
                    }
                    _ => format!("{} · after {name}", channel_label(channel)),
                }
            }
            Place::Layer { layer, alone: true } => format!("{} · alone", layer_name(layer)),
            Place::Layer {
                layer,
                alone: false,
            } => format!("Layers up to {}", layer_name(layer)),
            Place::LayerEdit { layer, after: None } => {
                format!("{} · alone · before its own edits", layer_name(layer))
            }
            Place::LayerEdit {
                layer,
                after: Some(step),
            } => format!("{} · alone · after {}", layer_name(layer), step_name(step)),
            Place::Mask(mask) => format!(
                "Mask: {} · white is where it shows",
                document
                    .mask(*mask)
                    .map_or("a mask that is gone", |mask| mask.name.as_str())
            ),
            Place::Combined => "Combined · before Finish".to_string(),
            Place::Finish(step) => format!("Combined · after {}", step_name(step)),
            Place::Final => "Final picture".to_string(),
        };
        let planned = plan(document, &self.view());
        let previewed = planned
            .channels
            .iter()
            .any(|channel| channel.stretch == PlannedStretch::Preview);
        if previewed {
            format!("{what} · view stretch, not saved")
        } else {
            what
        }
    }
}

/// How a canvas is divided into panes.
///
/// Every split but [`Split::One`] ends with the final picture, so a person
/// always sees what an edit does to the result. The panes before it follow
/// the selected row. A person may pin any pane to a place of their own,
/// such as a stage to compare against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Split {
    /// One pane: the selected row.
    One,
    /// The selected row, and the final picture.
    #[default]
    WithPicture,
    /// The row before the selected one, the selected row, and the final
    /// picture.
    BeforeAndAfter,
    /// Before and after in one pane, with a divider a person moves: the
    /// row before the selected one on its left, the selected row on its
    /// right.
    Wipe,
}

impl Split {
    /// Every split, in the order a person is offered them.
    pub const ALL: [Split; 4] = [
        Split::WithPicture,
        Split::BeforeAndAfter,
        Split::Wipe,
        Split::One,
    ];

    /// The split's name, as a person reads it.
    pub fn name(self) -> &'static str {
        match self {
            Split::One => "Single",
            Split::WithPicture => "With final",
            Split::BeforeAndAfter => "Before, after, final",
            Split::Wipe => "Wipe",
        }
    }
}

/// The places a split canvas shows, left to right, while `selected` is the
/// place of the selected row. A pane is not repeated: with the final
/// picture selected, "With final" is one pane.
pub fn panes(split: Split, selected: &Place, document: &Document) -> Vec<Place> {
    let mut panes = match split {
        Split::One => vec![selected.clone()],
        Split::WithPicture => vec![selected.clone(), Place::Final],
        Split::BeforeAndAfter => vec![selected.before(document), selected.clone(), Place::Final],
        // A wipe is always its two sides.
        Split::Wipe => return vec![selected.before(document), selected.clone()],
    };
    panes.dedup();
    panes
}
