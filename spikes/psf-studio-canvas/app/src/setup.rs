//! Setup: what goes where.
//!
//! Setup lists the inputs, the parts of the picture each input goes to, and
//! how the parts combine, beside the final picture. It is another view of
//! the document's layers and not a separate plan: every control here edits
//! the layers the workspace's tree shows, and a layer made or moved in the
//! workspace appears here. A recipe or a part is a start, and nothing made
//! from one is fixed afterwards.

use super::panes::{swatch, CONTROL, PANE, QUIET};
use super::{Page, Studio};
use crate::studio_model::{
    channel_label, layer_name, palettes_for, recipes_for, stars_channel, BlendMode, Document,
    LayerRecord, LayerSource, PartKind, Row, Split,
};
use gpui::{div, prelude::*, px, rgb, AnyElement, Context, SharedString};
use std::collections::BTreeMap;
use uuid::Uuid;

/// The width of the Setup pane.
const SETUP_WIDTH: f32 = 400.0;

fn id(text: String) -> SharedString {
    SharedString::from(text)
}

impl Studio {
    /// The document's inputs: its channels as prepared, without the
    /// states of one, such as its stars, that the store also holds.
    fn inputs(&self) -> Vec<String> {
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

    /// Shows the workspace with one row selected.
    fn open_in_workspace(&mut self, row: Row) {
        self.page = Page::Workspace;
        self.select(row);
    }

    pub(super) fn show_page(&mut self, page: Page) {
        self.page = page;
        if page == Page::Setup {
            // Setup shows the picture the parts make.
            self.set_split(Split::One);
            self.select(Row::Final);
        }
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
                        if this.page == Page::Workspace {
                            this.selected = Row::Layer(layer);
                        }
                    },
                )
            })
            .collect();
        self.menu(format!("add-layer-{key}"), "Add layer", picks, cx)
    }

    /// One layer of a part: its color and name. A click opens it in the
    /// workspace.
    fn layer_chip(&self, layer: &LayerRecord, cx: &mut Context<Self>) -> AnyElement {
        let layer_id = layer.id;
        let waits = matches!(&layer.source, LayerSource::Band { band }
            if !self.band_info.names.contains(band));
        div()
            .id(id(format!("chip-{layer_id}")))
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
                    .w(px(10.0))
                    .h(px(10.0))
                    .rounded_sm()
                    .bg(swatch(layer.color.unwrap_or([1.0; 3]), 1.0)),
            )
            .child(layer.name.clone())
            .when(waits, |chip| {
                chip.child(div().text_color(rgb(QUIET)).child("not prepared"))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_in_workspace(Row::Layer(layer_id));
                cx.notify();
            }))
            .into_any_element()
    }

    /// The inputs, and where each goes.
    fn routing(&self, cx: &mut Context<Self>) -> gpui::Div {
        let routes = self.document.routes();
        let parts: Vec<(Uuid, String)> = self
            .document
            .parts()
            .into_iter()
            .map(|part| (part.id, part.name.clone()))
            .collect();
        let mut table = div().flex().flex_col().gap_1();
        for (row, input) in self.inputs().into_iter().enumerate() {
            let goes: Vec<AnyElement> = routes
                .iter()
                .filter(|route| route.input == input)
                .map(|route| {
                    let part = route
                        .part
                        .and_then(|part| parts.iter().find(|(id, _)| *id == part))
                        .map_or("Top", |(_, name)| name.as_str());
                    let stars = route.shows.contains('@') && !part.contains("tars");
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
                        .child(if stars {
                            format!("{part} stars")
                        } else {
                            part.to_string()
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_in_workspace(Row::Layer(layer));
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect();
            let unused = goes.is_empty();
            let mut picks: Vec<_> = parts
                .iter()
                .enumerate()
                .map(|(index, (part, name))| {
                    let (part, input) = (*part, input.clone());
                    Self::pick(
                        name.clone(),
                        id(format!("route-add-{row}-{index}")),
                        false,
                        cx,
                        move |this| {
                            let (hue, saturation, blend) = Document::default_tone(&input);
                            this.document.add_layer_showing(
                                &input,
                                Some(part),
                                hue,
                                saturation,
                                blend,
                            );
                        },
                    )
                })
                .collect();
            let on_top = input.clone();
            picks.push(Self::pick(
                "Top",
                id(format!("route-add-{row}-top")),
                false,
                cx,
                move |this| {
                    let (hue, saturation, blend) = Document::default_tone(&on_top);
                    this.document
                        .add_layer_showing(&on_top, None, hue, saturation, blend);
                },
            ));
            table = table.child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex_none()
                            .w(px(64.0))
                            .pt_1()
                            .child(channel_label(&input)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .pt_1()
                            .children(goes)
                            .when(unused, |cell| cell.child(Self::note("Not used"))),
                    )
                    .child(div().flex_none().w(px(120.0)).flex().child(self.menu(
                        format!("route-add-{row}"),
                        "Add to",
                        picks,
                        cx,
                    ))),
            );
        }
        table
    }

    /// One part: how it blends, its palette and its layers.
    fn part_card(&self, part: &LayerRecord, cx: &mut Context<Self>) -> gpui::Div {
        let part_id = part.id;
        let layers: Vec<AnyElement> = self
            .document
            .group_members(part_id)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|layer| self.layer_chip(layer, cx))
            .collect();
        let has_palette = self.document.palette_in(Some(part_id)).is_some();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_sm()
            .bg(rgb(PANE))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(part.name.clone()))
                    .child(Self::act(
                        "Open",
                        id(format!("part-open-{part_id}")),
                        false,
                        cx,
                        move |this| this.open_in_workspace(Row::Layer(part_id)),
                    ))
                    .child(Self::act(
                        "Remove",
                        id(format!("part-remove-{part_id}")),
                        false,
                        cx,
                        move |this| {
                            let members: Vec<Uuid> = this
                                .document
                                .group_members(part_id)
                                .map(|layer| layer.id)
                                .collect();
                            for layer in members {
                                this.document.remove_layer(layer);
                            }
                            this.document.remove_layer(part_id);
                        },
                    )),
            )
            .child(Self::field("Blend", self.blend_menu(part, cx)))
            .when(has_palette, |card| {
                card.child(self.palette_block(Some(part_id), cx))
            })
            .child(Self::field(
                "Layers",
                div().flex().flex_wrap().gap_1().children(layers),
            ))
            .child(Self::field("", self.add_layer_menu(Some(part_id), cx)))
            .when(!has_palette, |card| {
                card.child(self.palette_block(Some(part_id), cx))
            })
    }

    /// The layers in no part, when there are any.
    fn loose_card(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let layers: Vec<AnyElement> = self
            .document
            .layers()
            .iter()
            .rev()
            .filter(|layer| layer.group.is_none() && layer.source != LayerSource::Group)
            .map(|layer| self.layer_chip(layer, cx))
            .collect();
        if layers.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_sm()
                .bg(rgb(PANE))
                .child("Top level")
                .child(self.palette_block(None, cx))
                .child(Self::field(
                    "Layers",
                    div().flex().flex_wrap().gap_1().children(layers),
                ))
                .child(Self::field("", self.add_layer_menu(None, cx))),
        )
    }

    /// The Setup pane.
    pub(super) fn setup(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let inputs = self.inputs();
        let recipes = recipes_for(&inputs)
            .into_iter()
            .enumerate()
            .map(|(index, recipe)| {
                Self::pick(recipe.name, ("recipe", index), false, cx, move |this| {
                    let inputs = this.inputs();
                    this.document.use_recipe(recipe, &inputs);
                })
            })
            .collect();
        let kinds = PartKind::ALL
            .into_iter()
            .filter(|kind| kind.fits(&inputs))
            .enumerate()
            .map(|(index, kind)| {
                Self::pick(kind.label(), ("add-part", index), false, cx, move |this| {
                    let inputs = this.inputs();
                    this.document.add_part(kind, &inputs);
                })
            })
            .collect();
        // Top part first, as the tree lists layers.
        let parts: Vec<gpui::Div> = self
            .document
            .parts()
            .into_iter()
            .rev()
            .map(|part| self.part_card(part, cx))
            .collect();
        div()
            .id("setup")
            .flex_none()
            .w(px(SETUP_WIDTH))
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .child(Self::field(
                "Recipe",
                self.menu("recipe", "Start over from…", recipes, cx),
            ))
            .children(Self::help(
                "A recipe replaces the layers that show inputs with its parts. Adjustments stay.",
            ))
            .child(Self::heading("Inputs"))
            .children(Self::help(
                "Where each input goes. Add an input to any part; nothing else changes.",
            ))
            .child(self.routing(cx))
            .child(Self::heading("Parts, top first"))
            .children(self.loose_card(cx).filter(|_| !parts.is_empty()))
            .children(parts)
            .children(
                self.loose_card(cx)
                    .filter(|_| self.document.parts().is_empty()),
            )
            .child(Self::field(
                "",
                self.menu("add-part", "Add part", kinds, cx),
            ))
    }
}
