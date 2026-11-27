//! The left pane: one tree of the whole document.
//!
//! The tree follows the layer panels of Photoshop and Affinity where they
//! agree: the top layer is listed first, each row has its own switch for
//! shown or hidden, an edit that belongs to one layer is indented under that
//! layer, and a row is moved by dragging it. An adjustment dropped on a
//! layer becomes that layer's own; dropped between layers, it changes every
//! layer beneath it.

use super::panes::{
    counted, is_drawn, mask_words, swatch, CONTROL, PANE, QUIET, SELECTED, STRETCHES, TEXT,
};
use super::{Studio, STEPS_WIDTH};
use crate::studio_model::{
    channel_label, input_steps, palettes_for, LayerRecord, LayerSource, Row, StageKind, StepKind,
    StepRecord,
};
use gpui::{div, prelude::*, px, rgb, AnyElement, Context, DragMoveEvent, SharedString, Window};
use uuid::Uuid;

/// A layer being dragged in the tree.
struct DragLayer(Uuid);
/// One layer's own edit being dragged in the tree.
struct DragStep(Uuid);

/// What follows the pointer while a row is dragged.
struct DragLabel(SharedString);

impl Render for DragLabel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(SELECTED))
            .text_color(rgb(TEXT))
            .text_sm()
            .child(self.0.clone())
    }
}

/// How far a row that belongs to the row above it is indented.
const CHILD_INDENT: f32 = 22.0;

impl Studio {
    /// A row of the tree. A click selects it.
    fn tree_row(
        &self,
        id: impl Into<gpui::ElementId>,
        row: Row,
        indent: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let selected = self.selected == row;
        div()
            .id(id)
            .pl(px(8.0 + indent))
            .pr_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(if selected { SELECTED } else { CONTROL }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select(row.clone());
                cx.notify();
            }))
    }

    /// The mark in front of a row that belongs to the row above it.
    fn child_mark() -> gpui::Div {
        div().text_color(rgb(QUIET)).child("↳")
    }

    /// A row's name, with a quiet line under it that says what the row
    /// applies to.
    fn named(name: impl Into<SharedString>, applies_to: impl Into<SharedString>) -> gpui::Div {
        div()
            .flex_1()
            .overflow_hidden()
            .whitespace_nowrap()
            .child(div().truncate().child(name.into()))
            .child(Self::note(applies_to).truncate())
    }

    /// The switch at the end of a row: shown or hidden, on or off.
    fn switch(
        id: impl Into<gpui::ElementId>,
        on: bool,
        words: (&'static str, &'static str),
        cx: &mut Context<Self>,
        flip: impl Fn(&mut Self) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        Self::act(if on { words.0 } else { words.1 }, id, false, cx, flip)
            .text_xs()
            .text_color(rgb(if on { TEXT } else { QUIET }))
    }

    /// What a step's row says under its name.
    fn step_words(&self, stage: StageKind, step: &StepRecord, applies_to: String) -> String {
        let mut words = vec![applies_to];
        words.extend(mask_words(&self.document, &step.mask));
        if !is_drawn(stage, step, &self.band_info) {
            words.push("recorded, not drawn yet".to_string());
        }
        words.join(" · ")
    }

    // ---- inputs ---------------------------------------------------------

    /// The rows of the inputs, or of the stars the tools took out of them.
    fn input_rows(&self, stars: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        let to_run = self.inputs_to_run();
        for (index, channel) in self.document.channels().into_iter().enumerate() {
            if channel.contains('@') != stars {
                continue;
            }
            let label = channel_label(&channel);
            // An input's steps are folded away until a person opens them or
            // selects one, so the layers stay in sight.
            let selected_here = matches!(
                &self.selected,
                Row::Input(own) | Row::InputStep { channel: own, .. } if *own == channel
            );
            let folded = !selected_here && !self.unfolded.contains(&channel);
            let steps = input_steps(&self.document, &channel);
            let fold = {
                let channel = channel.clone();
                div()
                    .id(("fold", index))
                    .w(px(14.0))
                    .text_color(rgb(QUIET))
                    .child(if folded { "▸" } else { "▾" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.unfolded.remove(&channel) {
                            this.unfolded.insert(channel.clone());
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }))
            };
            rows.push(
                self.tree_row(("input", index), Row::Input(channel.clone()), 0.0, cx)
                    .child(fold)
                    .child(Self::named(label.clone(), {
                        let mut words = match self.goes_to(&channel) {
                            Some(goes) => format!("{} {goes}", counted(steps.len(), "step")),
                            None => format!("{} · not used", counted(steps.len(), "step")),
                        };
                        // Its tools changed since the store was prepared.
                        if to_run.contains(&channel) {
                            words.push_str(" · tools not run");
                        }
                        words
                    }))
                    .into_any_element(),
            );
            if folded {
                continue;
            }
            for (at, listed) in steps.into_iter().enumerate() {
                let Some((stage, step)) = self.document.step(listed.id) else {
                    continue;
                };
                let id = listed.id;
                let mut applies_to = if let Some(group) = self.group_applies_to(step) {
                    group
                } else if listed.shared {
                    "every input".to_string()
                } else {
                    format!("{label} only")
                };
                // Which stretch a Stretch step is, so two of them are told
                // apart.
                if let Some((_, stretch)) = super::panes::stretch_model(step)
                    .and_then(|model| serde_json::to_value(model).ok())
                    .and_then(|model| {
                        let kind = model["type"].as_str()?.to_string();
                        STRETCHES.iter().find(|(name, _)| *name == kind)
                    })
                {
                    applies_to = format!("{stretch} · {applies_to}");
                }
                let row = Row::InputStep {
                    channel: channel.clone(),
                    step: id,
                };
                let enabled = step.enabled;
                rows.push(
                    self.tree_row(("input-step", index * 64 + at), row, CHILD_INDENT, cx)
                        .child(Self::child_mark())
                        .child(Self::named(
                            step.kind.name().to_string(),
                            self.step_words(stage, step, applies_to),
                        ))
                        .child(Self::switch(
                            ("input-step-on", index * 64 + at),
                            enabled,
                            ("on", "off"),
                            cx,
                            move |this| {
                                this.document.set_step_enabled(id, !enabled);
                            },
                        ))
                        .into_any_element(),
                );
            }
        }
        rows
    }

    // ---- layers ---------------------------------------------------------

    /// A layer dropped on another layer.
    ///
    /// - An adjustment dropped on a layer or a group becomes that layer's
    ///   or that group's own.
    /// - Any other layer dropped on a group joins the group, on top.
    /// - Any other layer dropped on a layer moves to just above it, or just
    ///   below it when dropped on the row's lower half, in the same group as
    ///   it: so a layer moves anywhere within its part.
    fn drop_layer_on(&mut self, dragged: Uuid, target: Uuid) {
        let lower = self
            .drop_lower
            .take()
            .is_some_and(|(over, lower)| over == target && lower);
        if dragged == target {
            return;
        }
        let source = |studio: &Self, id: Uuid| studio.layer(id).map(|layer| layer.source.clone());
        let is_adjustment = source(self, dragged).is_some_and(|source| source.is_adjustment());
        let target_source = source(self, target);
        let onto_adjustment = target_source
            .as_ref()
            .is_some_and(LayerSource::is_adjustment);
        // An adjustment a layer can have as its own becomes that layer's;
        // any other moves like a layer.
        if is_adjustment
            && !onto_adjustment
            && let Ok(step) = self.document.attach_adjustment(dragged, target)
        {
            self.selected = Row::LayerStep {
                layer: target,
                step,
            };
            return;
        }
        let layers = self.document.layers();
        let Some(at) = layers.iter().position(|layer| layer.id == target) else {
            return;
        };
        // A group holds no group: one dropped anywhere goes above the
        // target's whole part, or above the target at the top level.
        if source(self, dragged) == Some(LayerSource::Group) {
            let top = layers[at]
                .group
                .and_then(|group| layers.iter().position(|layer| layer.id == group))
                .unwrap_or(at);
            self.document.place_layer(dragged, top + 1, None);
            self.selected = Row::Layer(dragged);
            return;
        }
        let (position, group) = if target_source == Some(LayerSource::Group) {
            (at, Some(target))
        } else if lower {
            (at, layers[at].group)
        } else {
            (at + 1, layers[at].group)
        };
        self.document.place_layer(dragged, position, group);
        self.selected = Row::Layer(dragged);
    }

    /// One layer's own edit dropped on another layer or group: it becomes
    /// that one's.
    fn drop_step_on(&mut self, step: Uuid, target: Uuid) {
        let takes_edits = self
            .layer(target)
            .is_some_and(|layer| !layer.source.is_adjustment());
        if !takes_edits || self.document.layer_of_step(step) == Some(target) {
            return;
        }
        if let Some(moved) = self.document.remove_step(step) {
            let id = moved.id;
            if self.document.insert_layer_step(target, moved).is_ok() {
                self.selected = Row::LayerStep {
                    layer: target,
                    step: id,
                };
            }
        }
    }

    /// One layer's own adjustment dropped between layers: it becomes a
    /// layer of the stack there, and changes every layer beneath it, or
    /// every member beneath it when the place is inside a group.
    fn drop_step_between(&mut self, step: Uuid, position: usize, group: Option<Uuid>) {
        let owner = self.document.layer_of_step(step).and_then(|owner| {
            self.document
                .layers()
                .iter()
                .position(|layer| layer.id == owner)
        });
        let (Some(owner), Ok(layer)) = (owner, self.document.detach_adjustment(step)) else {
            return;
        };
        // The new layer went in right above its owner, which moved every
        // position above the owner up by one.
        let position = if position > owner {
            position + 1
        } else {
            position
        };
        self.document.place_layer(layer, position, group);
        self.selected = Row::Layer(layer);
    }

    /// The gap between two layers, where a dragged row can be dropped. A
    /// gap inside a group makes what is dropped a member of the group.
    fn drop_slot(
        &self,
        position: usize,
        group: Option<Uuid>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let inside = group.is_some();
        div()
            .id(("slot", position * 2 + usize::from(inside)))
            .ml(px(if inside { CHILD_INDENT } else { 0.0 }))
            .h(px(10.0))
            .rounded_sm()
            .drag_over::<DragLayer>(|style, _, _, _| style.bg(rgb(SELECTED)))
            .drag_over::<DragStep>(|style, _, _, _| style.bg(rgb(SELECTED)))
            .on_drop(cx.listener(move |this, dragged: &DragLayer, _, cx| {
                this.document.place_layer(dragged.0, position, group);
                this.selected = Row::Layer(dragged.0);
                this.refresh_canvas();
                cx.notify();
            }))
            .on_drop(cx.listener(move |this, dragged: &DragStep, _, cx| {
                this.drop_step_between(dragged.0, position, group);
                this.refresh_canvas();
                cx.notify();
            }))
            .into_any_element()
    }

    /// The layer a clipped layer is clipped to: the nearest layer beneath
    /// it in the same group that is not clipped and not an adjustment.
    fn clipped_to(&self, index: usize) -> Option<&LayerRecord> {
        let layers = self.document.layers();
        let layer = layers.get(index).filter(|layer| layer.clipped)?;
        layers[..index]
            .iter()
            .rev()
            .filter(|other| other.group == layer.group)
            .find(|other| !other.clipped && !other.source.is_adjustment())
    }

    fn layer_row(&self, index: usize, layer: &LayerRecord, cx: &mut Context<Self>) -> AnyElement {
        let id = layer.id;
        let visible = layer.visible;
        let is_adjustment = layer.source.is_adjustment();
        let is_group = layer.source == LayerSource::Group;
        let in_group = layer.group.is_some();
        let mut words = Vec::new();
        if is_group {
            words.push(format!(
                "group of {}",
                counted(self.document.group_members(id).count(), "layer")
            ));
        }
        words.push(if is_adjustment && in_group {
            "every member beneath".to_string()
        } else if is_adjustment {
            "every layer beneath".to_string()
        } else {
            format!("{} · {:.0}%", layer.blend, layer.opacity * 100.0)
        });
        if let Some(base) = self.clipped_to(index) {
            words.push(format!("clipped to {}", base.name));
        }
        words.extend(mask_words(&self.document, &layer.mask));
        let name = layer.name.clone();
        let label: SharedString = name.clone().into();
        let indent = if in_group { CHILD_INDENT } else { 0.0 };
        self.tree_row(("layer", index), Row::Layer(id), indent, cx)
            .when(layer.source.shows_pixels(), |row| {
                row.child(
                    div()
                        .flex_none()
                        .w(px(14.0))
                        .h(px(14.0))
                        .rounded_sm()
                        .bg(swatch(
                            layer.color.unwrap_or([0.6; 3]),
                            if visible { 1.0 } else { 0.25 },
                        )),
                )
            })
            .when(is_adjustment, |row| {
                row.child(div().w(px(14.0)).text_color(rgb(QUIET)).child("◐"))
            })
            .when(is_group, |row| {
                row.child(div().w(px(14.0)).text_color(rgb(QUIET)).child("▾"))
            })
            .child(Self::named(name, words.join(" · ")))
            .child(Self::switch(
                ("layer-shown", index),
                visible,
                ("shown", "hidden"),
                cx,
                move |this| {
                    if let Some(layer) = this.document.layer_mut(id) {
                        layer.visible = !layer.visible;
                    }
                },
            ))
            .on_drag(DragLayer(id), move |_, _, _, cx| {
                cx.new(|_| DragLabel(label.clone()))
            })
            .drag_over::<DragLayer>(|style, _, _, _| style.bg(rgb(SELECTED)))
            .drag_over::<DragStep>(|style, _, _, _| style.bg(rgb(SELECTED)))
            // Which half of the row the pointer is over, for the drop.
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<DragLayer>, _, _| {
                    let position = event.event.position;
                    if event.bounds.contains(&position) {
                        this.drop_lower = Some((id, position.y > event.bounds.center().y));
                    }
                }),
            )
            .on_drop(cx.listener(move |this, dragged: &DragLayer, _, cx| {
                this.drop_layer_on(dragged.0, id);
                this.refresh_canvas();
                cx.notify();
            }))
            .on_drop(cx.listener(move |this, dragged: &DragStep, _, cx| {
                this.drop_step_on(dragged.0, id);
                this.refresh_canvas();
                cx.notify();
            }))
            .into_any_element()
    }

    /// For a star removal on a layer or the picture, a row under it for the
    /// stars it takes out, which go back right after it: what a person sees
    /// of the new image a tool makes, with a switch that leaves them out.
    fn stars_row(
        &self,
        step: &StepRecord,
        indent: f32,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if step.kind.name() != crate::studio_model::flow_operations::STAR_REMOVAL {
            return None;
        }
        let id = step.id;
        let shown = super::late_tools::stars_shown(step);
        let words = match (shown, self.late.has(id)) {
            (_, false) => "made when the tools run",
            (true, true) => "screened back after it",
            (false, true) => "left out",
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .ml(px(indent))
                .child(Self::child_mark())
                .child(Self::named("Stars it took out", words))
                .child(Self::switch(
                    ("stars-shown", index),
                    shown,
                    ("shown", "hidden"),
                    cx,
                    move |this| {
                        let Some((_, step)) = this.document.step(id) else {
                            return;
                        };
                        let mut tool = crate::studio_model::tool_of(step)
                            .cloned()
                            .unwrap_or_default();
                        tool[crate::studio_model::STARS_SHOWN_KEY] = (!shown).into();
                        this.document.set_step_settings(id, tool);
                    },
                ))
                .into_any_element(),
        )
    }

    /// A row for an edit that belongs to one layer, indented under it.
    fn layer_step_row(
        &self,
        index: usize,
        layer: &LayerRecord,
        step: &StepRecord,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = step.id;
        let enabled = step.enabled;
        let row = Row::LayerStep {
            layer: layer.id,
            step: id,
        };
        let label: SharedString = step.kind.name().to_string().into();
        let draggable = matches!(step.kind, StepKind::Operation { .. });
        let indent = if layer.group.is_some() {
            2.0 * CHILD_INDENT
        } else {
            CHILD_INDENT
        };
        self.tree_row(("layer-step", index), row, indent, cx)
            .child(Self::child_mark())
            .child(Self::named(
                step.kind.name().to_string(),
                self.step_words(StageKind::Combine, step, format!("{} only", layer.name)),
            ))
            .child(Self::switch(
                ("layer-step-on", index),
                enabled,
                ("on", "off"),
                cx,
                move |this| {
                    this.document.set_step_enabled(id, !enabled);
                },
            ))
            .when(draggable, |row| {
                row.on_drag(DragStep(id), move |_, _, _, cx| {
                    cx.new(|_| DragLabel(label.clone()))
                })
            })
            .into_any_element()
    }

    fn layer_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let layers = self.document.layers();
        let mut rows = vec![self.drop_slot(layers.len(), None, cx)];
        // The top layer is listed first, as layer lists are read. A group
        // is above its members, so its members follow it, indented.
        for (index, layer) in layers.iter().enumerate().rev() {
            rows.push(self.layer_row(index, layer, cx));
            for (at, step) in layer.steps.iter().enumerate() {
                rows.push(self.layer_step_row(index * 64 + at, layer, step, cx));
                let indent = if layer.group.is_some() {
                    3.0 * CHILD_INDENT
                } else {
                    2.0 * CHILD_INDENT
                };
                rows.extend(self.stars_row(step, indent, 1000 + index * 64 + at, cx));
            }
            let is_group = layer.source == LayerSource::Group;
            // The group this row is in, or is.
            let around = if is_group {
                Some(layer.id)
            } else {
                layer.group
            };
            // Whether the layer beneath is in the same group.
            let goes_on = around.is_some()
                && index
                    .checked_sub(1)
                    .is_some_and(|below| layers[below].group == around);
            if around.is_some() {
                // Beneath this row, inside the group.
                rows.push(self.drop_slot(index, around, cx));
            }
            if !goes_on {
                // Beneath this row, or beneath its whole group, outside it.
                rows.push(self.drop_slot(index, None, cx));
            }
        }
        rows
    }

    /// The masks layers share, each a row of its own: a mask is changed in
    /// one place, for every layer behind it.
    fn mask_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let masks = self.document.masks();
        if masks.is_empty() {
            return Vec::new();
        }
        let mut rows = vec![Self::heading("Masks").into_any_element()];
        for (index, mask) in masks.iter().enumerate() {
            let mut words = vec![format!(
                "shared by {}",
                counted(self.document.layers_using_mask(mask.id).count(), "layer")
            )];
            if !mask.touch_ups.is_empty() {
                words.push(counted(mask.touch_ups.len(), "touch-up"));
            }
            rows.push(
                self.tree_row(("mask", index), Row::Mask(mask.id), 0.0, cx)
                    .child(div().w(px(14.0)).text_color(rgb(QUIET)).child("◑"))
                    .child(Self::named(mask.name.clone(), words.join(" · ")))
                    .into_any_element(),
            );
        }
        rows
    }

    // ---- the pane -------------------------------------------------------

    pub(super) fn tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palettes_for(&self.document.channels())
            .into_iter()
            .find(|palette| self.document.uses_palette(palette))
            .map_or("layers, top first".to_string(), |palette| {
                format!("{} · layers, top first", palette.name)
            });
        let finish = self
            .document
            .stage(StageKind::Finish)
            .steps
            .iter()
            .enumerate()
            .flat_map(|(index, step)| {
                let id = step.id;
                let enabled = step.enabled;
                let row = self
                    .tree_row(("finish", index), Row::Finish(id), 0.0, cx)
                    .child(Self::named(
                        step.kind.name().to_string(),
                        self.step_words(StageKind::Finish, step, "the whole picture".to_string()),
                    ))
                    .child(Self::switch(
                        ("finish-on", index),
                        enabled,
                        ("on", "off"),
                        cx,
                        move |this| {
                            this.document.set_step_enabled(id, !enabled);
                        },
                    ))
                    .into_any_element();
                std::iter::once(row).chain(self.stars_row(step, CHILD_INDENT, index, cx))
            })
            .collect::<Vec<_>>();
        div()
            .id("tree")
            .flex_none()
            .w(px(STEPS_WIDTH))
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .bg(rgb(PANE))
            .child(Self::heading("Inputs"))
            .children(self.input_rows(false, cx))
            .when(
                self.document.channels().iter().any(|c| c.contains('@')),
                |tree| {
                    tree.child(
                        self.section("stars", "Stars and lines", false, cx, |this, cx| {
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .children(this.input_rows(true, cx))
                        }),
                    )
                },
            )
            .child(Self::heading(format!("Combine · {palette}")))
            .child(self.parts_bar(cx))
            .children(self.layer_rows(cx))
            .children(self.mask_rows(cx))
            .child(
                self.tree_row("combined", Row::Combined, 0.0, cx)
                    .child(Self::named(
                        "Combined",
                        if finish.is_empty() {
                            "every layer"
                        } else {
                            "every layer, before Finish"
                        },
                    )),
            )
            // Steps on the picture are adjustment layers; a document from
            // before them may still have Finish steps.
            .when(!finish.is_empty(), |tree| {
                tree.child(Self::heading("Finish")).children(finish)
            })
            .child(
                self.tree_row("final", Row::Final, 0.0, cx)
                    .child(Self::named("Final picture", "every step")),
            )
    }
}
