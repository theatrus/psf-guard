//! The right pane: the selected row's controls.
//!
//! The pane starts with the row's name and one line that says what the row
//! applies to: one input, every input, one layer, every layer beneath, or
//! the whole picture. The controls under it change that and nothing else.

use super::panes::{
    amount, amount_in, amount_range, is_drawn, mask_words, new_stretch, operation,
    stretch_settings, ADJUSTMENTS, CONTROL, PANE, QUIET, STRETCHES,
};
use super::{SliderId, Studio, Tool, PANEL_WIDTH};
use crate::image_steps::Accepts;
use crate::studio_model::{
    balance_colors, band_layer, channel_label, flow_operations, hue_of, input_steps, layer_name,
    level_for_zoom, own_step_for_channel, palettes_for, BlendMode, JunctionStatus, LayerRecord,
    LayerSource, MaskRecord, MaskShape, Row, SharedMask, StageKind, StepKind, StepRecord,
};
use gpui::{div, prelude::*, px, rgb, AnyElement, Context};
use uuid::Uuid;

/// The steps a reduced view leaves out, because their result depends on
/// pixel scale. The engine reports these per step; the demo names them.
const FULL_RESOLUTION_ONLY: [&str; 2] = ["Deconvolution", "Noise reduction"];

/// The steps offered on the combined picture.
const FINISH_STEPS: [(&str, Accepts); 3] = [
    ("Curves", Accepts::Display),
    ("Saturation", Accepts::Display),
    ("Local contrast", Accepts::Display),
];

fn column() -> gpui::Div {
    div().flex().flex_col().gap_1()
}

fn wrap() -> gpui::Div {
    div().flex().flex_wrap().gap_1()
}

impl Studio {
    /// The row's name, and what it applies to.
    fn title(
        name: impl Into<gpui::SharedString>,
        applies_to: impl Into<gpui::SharedString>,
    ) -> gpui::Div {
        column().child(name.into()).child(Self::note(applies_to))
    }

    /// The nearest layer beneath `layer` that shows pixels.
    fn layer_beneath(&self, layer: Uuid) -> Option<Uuid> {
        let layers = self.document.layers();
        let at = layers.iter().position(|other| other.id == layer)?;
        layers[..at]
            .iter()
            .rev()
            .find(|other| !matches!(other.source, LayerSource::Adjustment { .. }))
            .map(|other| other.id)
    }

    /// Puts a new adjustment layer right above `layer` and selects it.
    fn add_adjustment_above(&mut self, layer: Option<Uuid>, name: &str) {
        let layers = self.document.layers();
        let above = layer
            .and_then(|id| layers.iter().position(|other| other.id == id))
            .map_or(layers.len(), |at| at + 1);
        let adjustment = LayerRecord::new(
            name,
            LayerSource::Adjustment {
                operation: name.to_string(),
                settings: serde_json::Value::Null,
            },
        );
        let id = adjustment.id;
        if self.document.insert_layer(above, adjustment).is_ok() {
            self.selected = Row::Layer(id);
        }
    }

    /// The buttons that add an adjustment. `add` is given the adjustment's
    /// name.
    fn adjustment_buttons(
        id: &'static str,
        cx: &mut Context<Self>,
        add: impl Fn(&mut Self, &'static str) + Clone + 'static,
    ) -> gpui::Div {
        wrap().children(ADJUSTMENTS.iter().enumerate().map(|(index, (name, _))| {
            let (name, add) = (*name, add.clone());
            Self::act(name, (id, index), false, cx, move |this| add(this, name))
        }))
    }

    /// The buttons that add a step to the Finish stage.
    fn finish_buttons(cx: &mut Context<Self>) -> gpui::Div {
        column()
            .child(Self::heading("Add a step on the whole picture"))
            .child(
                wrap().children(
                    FINISH_STEPS
                        .iter()
                        .enumerate()
                        .map(|(index, (name, accepts))| {
                            let (name, accepts) = (*name, *accepts);
                            Self::act(name, ("add-finish", index), false, cx, move |this| {
                                let step = operation(name, accepts);
                                let id = step.id;
                                if this
                                    .document
                                    .insert_step(StageKind::Finish, usize::MAX, step)
                                    .is_ok()
                                {
                                    this.selected = Row::Finish(id);
                                }
                            })
                        }),
                ),
            )
    }

    /// What every step's controls share: on or off, its amount, whether the
    /// canvas draws it, and its mask.
    fn step_controls(
        &self,
        stage: StageKind,
        step: &StepRecord,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let id = step.id;
        let enabled = step.enabled;
        let name = step.kind.name().to_string();
        let masked = step.mask.is_some();
        let slider = match &step.kind {
            StepKind::Operation {
                operation,
                settings,
                ..
            } if operation == flow_operations::STRETCH => {
                Some(self.stretch_controls(id, settings, cx))
            }
            StepKind::Operation { operation, .. } => amount_range(operation).map(|(low, high)| {
                let value = amount(step);
                self.labelled_slider(
                    format!("{name} {value:.2}"),
                    SliderId::Amount,
                    (value - low) / (high - low),
                    cx,
                )
            }),
            StepKind::Heal { .. } => None,
        };
        column()
            .child(Self::act(
                if enabled { "On" } else { "Off" },
                "step-on",
                enabled,
                cx,
                move |this| {
                    this.document.set_step_enabled(id, !enabled);
                },
            ))
            .children(slider)
            // A step of a layer or of the picture has an opacity. A step of
            // an input is applied in full.
            .when(
                matches!(stage, StageKind::Combine | StageKind::Finish),
                |pane| {
                    pane.child(self.labelled_slider(
                        format!("Opacity {:.0}%", step.opacity * 100.0),
                        SliderId::Opacity,
                        step.opacity,
                        cx,
                    ))
                },
            )
            .when(!is_drawn(stage, step, &self.band_info), |pane| {
                pane.child(Self::note(
                    "The document records this step. The canvas does not draw it yet.",
                ))
            })
            .when_some(mask_words(&self.document, &step.mask), |pane, words| {
                pane.child(Self::heading("Mask"))
                    .child(div().text_xs().child(words))
            })
            .when(masked, |pane| {
                pane.child(Self::act(
                    "Remove mask",
                    "step-unmask",
                    false,
                    cx,
                    move |this| {
                        this.document.clear_step_mask(id);
                    },
                ))
            })
    }

    /// A Stretch step's controls: which of Seiza's stretches it is, and
    /// that stretch's settings. A step that follows another Stretch step
    /// of the same input stretches what that one left.
    fn stretch_controls(
        &self,
        id: Uuid,
        settings: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let kind = settings["model"]["type"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let kinds = STRETCHES
            .iter()
            .enumerate()
            .map(|(index, (name, label))| {
                let (name, settings) = (*name, settings.clone());
                Self::act(
                    *label,
                    ("stretch-kind", index),
                    name == kind,
                    cx,
                    move |this| {
                        let mut settings = settings.clone();
                        if !settings.is_object() {
                            settings = serde_json::json!({ "color_strategy": "linked" });
                        }
                        settings["model"] = new_stretch(name);
                        this.document.set_step_settings(id, settings);
                    },
                )
            })
            .collect::<Vec<_>>();
        let known = STRETCHES.iter().any(|(name, _)| *name == kind);
        let mut block = column()
            .child(Self::heading("Stretch"))
            .child(wrap().children(kinds))
            .when(!known, |block| {
                block.child(Self::note(format!(
                    "This step holds the stretch “{kind}”, which the canvas does not draw."
                )))
            });
        for (index, (key, label, low, high)) in stretch_settings(&kind).iter().enumerate() {
            let value = settings["model"][*key].as_f64().unwrap_or(*low);
            block = block.child(self.labelled_slider(
                format!("{label} {value:.2}"),
                SliderId::StretchParam(index),
                ((value - low) / (high - low)) as f32,
                cx,
            ));
        }
        block.child(Self::act(
            "Add another stretch after this one",
            "stretch-add",
            false,
            cx,
            move |this| {
                let Some((stage, step)) = this.document.step(id) else {
                    return;
                };
                let band = step.band.clone();
                let Some(at) = this
                    .document
                    .stage(stage)
                    .steps
                    .iter()
                    .position(|other| other.id == id)
                else {
                    return;
                };
                let mut next = StepRecord::new(StepKind::Operation {
                    operation: flow_operations::STRETCH.to_string(),
                    accepts: Accepts::Linear,
                    settings: serde_json::json!({
                        "model": new_stretch("ghs"),
                        "color_strategy": "linked",
                    }),
                });
                next.band = band.clone();
                let new = next.id;
                if this.document.insert_step(stage, at + 1, next).is_ok()
                    && let Row::InputStep { channel, .. } = &this.selected
                {
                    this.selected = Row::InputStep {
                        channel: channel.clone(),
                        step: new,
                    };
                }
            },
        ))
        .child(Self::note(
            "Stretches of one input apply in order. Each is resolved from what the one before it left.",
        ))
    }

    // ---- one inspector per kind of row ----------------------------------

    fn inspect_input(&self, channel: &str, cx: &mut Context<Self>) -> AnyElement {
        let label = channel_label(channel);
        column()
            .child(Self::title(
                format!("{label} input"),
                "The stack as PSF Guard made it, before any step.",
            ))
            .child(self.tools_block(channel, cx))
            .child(self.zoom_notes(channel))
            .into_any_element()
    }

    fn inspect_input_step(&self, channel: &str, id: Uuid, cx: &mut Context<Self>) -> AnyElement {
        let Some((stage, step)) = self.document.step(id) else {
            return Self::note("This step is gone.").into_any_element();
        };
        let label = channel_label(channel);
        let shared = step.band.is_none();
        let applies_to = if shared {
            "Applies to every input that has no step of its own of this kind.".to_string()
        } else {
            format!("Applies to {label} only.")
        };
        let is_operation = matches!(step.kind, StepKind::Operation { .. });
        let own = (shared && is_operation).then(|| {
            let channel = channel.to_string();
            Self::act(
                format!("Give {label} its own settings"),
                "own-settings",
                false,
                cx,
                move |this| {
                    if let Some(own) = own_step_for_channel(&mut this.document, id, &channel) {
                        this.selected = Row::InputStep {
                            channel: channel.clone(),
                            step: own,
                        };
                    }
                },
            )
        });
        let channel = channel.to_string();
        column()
            .child(Self::title(step.kind.name().to_string(), applies_to))
            .child(self.step_controls(stage, step, cx))
            .when(
                step.kind.name() == flow_operations::RC_ASTRO_INPUT,
                |pane| pane.child(self.tools_block(&channel, cx)),
            )
            .children(own)
            .child(Self::act(
                if shared {
                    "Remove from every input"
                } else {
                    "Remove"
                },
                "step-remove",
                false,
                cx,
                move |this| {
                    this.document.remove_step(id);
                    this.selected = Row::Input(channel.clone());
                },
            ))
            .into_any_element()
    }

    fn inspect_band_layer(&self, layer: &LayerRecord, cx: &mut Context<Self>) -> AnyElement {
        let id = layer.id;
        let LayerSource::Band { band } = &layer.source else {
            return div().into_any_element();
        };
        let (hue, saturation, _) = hue_of(layer.color.unwrap_or([1.0; 3]));
        let colored = layer.blend != BlendMode::Luminance;
        let inputs = self
            .document
            .channels()
            .into_iter()
            .enumerate()
            .map(|(index, channel)| {
                let on = channel == *band;
                Self::act(
                    channel_label(&channel),
                    ("layer-input", index),
                    on,
                    cx,
                    move |this| {
                        if let Some(layer) = this.document.layer_mut(id) {
                            layer.name =
                                layer_name(&channel, layer.color.unwrap_or([1.0; 3]), layer.blend);
                            layer.source = LayerSource::Band {
                                band: channel.clone(),
                            };
                        }
                    },
                )
            })
            .collect::<Vec<_>>();
        let copy = layer.clone();
        column()
            .child(Self::title(
                layer.name.clone(),
                "A layer of the picture: one input, in one color.",
            ))
            .child(Self::heading("Input shown"))
            .child(wrap().children(inputs))
            .when(colored, |pane| {
                pane.child(Self::heading("Color"))
                    .child(self.labelled_slider(
                        format!("Hue {hue:.0}°"),
                        SliderId::Hue,
                        hue / 360.0,
                        cx,
                    ))
                    .child(self.labelled_slider(
                        format!("Saturation {:.0}%", saturation * 100.0),
                        SliderId::Saturation,
                        saturation,
                        cx,
                    ))
            })
            .child(self.labelled_slider(
                format!("Opacity {:.0}%", layer.opacity * 100.0),
                SliderId::Opacity,
                layer.opacity,
                cx,
            ))
            .child(Self::blend_block(layer, cx))
            .child(self.shows_block("This layer alone", cx))
            .child(self.mask_block(layer, cx))
            .child(self.shared_mask_block(layer, cx))
            .child(Self::heading("Clipping"))
            .child(Self::act(
                if layer.clipped {
                    "Clipped to the layer beneath"
                } else {
                    "Not clipped"
                },
                "clipped",
                layer.clipped,
                cx,
                move |this| {
                    if let Some(layer) = this.document.layer_mut(id) {
                        layer.clipped = !layer.clipped;
                    }
                },
            ))
            .child(Self::note(
                "A clipped layer shows only inside the mask of the layer beneath it, and not at all while that layer is hidden.",
            ))
            .child(Self::heading("Add an adjustment to this layer only"))
            .child(Self::adjustment_buttons(
                "adjust-own",
                cx,
                move |this, name| {
                    let step = operation(name, Accepts::Display);
                    let step_id = step.id;
                    if this.document.insert_layer_step(id, step).is_ok() {
                        this.selected = Row::LayerStep {
                            layer: id,
                            step: step_id,
                        };
                    }
                },
            ))
            .child(Self::heading(
                "Add an adjustment above, for every layer beneath",
            ))
            .child(Self::adjustment_buttons(
                "adjust-above",
                cx,
                move |this, name| this.add_adjustment_above(Some(id), name),
            ))
            .child(Self::heading("Layer"))
            .child(
                wrap()
                    .child(Self::act(
                        "Duplicate",
                        "layer-copy",
                        false,
                        cx,
                        move |this| {
                            let mut copy = copy.clone();
                            copy.id = Uuid::new_v4();
                            for step in &mut copy.steps {
                                step.id = Uuid::new_v4();
                            }
                            let new = copy.id;
                            let above = this
                                .document
                                .layers()
                                .iter()
                                .position(|layer| layer.id == id)
                                .map_or(usize::MAX, |at| at + 1);
                            if this.document.insert_layer(above, copy).is_ok() {
                                this.selected = Row::Layer(new);
                            }
                        },
                    ))
                    .when(layer.group.is_none(), |row| {
                        row.child(Self::act(
                            "Put in a new group",
                            "layer-group",
                            false,
                            cx,
                            move |this| {
                                if let Ok(group) = this.document.group_layer(id) {
                                    this.selected = Row::Layer(group);
                                }
                            },
                        ))
                    })
                    .child(Self::act(
                        "Remove",
                        "layer-remove",
                        false,
                        cx,
                        move |this| {
                            this.document.remove_layer(id);
                            this.selected = Row::Combined;
                        },
                    )),
            )
            .into_any_element()
    }

    /// The blend modes, with the layer's own marked.
    fn blend_block(layer: &LayerRecord, cx: &mut Context<Self>) -> gpui::Div {
        let id = layer.id;
        let blends = BlendMode::ALL
            .into_iter()
            .enumerate()
            .map(|(index, mode)| {
                Self::act(
                    mode.to_string(),
                    ("blend", index),
                    layer.blend == mode,
                    cx,
                    move |this| {
                        if let Some(layer) = this.document.layer_mut(id) {
                            layer.blend = mode;
                            if let LayerSource::Band { band } = &layer.source {
                                layer.name =
                                    layer_name(band, layer.color.unwrap_or([1.0; 3]), layer.blend);
                            }
                        }
                    },
                )
            })
            .collect::<Vec<_>>();
        column()
            .child(Self::heading("Blend with the layers beneath"))
            .child(wrap().children(blends))
    }

    /// Whether the canvas shows the stack up to the selected layer, or the
    /// selected layer alone.
    fn shows_block(&self, alone_label: &'static str, cx: &mut Context<Self>) -> gpui::Div {
        let alone = self.alone;
        column().child(Self::heading("Canvas shows")).child(
            wrap()
                .child(Self::act(
                    "Layers up to this one",
                    "show-stack",
                    !alone,
                    cx,
                    |this| this.alone = false,
                ))
                .child(Self::act(alone_label, "show-alone", alone, cx, |this| {
                    this.alone = true
                })),
        )
    }

    fn inspect_group(&self, layer: &LayerRecord, cx: &mut Context<Self>) -> AnyElement {
        let id = layer.id;
        let members = self.document.group_members(id).count();
        column()
            .child(Self::title(
                layer.name.clone(),
                format!(
                    "A group of {}. Its layers blend together on their own, then with the layers beneath as one layer.",
                    super::panes::counted(members, "layer")
                ),
            ))
            .child(Self::note(
                "Drag a layer onto the group, or into the gap under one of its layers, to add it. Drag it out to remove it.",
            ))
            .child(self.labelled_slider(
                format!("Opacity {:.0}%", layer.opacity * 100.0),
                SliderId::Opacity,
                layer.opacity,
                cx,
            ))
            .child(Self::blend_block(layer, cx))
            .child(self.shows_block("This group alone", cx))
            .child(self.mask_block(layer, cx))
            .child(self.shared_mask_block(layer, cx))
            .child(Self::heading("Add an adjustment to this group only"))
            .child(Self::adjustment_buttons(
                "adjust-own",
                cx,
                move |this, name| {
                    let step = operation(name, Accepts::Display);
                    let step_id = step.id;
                    if this.document.insert_layer_step(id, step).is_ok() {
                        this.selected = Row::LayerStep {
                            layer: id,
                            step: step_id,
                        };
                    }
                },
            ))
            .child(Self::heading("Add an adjustment above, for every layer beneath"))
            .child(Self::adjustment_buttons(
                "adjust-above",
                cx,
                move |this, name| this.add_adjustment_above(Some(id), name),
            ))
            .child(Self::heading("Group"))
            .child(Self::act("Ungroup", "ungroup", false, cx, move |this| {
                this.document.ungroup(id);
                this.selected = Row::Combined;
            }))
            .child(Self::note(
                "Ungrouping keeps the layers where they are. The group's own mask and adjustments go with it.",
            ))
            .into_any_element()
    }

    /// Which shared mask a layer is behind, and whether it shows where the
    /// mask is strong or weak. The mask itself is changed on its own row.
    fn shared_mask_block(&self, layer: &LayerRecord, cx: &mut Context<Self>) -> gpui::Div {
        let id = layer.id;
        let used = match &layer.mask {
            Some(MaskRecord::Shared { mask, inverted }) => Some((*mask, *inverted)),
            _ => None,
        };
        let painted = matches!(layer.mask, Some(MaskRecord::Painted(_)));
        let masks = self
            .document
            .masks()
            .iter()
            .enumerate()
            .map(|(index, mask)| {
                let mask_id = mask.id;
                let on = used.is_some_and(|(used, _)| used == mask_id);
                Self::act(
                    mask.name.clone(),
                    ("layer-mask", index),
                    on,
                    cx,
                    move |this| {
                        if let Some(layer) = this.document.layer_mut(id) {
                            layer.mask = (!on).then_some(MaskRecord::Shared {
                                mask: mask_id,
                                inverted: false,
                            });
                        }
                    },
                )
            })
            .collect::<Vec<_>>();
        let first_input = self.document.channels().into_iter().next();
        column()
            .child(Self::heading("Shared mask"))
            .child(wrap().children(masks).child(Self::act(
                "New mask",
                "layer-mask-new",
                false,
                cx,
                move |this| {
                    let Some(input) = first_input.clone() else {
                        return;
                    };
                    let mask = this.document.add_mask(SharedMask::new(vec![input]));
                    if let Some(layer) = this.document.layer_mut(id) {
                        layer.mask = Some(MaskRecord::Shared {
                            mask,
                            inverted: false,
                        });
                    }
                    this.selected = Row::Mask(mask);
                },
            )))
            .child(Self::note(
                "A shared mask follows the signal of one or two inputs. Several layers can be behind the same mask, or its inverse.",
            ))
            .when(painted, |pane| {
                pane.child(Self::note("Choosing a shared mask replaces the painted mask."))
            })
            .when_some(used, |pane, (mask, inverted)| {
                let set = move |this: &mut Self, inverted: bool| {
                    if let Some(layer) = this.document.layer_mut(id) {
                        layer.mask = Some(MaskRecord::Shared { mask, inverted });
                    }
                };
                pane.child(
                    wrap()
                        .child(Self::act(
                            "Where it is strong",
                            "mask-strong",
                            !inverted,
                            cx,
                            move |this| set(this, false),
                        ))
                        .child(Self::act(
                            "Where it is weak",
                            "mask-weak",
                            inverted,
                            cx,
                            move |this| set(this, true),
                        ))
                        .child(Self::act("Edit the mask", "mask-edit", false, cx, move |this| {
                            this.selected = Row::Mask(mask)
                        })),
                )
            })
    }

    /// A shared mask's controls: what it is computed from, how far it
    /// reaches, and its touch-ups.
    fn inspect_mask(&self, id: Uuid, cx: &mut Context<Self>) -> AnyElement {
        let Some(mask) = self.document.mask(id) else {
            return Self::note("This mask is gone.").into_any_element();
        };
        let users: Vec<String> = self
            .document
            .layers_using_mask(id)
            .map(|layer| match &layer.mask {
                Some(MaskRecord::Shared { inverted: true, .. }) => {
                    format!("{} (where weak)", layer.name)
                }
                _ => layer.name.clone(),
            })
            .collect();
        let chosen = mask.inputs.clone();
        let inputs = self
            .document
            .channels()
            .into_iter()
            .enumerate()
            .map(|(index, channel)| {
                let on = chosen.contains(&channel);
                let chosen = chosen.clone();
                Self::act(
                    channel_label(&channel),
                    ("mask-input", index),
                    on,
                    cx,
                    move |this| {
                        let mut inputs = chosen.clone();
                        if on && inputs.len() > 1 {
                            inputs.retain(|input| *input != channel);
                        } else if !on && inputs.len() < 2 {
                            inputs.push(channel.clone());
                        }
                        if let Some(mask) = this.document.mask_mut(id) {
                            mask.name = SharedMask::name_for(&inputs);
                            mask.inputs = inputs;
                            // A mask a person changed is theirs: a palette
                            // no longer remakes it.
                            mask.slots.clear();
                        }
                    },
                )
            })
            .collect::<Vec<_>>();
        let shape = mask.shape;
        let reach = mask.reach;
        let touch_ups = mask.touch_ups.len();
        let smoothing = mask.smoothing;
        let removes = self.brush_removes;
        let set_shape = move |this: &mut Self, shape| {
            if let Some(mask) = this.document.mask_mut(id) {
                mask.shape = shape;
            }
        };
        column()
            .child(Self::title(
                format!("Mask: {}", mask.name),
                "Computed from inputs at each pixel, smoothed, then touched up by hand. In the mask, white is where a layer behind it shows.",
            ))
            .child(Self::heading("Canvas shows"))
            .child(
                wrap()
                    .child(Self::act(
                        "The mask",
                        "mask-view-mask",
                        !self.mask_shows_picture,
                        cx,
                        |this| this.mask_shows_picture = false,
                    ))
                    .child(Self::act(
                        "The final picture",
                        "mask-view-picture",
                        self.mask_shows_picture,
                        cx,
                        |this| this.mask_shows_picture = true,
                    )),
            )
            .child(Self::note(
                "The Input and picture split shows the mask and the final picture side by side.",
            ))
            .child(Self::heading("Layers behind this mask"))
            .child(div().text_xs().child(if users.is_empty() {
                "None.".to_string()
            } else {
                users.join(", ")
            }))
            .child(Self::heading("Inputs, multiplied"))
            .child(wrap().children(inputs))
            .child(Self::note(
                "Each input is read as its own Stretch step leaves it, so the mask follows the inputs.",
            ))
            .child(Self::heading("Shape"))
            .child(
                wrap()
                    .child(Self::act(
                        "Lift faint signal",
                        "mask-lift",
                        shape == MaskShape::PowerOfInverse,
                        cx,
                        move |this| set_shape(this, MaskShape::PowerOfInverse),
                    ))
                    .child(Self::act(
                        "As stretched",
                        "mask-plain",
                        shape == MaskShape::Value,
                        cx,
                        move |this| set_shape(this, MaskShape::Value),
                    )),
            )
            .child(self.labelled_slider(
                format!("Reach {reach:.2}"),
                SliderId::Reach,
                (reach.log2() + 2.0) / 4.0,
                cx,
            ))
            .child(Self::note(
                "Below 1 the mask reaches further into faint signal. Above 1 it keeps to strong signal.",
            ))
            .child(self.labelled_slider(
                format!("Smoothing {smoothing:.1} px"),
                SliderId::Smoothing,
                smoothing / 32.0,
                cx,
            ))
            .child(Self::note(
                "Blurs the mask so the inputs' noise does not become speckle in the layers behind it. 0 leaves the mask as computed.",
            ))
            .child(Self::heading("Touch-ups"))
            .child(div().text_xs().child(format!(
                "{}. Paint with the mask brush.",
                super::panes::counted(touch_ups, "stroke")
            )))
            .child(
                wrap()
                    .child(Self::act("Brush adds", "brush-adds", !removes, cx, |this| {
                        this.brush_removes = false
                    }))
                    .child(Self::act(
                        "Brush takes away",
                        "brush-removes",
                        removes,
                        cx,
                        |this| this.brush_removes = true,
                    ))
                    .when(touch_ups > 0, |row| {
                        row.child(Self::act(
                            "Clear touch-ups",
                            "mask-clear",
                            false,
                            cx,
                            move |this| {
                                if let Some(mask) = this.document.mask_mut(id) {
                                    mask.touch_ups.clear();
                                }
                            },
                        ))
                    }),
            )
            .child(Self::act("Remove mask", "mask-remove", false, cx, move |this| {
                this.document.remove_mask(id);
                this.selected = Row::Combined;
            }))
            .child(Self::note(
                "Removing the mask leaves the layers behind it showing everywhere.",
            ))
            .into_any_element()
    }

    /// The tone of every layer that shows an input, in one list: a palette
    /// is a starting point, and each layer's hue and saturation are set
    /// here without selecting the layers one by one.
    fn tones_block(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut block = column()
            .child(Self::heading("Color tones"))
            .child(Self::note(
                "Each layer's hue and saturation. A palette sets them to start with.",
            ));
        // Top layer first, as the tree lists them.
        for (index, layer) in self.document.layers().iter().enumerate().rev() {
            let Some(color) = layer.color.filter(|_| layer.source.shows_pixels()) else {
                continue;
            };
            if layer.blend == BlendMode::Luminance || index >= studio_render_spike::MAX_LAYERS {
                continue;
            }
            let (hue, saturation, _) = hue_of(color);
            block = block
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .pt_1()
                        .child(
                            div()
                                .flex_none()
                                .w(px(14.0))
                                .h(px(14.0))
                                .rounded_sm()
                                .bg(super::panes::swatch(color, 1.0)),
                        )
                        .child(div().flex_1().text_xs().child(layer.name.clone()))
                        .child(Self::note(format!(
                            "{hue:.0}° · {:.0}%",
                            saturation * 100.0
                        ))),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(Self::note("Hue"))
                        .child(self.slider(SliderId::ToneHue(index), hue / 360.0, cx))
                        .child(Self::note("Saturation"))
                        .child(self.slider(SliderId::ToneSaturation(index), saturation, cx)),
                );
        }
        block
    }

    /// For the document's palette: which input each of its slots shows.
    fn palette_inputs_block(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let used = self.document.palette.clone()?;
        let channels = self.document.channels();
        let mut block = column()
            .child(Self::heading(format!("Inputs of {}", used.name)))
            .child(Self::note(
                "Giving a slot another input remakes the layers and the masks that use it. Their colors, adjustments and mask touch-ups are kept.",
            ));
        for (row, (slot, shown)) in used.inputs.into_iter().enumerate() {
            let choices = channels
                .iter()
                .enumerate()
                .map(|(index, channel)| {
                    let (slot, channel) = (slot.clone(), channel.clone());
                    Self::act(
                        channel_label(&channel),
                        ("slot-input", row * 16 + index),
                        channel == shown,
                        cx,
                        move |this| {
                            this.document.set_palette_input(&slot, &channel);
                        },
                    )
                })
                .collect::<Vec<_>>();
            block = block.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(64.0))
                            .text_xs()
                            .child(format!("{} slot", channel_label(&slot))),
                    )
                    .child(wrap().children(choices)),
            );
        }
        Some(block)
    }

    /// A layer's mask, and the button that removes it.
    fn mask_block(&self, layer: &LayerRecord, cx: &mut Context<Self>) -> gpui::Div {
        let id = layer.id;
        column()
            .child(Self::heading("Mask"))
            .child(
                div()
                    .text_xs()
                    .child(mask_words(&self.document, &layer.mask).unwrap_or_else(|| {
                        "None: it applies everywhere. Paint one with the mask brush.".to_string()
                    })),
            )
            .when(layer.mask.is_some(), |pane| {
                pane.child(Self::act(
                    "Remove mask",
                    "layer-unmask",
                    false,
                    cx,
                    move |this| {
                        if let Some(layer) = this.document.layer_mut(id) {
                            layer.mask = None;
                        }
                    },
                ))
            })
    }

    fn inspect_adjustment_layer(&self, layer: &LayerRecord, cx: &mut Context<Self>) -> AnyElement {
        let id = layer.id;
        let LayerSource::Adjustment {
            operation,
            settings,
        } = &layer.source
        else {
            return div().into_any_element();
        };
        let value = amount_in(operation, settings);
        let slider = amount_range(operation).map(|(low, high)| {
            self.labelled_slider(
                format!("{operation} {value:.2}"),
                SliderId::LayerAmount,
                (value - low) / (high - low),
                cx,
            )
        });
        let beneath = self.layer_beneath(id);
        let beneath_name = beneath
            .and_then(|id| self.layer(id))
            .map(|layer| layer.name.clone());
        column()
            .child(Self::title(
                layer.name.clone(),
                "An adjustment of every layer beneath it.",
            ))
            .children(slider)
            .child(self.labelled_slider(
                format!("Opacity {:.0}%", layer.opacity * 100.0),
                SliderId::Opacity,
                layer.opacity,
                cx,
            ))
            .child(Self::heading("Applies to"))
            .child(
                wrap()
                    .child(Self::button("Every layer beneath", "scope-stack", true))
                    .when_some(beneath.zip(beneath_name), |row, (target, name)| {
                        row.child(Self::act(
                            format!("{name} only"),
                            "scope-own",
                            false,
                            cx,
                            move |this| {
                                if let Ok(step) = this.document.attach_adjustment(id, target) {
                                    this.selected = Row::LayerStep {
                                        layer: target,
                                        step,
                                    };
                                }
                            },
                        ))
                    }),
            )
            .child(Self::note(
                "Dragging does the same: onto a layer for that layer only, between layers for every layer beneath.",
            ))
            .child(self.mask_block(layer, cx))
            .child(self.shared_mask_block(layer, cx))
            .child(Self::act("Remove", "layer-remove", false, cx, move |this| {
                this.document.remove_layer(id);
                this.selected = Row::Combined;
            }))
            .into_any_element()
    }

    fn inspect_layer_step(&self, layer: Uuid, id: Uuid, cx: &mut Context<Self>) -> AnyElement {
        let (Some(owner), Some((stage, step))) = (self.layer(layer), self.document.step(id)) else {
            return Self::note("This edit is gone.").into_any_element();
        };
        let is_adjustment = matches!(step.kind, StepKind::Operation { .. });
        column()
            .child(Self::title(
                step.kind.name().to_string(),
                format!("An edit of {} only.", owner.name),
            ))
            .child(self.step_controls(stage, step, cx))
            .when(is_adjustment, |pane| {
                pane.child(Self::heading("Applies to"))
                    .child(
                        wrap()
                            .child(Self::button(
                                format!("{} only", owner.name),
                                "scope-own",
                                true,
                            ))
                            .child(Self::act(
                                "Every layer beneath",
                                "scope-stack",
                                false,
                                cx,
                                move |this| {
                                    if let Ok(new) = this.document.detach_adjustment(id) {
                                        this.selected = Row::Layer(new);
                                    }
                                },
                            )),
                    )
                    .child(Self::note(
                        "Paint with the mask brush to limit where this adjustment applies.",
                    ))
            })
            .child(Self::act("Remove", "step-remove", false, cx, move |this| {
                this.document.remove_step(id);
                this.selected = Row::Layer(layer);
            }))
            .into_any_element()
    }

    fn inspect_combined(&self, cx: &mut Context<Self>) -> AnyElement {
        let channels = self.document.channels();
        let palettes = palettes_for(&channels)
            .into_iter()
            .enumerate()
            .map(|(index, palette)| {
                Self::act(
                    palette.name,
                    ("palette", index),
                    self.document.uses_palette(palette),
                    cx,
                    move |this| this.document.apply_palette(palette),
                )
            })
            .collect::<Vec<_>>();
        let no_palette = palettes.is_empty();
        let inputs = channels
            .into_iter()
            .enumerate()
            .map(|(index, channel)| {
                Self::act(
                    channel_label(&channel),
                    ("add-layer", index),
                    false,
                    cx,
                    move |this| {
                        let layer = band_layer(&channel, 0.0, 0.0, BlendMode::Add);
                        let id = layer.id;
                        if this.document.insert_layer(usize::MAX, layer).is_ok() {
                            this.selected = Row::Layer(id);
                        }
                    },
                )
            })
            .collect::<Vec<_>>();
        column()
            .child(Self::title(
                "Combined",
                "Every layer, blended bottom to top, before the Finish steps.",
            ))
            .child(Self::heading("Palette"))
            .child(wrap().children(palettes))
            .when(no_palette, |pane| {
                pane.child(Self::note("No palette fits these inputs."))
            })
            .child(Self::note(
                "A palette replaces the layers that show an input. After that, each layer's color is yours to change.",
            ))
            .children(self.palette_inputs_block(cx))
            .child(self.tones_block(cx))
            .child(Self::act(
                "Balance the layers' colors",
                "balance",
                false,
                cx,
                |this| {
                    let mut layers = this.document.layers().to_vec();
                    balance_colors(&mut layers);
                    for balanced in layers {
                        if let Some(layer) = this.document.layer_mut(balanced.id) {
                            layer.color = balanced.color;
                        }
                    }
                },
            ))
            .child(Self::note(
                "Scales the added layers together so their sum is full scale.",
            ))
            .child(Self::heading("Add a layer that shows an input"))
            .child(wrap().children(inputs))
            .child(Self::heading("Add an adjustment on top, for every layer"))
            .child(Self::adjustment_buttons(
                "adjust-top",
                cx,
                |this, name| this.add_adjustment_above(None, name),
            ))
            .child(Self::finish_buttons(cx))
            .into_any_element()
    }

    fn inspect_finish(&self, id: Uuid, cx: &mut Context<Self>) -> AnyElement {
        let Some((stage, step)) = self.document.step(id) else {
            return Self::note("This step is gone.").into_any_element();
        };
        column()
            .child(Self::title(
                step.kind.name().to_string(),
                "A step on the whole picture, after the layers are combined.",
            ))
            .child(self.step_controls(stage, step, cx))
            .child(Self::note(
                "Paint with the mask brush to limit where this step applies.",
            ))
            .child(Self::act("Remove", "step-remove", false, cx, move |this| {
                this.document.remove_step(id);
                this.selected = Row::Combined;
            }))
            .child(Self::finish_buttons(cx))
            .into_any_element()
    }

    fn inspect_final(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .child(Self::title("Final picture", "Every step of the document."))
            .child(Self::finish_buttons(cx))
            .child(self.junction_block())
            .into_any_element()
    }

    // ---- blocks under the row's controls --------------------------------

    /// Which of an input's steps the view leaves out at this zoom.
    fn zoom_notes(&self, channel: &str) -> gpui::Div {
        let zoom = self.canvas.read(|shared| shared.canvas.view.zoom);
        let level = level_for_zoom(zoom, self.levels);
        let omitted: Vec<&str> = input_steps(&self.document, channel)
            .iter()
            .filter_map(|row| self.document.step(row.id))
            .map(|(_, step)| step.kind.name())
            .filter(|name| level > 0 && FULL_RESOLUTION_ONLY.contains(name))
            .collect();
        let note = if omitted.is_empty() {
            "The view leaves out no step for its zoom.".to_string()
        } else {
            format!(
                "At this zoom the view leaves out: {}. Zoom to 100% or use a crop preview to see them.",
                omitted.join(", ")
            )
        };
        column()
            .child(Self::heading("Zoom"))
            .child(
                div()
                    .text_xs()
                    .child(format!("Zoom {:.0}%, pyramid level {level}", zoom * 100.0)),
            )
            .child(div().text_xs().child(note))
    }

    fn junction_block(&self) -> gpui::Div {
        // The key for everything before the junction comes from the engine's
        // cache keys, which the canvas does not use yet.
        let status = match self.document.stage(StageKind::Finish).junction.status("") {
            JunctionStatus::Computed => "Export uses Studio's own result.",
            JunctionStatus::WaitingForEdit => "A file is open in an external editor.",
            JunctionStatus::Edited => "Export uses the file edited outside Studio.",
            JunctionStatus::EditedFromOlderResult => {
                "The edited file was made from an older result."
            }
            JunctionStatus::WaitingOnOlderResult => {
                "The file in the external editor was made from an older result."
            }
        };
        column()
            .child(Self::heading("External editor"))
            .child(div().text_xs().child(status))
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(CONTROL))
                    .text_color(rgb(QUIET))
                    .child("Open in external editor"),
            )
            .child(Self::note(
                "Disabled: Studio cannot write the file for an editor yet.",
            ))
    }

    /// Where a heal stroke goes, for the selected row.
    fn heal_target(&self) -> String {
        match &self.selected {
            Row::Input(channel) | Row::InputStep { channel, .. } => {
                format!("the {} input only", channel_label(channel))
            }
            Row::Layer(layer) | Row::LayerStep { layer, .. } => self
                .layer(*layer)
                .filter(|layer| layer.source.shows_pixels())
                .map_or_else(
                    || "nothing: an adjustment or a group has no pixels to heal".to_string(),
                    |layer| format!("the layer {} only", layer.name),
                ),
            Row::Combined | Row::Finish(_) | Row::Final | Row::Mask(_) => {
                "the whole picture".to_string()
            }
        }
    }

    fn tools(&self, cx: &mut Context<Self>) -> gpui::Div {
        let tool = |tool: Tool, label: &'static str, cx: &mut Context<Self>| {
            Self::act(label, label, self.tool == tool, cx, move |this| {
                this.tool = tool;
                // A person who picks the mask brush wants to see the mask.
                if tool == Tool::Brush {
                    this.mask_shown = true;
                }
            })
        };
        let can_mask = self.can_mask();
        let help = match self.tool {
            Tool::Pan => "Drag to move the view. Every pane moves together.".to_string(),
            Tool::Brush => "Drag to paint the selected row's mask.".to_string(),
            Tool::Heal => format!(
                "Drag over a blemish. Each stroke becomes a Heal step of {}.",
                self.heal_target()
            ),
        };
        column()
            .child(Self::heading("Tools"))
            .child(
                wrap()
                    .child(tool(Tool::Pan, "Pan", cx))
                    .when(can_mask, |row| {
                        row.child(tool(Tool::Brush, "Mask brush", cx))
                    })
                    .child(tool(Tool::Heal, "Heal", cx)),
            )
            .child(Self::note(help))
            .when(can_mask || self.has_strokes(), |pane| {
                let shown = self.mask_shown;
                pane.child(Self::act(
                    if shown {
                        "Mask overlay: shown"
                    } else {
                        "Mask overlay: hidden"
                    },
                    "mask-shown",
                    shown,
                    cx,
                    move |this| this.mask_shown = !shown,
                ))
                .child(Self::note(
                    "The overlay marks where the selected row's mask is painted. Hiding it does not change the mask.",
                ))
            })
            .when(!can_mask, |pane| {
                pane.child(Self::note(
                    "Mask brush: select a layer, a layer's adjustment or a Finish step.",
                ))
            })
    }

    // ---- the pane -------------------------------------------------------

    pub(super) fn inspector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let controls = match self.selected.clone() {
            Row::Input(channel) => self.inspect_input(&channel, cx),
            Row::InputStep { channel, step } => self.inspect_input_step(&channel, step, cx),
            Row::Layer(id) => match self.layer(id) {
                Some(layer) if matches!(layer.source, LayerSource::Adjustment { .. }) => {
                    self.inspect_adjustment_layer(layer, cx)
                }
                Some(layer) if layer.source == LayerSource::Group => self.inspect_group(layer, cx),
                Some(layer) => self.inspect_band_layer(layer, cx),
                None => Self::note("This layer is gone.").into_any_element(),
            },
            Row::Mask(mask) => self.inspect_mask(mask, cx),
            Row::LayerStep { layer, step } => self.inspect_layer_step(layer, step, cx),
            Row::Combined => self.inspect_combined(cx),
            Row::Finish(step) => self.inspect_finish(step, cx),
            Row::Final => self.inspect_final(cx),
        };
        div()
            .id("inspector")
            .flex_none()
            .w(px(PANEL_WIDTH))
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(PANE))
            .child(controls)
            .child(self.tools(cx))
            .child(div().flex_1())
            .child(Self::heading("Canvas"))
            .children(
                self.readout()
                    .into_iter()
                    .map(|line| div().text_xs().child(line)),
            )
    }
}
