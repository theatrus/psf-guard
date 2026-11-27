//! Parts of the picture, in the workspace.
//!
//! What goes where is answered in the tree and the inspector, not on a page
//! of its own, so a person sees what they are building while they set it
//! up. The tree shows each input with the parts it goes to; the Combine
//! heading has the Recipe and Add part menus; an input's controls add it to
//! any part; a part's controls hold its blend, palette, slots and layers.
//! A recipe or a part is a start, and nothing made from one is fixed.

use super::panes::{swatch, CONTROL};
use super::Studio;
use crate::studio_model::{
    channel_label, layer_name, palettes_for, recipes_for, stars_channel, BlendMode, Document,
    LayerRecord, LayerSource, PartKind, Row,
};
use gpui::{div, prelude::*, px, rgb, AnyElement, Context, SharedString};
use std::collections::BTreeMap;
use uuid::Uuid;

fn id(text: String) -> SharedString {
    SharedString::from(text)
}

impl Studio {
    /// The document's inputs: its channels as prepared, without the
    /// states of one, such as its stars, that the store also holds.
    pub(super) fn inputs(&self) -> Vec<String> {
        self.document
            .channels()
            .into_iter()
            .filter(|channel| !channel.contains('@'))
            .collect()
    }

    /// What a layer can show: every input, and the stars of each input the
    /// store has stars for.
    pub(super) fn showable(&self) -> Vec<String> {
        let inputs = self.inputs();
        let stars = inputs
            .iter()
            .map(|input| stars_channel(input))
            .filter(|stars| self.band_info.names.contains(stars))
            .collect::<Vec<_>>();
        inputs.into_iter().chain(stars).collect()
    }

    /// A layer's or a group's blend mode, as a menu.
    pub(super) fn blend_menu(&self, layer: &LayerRecord, cx: &mut Context<Self>) -> gpui::Div {
        let layer_id = layer.id;
        let picks = BlendMode::ALL
            .into_iter()
            .enumerate()
            .map(|(index, mode)| {
                Self::pick(
                    mode.to_string(),
                    id(format!("blend-{layer_id}-{index}")),
                    layer.blend == mode,
                    cx,
                    move |this| {
                        if let Some(layer) = this.document.layer_mut(layer_id) {
                            layer.blend = mode;
                            if let LayerSource::Band { band } = &layer.source {
                                layer.name =
                                    layer_name(band, layer.color.unwrap_or([1.0; 3]), layer.blend);
                            }
                        }
                    },
                )
            })
            .collect();
        self.menu(format!("blend-{layer_id}"), "Blend", picks, cx)
    }

    /// The palette of a part, or of the layers in no part, and the input
    /// each of its slots shows.
    pub(super) fn palette_block(&self, part: Option<Uuid>, cx: &mut Context<Self>) -> gpui::Div {
        let key = part.map_or("top".to_string(), |part| part.to_string());
        let used = self.document.palette_in(part).cloned();
        let kept: BTreeMap<String, String> = used
            .as_ref()
            .map(|used| used.inputs.clone())
            .unwrap_or_default();
        let palettes = palettes_for(&self.inputs())
            .into_iter()
            .enumerate()
            .map(|(index, palette)| {
                let kept = kept.clone();
                Self::pick(
                    palette.name,
                    id(format!("palette-{key}-{index}")),
                    self.document.uses_palette_in(part, palette),
                    cx,
                    move |this| {
                        // A slot the new palette shares keeps its input.
                        let inputs: BTreeMap<String, String> = kept
                            .iter()
                            .filter(|(slot, _)| palette.slots().contains(&slot.as_str()))
                            .map(|(slot, input)| (slot.clone(), input.clone()))
                            .collect();
                        this.document.apply_palette_in(part, palette, &inputs);
                    },
                )
            })
            .collect::<Vec<_>>();
        let mut block = div().flex().flex_col().gap_1().child(Self::field(
            "Palette",
            self.menu(format!("palette-{key}"), "None", palettes, cx),
        ));
        block = block.children(Self::help(
            "A palette replaces the layers it made. Layers you added stay.",
        ));
        let showable = self.showable();
        for (row, (slot, shown)) in kept.into_iter().enumerate() {
            let picks = showable
                .iter()
                .enumerate()
                .map(|(index, input)| {
                    let (slot, input) = (slot.clone(), input.clone());
                    Self::pick(
                        channel_label(&input),
                        id(format!("slot-{key}-{row}-{index}")),
                        input == shown,
                        cx,
                        move |this| {
                            this.document.set_palette_input_in(part, &slot, &input);
                        },
                    )
                })
                .collect();
            block = block.child(Self::field(
                channel_label(&slot),
                self.menu(format!("slot-{key}-{slot}"), "Input", picks, cx),
            ));
        }
        block
    }

    /// The menu that adds a layer showing an input to a part, or on top of
    /// every layer.
    pub(super) fn add_layer_menu(&self, part: Option<Uuid>, cx: &mut Context<Self>) -> gpui::Div {
        let key = part.map_or("top".to_string(), |part| part.to_string());
        let picks = self
            .showable()
            .into_iter()
            .enumerate()
            .map(|(index, input)| {
                Self::pick(
                    channel_label(&input),
                    id(format!("add-layer-{key}-{index}")),
                    false,
                    cx,
                    move |this| {
                        let (hue, saturation, blend) = Document::default_tone(&input);
                        let layer = this
                            .document
                            .add_layer_showing(&input, part, hue, saturation, blend);
                        this.selected = Row::Layer(layer);
                    },
                )
            })
            .collect();
        self.menu(format!("add-layer-{key}"), "Add layer", picks, cx)
    }

    /// The parts an input goes to, as words for its row in the tree.
    pub(super) fn goes_to(&self, input: &str) -> Option<String> {
        let parts = self.document.parts();
        let mut names: Vec<&str> = Vec::new();
        for route in self.document.routes() {
            if route.shows != input {
                continue;
            }
            let name = route
                .part
                .and_then(|part| parts.iter().find(|group| group.id == part))
                .map_or("top", |group| group.name.as_str());
            if !names.contains(&name) {
                names.push(name);
            }
        }
        (!names.is_empty()).then(|| format!("→ {}", names.join(", ")))
    }

    /// Where an input goes: one chip per layer that shows it, and the menu
    /// that adds it to a part. A click on a chip selects the layer.
    pub(super) fn routes_block(&self, input: &str, cx: &mut Context<Self>) -> gpui::Div {
        let parts: Vec<(Uuid, String)> = self
            .document
            .parts()
            .into_iter()
            .map(|part| (part.id, part.name.clone()))
            .collect();
        let chips: Vec<AnyElement> = self
            .document
            .routes()
            .into_iter()
            .filter(|route| route.shows == input)
            .map(|route| {
                let part = route
                    .part
                    .and_then(|part| parts.iter().find(|(id, _)| *id == part))
                    .map_or("Top", |(_, name)| name.as_str())
                    .to_string();
                let layer = route.layer;
                let color = self
                    .layer(layer)
                    .and_then(|layer| layer.color)
                    .unwrap_or([1.0; 3]);
                div()
                    .id(id(format!("route-{layer}")))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(rgb(CONTROL))
                    .text_xs()
                    .child(
                        div()
                            .flex_none()
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded_sm()
                            .bg(swatch(color, 1.0)),
                    )
                    .child(part)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select(Row::Layer(layer));
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let unused = chips.is_empty();
        let mut picks: Vec<_> = parts
            .iter()
            .enumerate()
            .map(|(index, (part, name))| {
                let (part, input) = (*part, input.to_string());
                Self::pick(
                    name.clone(),
                    id(format!("route-add-{index}")),
                    false,
                    cx,
                    move |this| {
                        let (hue, saturation, blend) = Document::default_tone(&input);
                        let layer = this.document.add_layer_showing(
                            &input,
                            Some(part),
                            hue,
                            saturation,
                            blend,
                        );
                        this.selected = Row::Layer(layer);
                    },
                )
            })
            .collect();
        let on_top = input.to_string();
        picks.push(Self::pick("Top", "route-add-top", false, cx, move |this| {
            let (hue, saturation, blend) = Document::default_tone(&on_top);
            let layer = this
                .document
                .add_layer_showing(&on_top, None, hue, saturation, blend);
            this.selected = Row::Layer(layer);
        }));
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Self::field(
                "Goes to",
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .pt_1()
                    .children(chips)
                    .when(unused, |cell| cell.child(Self::note("Not used"))),
            ))
            .child(Self::field("", self.menu("route-add", "Add to", picks, cx)))
    }

    /// The menus over the layers: start from a recipe, or add a part.
    /// Makes the picture from a recipe's parts. A recipe that shows stars
    /// apart has them separated.
    pub(super) fn choose_recipe(&mut self, recipe: &crate::studio_model::Recipe) {
        let inputs = self.inputs();
        self.document.use_recipe(recipe, &inputs);
        self.selected = Row::Combined;
        self.separate_stars();
    }

    /// Adds a part of `kind` and selects it; a part that shows stars apart
    /// has them separated.
    pub(super) fn add_part_of(&mut self, kind: PartKind) {
        let inputs = self.inputs();
        if let Some(part) = self.document.add_part(kind, &inputs) {
            self.selected = Row::Layer(part);
        }
        self.separate_stars();
    }

    pub(super) fn parts_bar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let inputs = self.inputs();
        let recipes = recipes_for(&inputs)
            .into_iter()
            .enumerate()
            .map(|(index, recipe)| {
                Self::pick(recipe.name, ("recipe", index), false, cx, move |this| {
                    this.choose_recipe(recipe);
                })
            })
            .collect();
        let kinds = PartKind::ALL
            .into_iter()
            .filter(|kind| kind.fits(&inputs))
            .enumerate()
            .map(|(index, kind)| {
                Self::pick(kind.label(), ("add-part", index), false, cx, move |this| {
                    this.add_part_of(kind);
                })
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.menu("recipe", "Recipe", recipes, cx))
            .child(self.menu("add-part", "Add part", kinds, cx))
            .when(self.stars_missing() && !self.is_running(), |bar| {
                bar.child(Self::note(
                    "The stars layers show no stars yet: their inputs' stars have not been removed. Choose the recipe again to remove them with a star removal tool and prepare the store, or give each input a star removal tool and run the tools.",
                ))
            })
    }

    /// Removes a part with its layers.
    pub(super) fn remove_part(&mut self, part: Uuid) {
        self.document.remove_part(part);
        self.selected = Row::Combined;
    }
}
