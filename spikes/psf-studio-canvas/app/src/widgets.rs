//! Controls the panes share, laid out the same way everywhere.
//!
//! - A **field** is one row: a quiet label on the left, its control on the
//!   right.
//! - A **menu** shows the choice in force and opens to the others. It
//!   replaces a row of one button per choice.
//! - A **segmented control** is for two or three choices that are always
//!   in view.
//! - A **section** folds, so a pane shows the controls in use and the
//!   titles of the rest.
//! - **Help** is hidden until a person turns it on in the top bar. A label
//!   is a word or two; the sentence that explains it is help.

use super::panes::{CONTROL, QUIET};
use super::Studio;
use gpui::{div, prelude::*, px, rgb, Context, SharedString};
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether help text is shown.
static HELP: AtomicBool = AtomicBool::new(false);

/// One choice of a menu.
pub(super) struct Pick {
    label: SharedString,
    on: bool,
    element: gpui::Stateful<gpui::Div>,
}

impl Studio {
    pub(super) fn help_shown() -> bool {
        HELP.load(Ordering::Relaxed)
    }

    pub(super) fn toggle_help() {
        HELP.fetch_xor(true, Ordering::Relaxed);
    }

    /// A sentence that explains a control. Hidden until help is turned on.
    pub(super) fn help(text: impl Into<SharedString>) -> Option<gpui::Div> {
        Self::help_shown().then(|| Self::note(text))
    }

    /// A menu's choice. `on` marks the choice in force.
    pub(super) fn pick(
        label: impl Into<SharedString>,
        id: impl Into<gpui::ElementId>,
        on: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self) + 'static,
    ) -> Pick {
        let label = label.into();
        Pick {
            label: label.clone(),
            on,
            element: Self::act(label, id, on, cx, action),
        }
    }

    /// A menu: the choice in force, or `placeholder` when there is none,
    /// and the other choices while it is open. One menu is open at a time,
    /// and choosing closes it.
    pub(super) fn menu(
        &self,
        key: impl Into<String>,
        placeholder: &str,
        picks: Vec<Pick>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let key = key.into();
        let open = self.open_menu.as_deref() == Some(key.as_str());
        let current = picks
            .iter()
            .find(|pick| pick.on)
            .map(|pick| pick.label.clone())
            .unwrap_or_else(|| placeholder.to_string().into());
        let toggled = key.clone();
        let head = div()
            .id(SharedString::from(format!("menu-{key}")))
            .flex()
            .justify_between()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .bg(rgb(CONTROL))
            .child(current)
            .child(
                div()
                    .text_color(rgb(QUIET))
                    .child(if open { "▴" } else { "▾" }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_menu = if this.open_menu.as_deref() == Some(toggled.as_str()) {
                    None
                } else {
                    Some(toggled.clone())
                };
                cx.stop_propagation();
                cx.notify();
            }));
        let mut menu = div().flex_1().flex().flex_col().gap_px().child(head);
        if open {
            menu = menu.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_px()
                    .pl_2()
                    .border_l_2()
                    .border_color(rgb(CONTROL))
                    .children(picks.into_iter().map(|pick| pick.element)),
            );
        }
        menu
    }

    /// One row: a label on the left, its control on the right.
    pub(super) fn field(label: impl Into<SharedString>, control: impl IntoElement) -> gpui::Div {
        div()
            .flex()
            .items_start()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .w(px(64.0))
                    .pt_1()
                    .text_xs()
                    .text_color(rgb(QUIET))
                    .child(label.into()),
            )
            .child(div().flex_1().flex().flex_col().child(control))
    }

    /// The frame of two or three choices that are always in view.
    pub(super) fn segmented() -> gpui::Div {
        div().flex().gap_px().rounded_sm().overflow_hidden()
    }

    /// A titled block that folds. `open` is whether it starts open.
    pub(super) fn section(
        &self,
        key: &'static str,
        title: impl Into<SharedString>,
        open: bool,
        cx: &mut Context<Self>,
        body: impl FnOnce(&Self, &mut Context<Self>) -> gpui::Div,
    ) -> gpui::Div {
        let open = open != self.toggled.contains(key);
        let head = div()
            .id(("section", key.as_ptr() as usize))
            .flex()
            .gap_1()
            .pt_2()
            .text_xs()
            .text_color(rgb(QUIET))
            .cursor_pointer()
            .child(if open { "▾" } else { "▸" })
            .child(title.into())
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.toggled.remove(key) {
                    this.toggled.insert(key);
                }
                cx.stop_propagation();
                cx.notify();
            }));
        let block = div().flex().flex_col().gap_1().child(head);
        if open {
            block.child(body(self, cx))
        } else {
            block
        }
    }
}
