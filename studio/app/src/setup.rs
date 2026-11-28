//! Setup: what the picture is made of, in one place.
//!
//! **Setup** is the first row of the tree. Choosing it shows, over the
//! canvas, the recipe the layers started from, where each input goes, and a
//! card for each part. Nothing here is stored apart from the layers: every
//! line is read from the document as it is, and every control changes the
//! same layers the tree lists, so the tree stays in view and agrees with it.
//! Choosing any other row of the tree shows the canvas again.

use super::panes::{swatch, CONTROL, PANE, QUIET, SELECTED};
use super::Studio;
use crate::studio_model::{
    channel_label, color_word, recipes_for, LayerRecord, LayerSource, PartKind, Row,
};
use gpui::{div, prelude::*, px, rgb, AnyElement, Context, SharedString};
use uuid::Uuid;

fn id(text: String) -> SharedString {
    SharedString::from(text)
}

impl Studio {
    /// Shows Setup over the canvas.
    pub(super) fn open_setup(&mut self) {
        self.open_menu = None;
        self.setup_open = true;
    }

    /// The Setup row at the top of the tree.
    pub(super) fn setup_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let parts = self.top_parts().len();
        let words = match &self.document.recipe {
            Some(recipe) => format!("{recipe} · {}", super::panes::counted(parts, "part")),
            None => super::panes::counted(parts, "part"),
        };
        div()
            .id("setup-row")
            .px_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(if self.setup_open { SELECTED } else { CONTROL }))
            .child(Self::named("Setup", words))
            .on_click(cx.listener(|this, _, _, cx| {
                this.open_setup();
                cx.notify();
            }))
            .into_any_element()
    }

    /// The parts at the top of the stack, top first: the groups no other
    /// group holds.
    fn top_parts(&self) -> Vec<LayerRecord> {
        let mut parts: Vec<LayerRecord> = self
            .document
            .parts()
            .into_iter()
            .filter(|part| part.group.is_none())
            .cloned()
            .collect();
        parts.reverse();
        parts
    }

    /// What a part shows, top first as the tree lists it: "Ha as green,
    /// OIII as teal".
    fn part_members(&self, part: Uuid) -> String {
        let members: Vec<&LayerRecord> = self.document.group_members(part).collect();
        let shown: Vec<String> = members
            .into_iter()
            .rev()
            .filter_map(|layer| match &layer.source {
                LayerSource::Band { band } => Some(match layer.color {
                    Some(color) => format!("{} as {}", channel_label(band), color_word(color)),
                    None => channel_label(band),
                }),
                LayerSource::Group => Some(format!("the group {}", layer.name)),
                _ => None,
            })
            .collect();
        if shown.is_empty() {
            "No layers yet".to_string()
        } else {
            shown.join(", ")
        }
    }

    /// Setup, over the canvas.
    pub(super) fn setup_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let inputs = self.inputs();
        let recipe_now = self.document.recipe.clone();
        let recipes: Vec<AnyElement> = recipes_for(&inputs)
            .into_iter()
            .enumerate()
            .map(|(index, recipe)| {
                let chosen = recipe_now.as_deref() == Some(recipe.name);
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .w(px(210.0))
                    .p_2()
                    .rounded_md()
                    .bg(rgb(if chosen { SELECTED } else { CONTROL }))
                    .child(Self::act(
                        recipe.name,
                        ("setup-recipe", index),
                        chosen,
                        cx,
                        move |this| this.choose_recipe(recipe),
                    ))
                    .child(Self::note(recipe.help))
                    .into_any_element()
            })
            .collect();

        let input_rows: Vec<AnyElement> = inputs
            .iter()
            .enumerate()
            .map(|(index, input)| {
                let picked = input.clone();
                div()
                    .flex()
                    .gap_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(CONTROL))
                    .child(
                        div()
                            .id(("setup-input", index))
                            .w(px(150.0))
                            .flex_none()
                            .cursor_pointer()
                            .child(div().child(channel_label(input)))
                            .child(Self::note(self.tool_words(input)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select(Row::Input(picked.clone()));
                                cx.notify();
                            })),
                    )
                    .child(div().w(px(420.0)).child(self.routes_block(input, cx)))
                    .into_any_element()
            })
            .collect();

        let cards: Vec<AnyElement> = self
            .top_parts()
            .into_iter()
            .enumerate()
            .map(|(index, part)| self.part_card(index, &part, cx))
            .collect();
        let kinds = PartKind::ALL
            .into_iter()
            .filter(|kind| kind.fits(&inputs))
            .enumerate()
            .map(|(index, kind)| {
                Self::pick(
                    kind.label(),
                    ("setup-add-part", index),
                    false,
                    cx,
                    move |this| {
                        this.add_part_of(kind);
                    },
                )
            })
            .collect();

        div()
            .id("setup")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .overflow_y_scroll()
            .bg(rgb(PANE))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            // Setup covers the canvas: the mouse goes no further.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Setup"),
                    )
                    .child(Self::act("Show the canvas", "setup-close", false, cx, |this| {
                        this.setup_open = false;
                    })),
            )
            .child(Self::note(
                "What the picture is made of. Each line is read from the layers in the tree, and each control changes them, so the tree and Setup always agree.",
            ))
            .child(Self::heading("Recipe"))
            .child(div().flex().flex_wrap().gap_2().children(recipes))
            .child(Self::note(match recipe_now {
                Some(recipe) => format!(
                    "The layers started from the recipe {recipe}. Choosing a recipe replaces the layers that show inputs, and the parts; adjustment layers stay, and Undo puts the old layers back."
                ),
                None => "Choosing a recipe replaces the layers that show inputs, and the parts; adjustment layers stay, and Undo puts the old layers back.".to_string(),
            }))
            .child(Self::heading("Inputs: their tools, and where each goes"))
            .child(div().flex().flex_col().children(input_rows))
            .child(Self::heading("Parts, top first"))
            .child(div().flex().flex_wrap().gap_3().children(cards))
            .child(div().w(px(240.0)).child(self.menu("setup-add-part", "Add part", kinds, cx)))
            .into_any_element()
    }

    /// One part's card: what it shows, its blend and palette, and what can
    /// be done with it.
    fn part_card(&self, index: usize, part: &LayerRecord, cx: &mut Context<Self>) -> AnyElement {
        let part_id = part.id;
        let color = part.color.unwrap_or([1.0; 3]);
        div()
            .w(px(320.0))
            .p_3()
            .rounded_md()
            .bg(rgb(CONTROL))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .id(id(format!("setup-part-{index}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .child(
                        div()
                            .flex_none()
                            .w(px(10.0))
                            .h(px(10.0))
                            .rounded_sm()
                            .bg(swatch(color, 1.0)),
                    )
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(part.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(QUIET))
                            .child(if part.visible { "" } else { "hidden" }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select(Row::Layer(part_id));
                        cx.notify();
                    })),
            )
            .child(Self::note(self.part_members(part_id)))
            .child(Self::field("Blend", self.blend_menu(part, cx)))
            .child(self.palette_block(Some(part_id), cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(Self::act(
                        "Move up",
                        ("setup-part-up", index),
                        false,
                        cx,
                        move |this| {
                            this.document.step_layer(part_id, true);
                        },
                    ))
                    .child(Self::act(
                        "Move down",
                        ("setup-part-down", index),
                        false,
                        cx,
                        move |this| {
                            this.document.step_layer(part_id, false);
                        },
                    ))
                    .child(Self::act(
                        "Remove part",
                        ("setup-part-remove", index),
                        false,
                        cx,
                        move |this| {
                            this.remove_part(part_id);
                        },
                    )),
            )
            .into_any_element()
    }
}
