//! The right pane: the selected row's controls.
//!
//! The pane starts with the row's name and one line that says what the row
//! applies to: one input, every input, one layer, every layer beneath, or
//! the whole picture. The controls under it change that and nothing else.

use super::panes::{
    amount, amount_in, amount_range, is_drawn, mask_words, new_stretch, operation,
    output_stretch_step, stretch_settings, ADJUSTMENTS, MAX_SKY_FLOOR, PANE, SKY_FLOOR, STRETCHES,
};
use super::{SliderId, Studio, Tool, PANEL_WIDTH};
use crate::image_steps::Accepts;
use crate::studio_model::{
    balance_colors, channel_label, flow_operations, hue_of, layer_name, level_for_zoom,
    own_step_for_channel, BlendMode, CropRecord, JunctionStatus, LayerRecord, LayerSource,
    MaskRecord, MaskShape, Row, SharedMask, StageKind, StepKind, StepRecord,
};
use gpui::{div, prelude::*, px, rgb, AnyElement, Context};
use uuid::Uuid;

/// The steps offered on the combined picture.
const FINISH_STEPS: [(&str, Accepts); 5] = [
    (flow_operations::OUTPUT_STRETCH, Accepts::Display),
    (crate::tone_step::OPERATION, Accepts::Display),
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
        column().child(name.into()).children(Self::help(applies_to))
    }

    /// The nearest layer beneath `layer` that shows pixels.
    fn layer_beneath(&self, layer: Uuid) -> Option<Uuid> {
        let layers = self.document.layers();
        let at = layers.iter().position(|other| other.id == layer)?;
        layers[..at]
            .iter()
            .rev()
            .find(|other| !other.source.is_adjustment())
            .map(|other| other.id)
    }

    /// Puts a new adjustment layer right above `layer` and selects it.
    fn add_adjustment_above(&mut self, layer: Option<Uuid>, name: &str) {
        let layers = self.document.layers();
        let above = layer
            .and_then(|id| layers.iter().position(|other| other.id == id))
            .map_or(layers.len(), |at| at + 1);
        let adjustment = crate::studio_model::adjustment_layer(operation(name, Accepts::Display));
        let id = adjustment.id;
        if self.document.insert_layer(above, adjustment).is_ok() {
            self.selected = Row::Layer(id);
        }
    }

    /// The menu that adds an adjustment. `add` is given the adjustment's
    /// name.
    fn adjustment_menu(
        &self,
        id: &'static str,
        label: &str,
        cx: &mut Context<Self>,
        add: impl Fn(&mut Self, &'static str) + Clone + 'static,
    ) -> gpui::Div {
        let picks = ADJUSTMENTS
            .iter()
            .enumerate()
            .map(|(index, (name, _))| {
                let (name, add) = (*name, add.clone());
                Self::pick(name, (id, index), false, cx, move |this| add(this, name))
            })
            .collect();
        self.menu(id, label, picks, cx)
    }

    /// The menus that add a step or a tool on top of every layer, as an
    /// adjustment layer of its own.
    fn finish_menu(&self, cx: &mut Context<Self>) -> gpui::Div {
        let picks = FINISH_STEPS
            .iter()
            .enumerate()
            .map(|(index, (name, accepts))| {
                let (name, accepts) = (*name, *accepts);
                Self::pick(name, ("add-finish", index), false, cx, move |this| {
                    let step = if name == flow_operations::OUTPUT_STRETCH {
                        output_stretch_step()
                    } else if name == crate::tone_step::OPERATION {
                        StepRecord::new(StepKind::Operation {
                            operation: name.to_string(),
                            accepts,
                            settings: crate::tone_step::new_settings(),
                        })
                    } else {
                        operation(name, accepts)
                    };
                    this.selected = Row::Layer(this.document.add_picture_step(step));
                })
            })
            .collect();
        column()
            .child(Self::field(
                "On top",
                self.menu("add-finish", "Add step", picks, cx),
            ))
            .child(Self::field(
                "",
                self.tool_menu("add-finish-tool", "Add tool", cx, |this, step| {
                    this.selected = Row::Layer(this.document.add_picture_step(step));
                }),
            ))
    }

    /// The narrowband balance: on or off, and its settings, which the canvas
    /// draws at once. It matches OIII, and SII, to H-alpha, each by a curve
    /// resolved from the stretched channels.
    fn balance_block(&self, step: &StepRecord, cx: &mut Context<Self>) -> gpui::Div {
        let id = step.id;
        let StepKind::Operation { settings, .. } = &step.kind else {
            return column();
        };
        let balance = settings["balance"].clone();
        let on = balance.is_object();
        let switch = Self::act(
            if on { "Balance on" } else { "Balance off" },
            "balance-on",
            on,
            cx,
            move |this| {
                let Some((
                    _,
                    StepRecord {
                        kind: StepKind::Operation { settings, .. },
                        ..
                    },
                )) = this.document.step(id)
                else {
                    return;
                };
                let mut settings = settings.clone();
                settings["balance"] = if on {
                    serde_json::Value::Null
                } else {
                    serde_json::json!({
                        "shadow_protection": 1.0,
                        "oiii_boost": 1.0,
                        "sii_boost": 1.0,
                    })
                };
                this.document.set_step_settings(id, settings);
            },
        );
        let sliders = super::panes::BALANCE_CONTROLS
            .iter()
            .enumerate()
            .filter(|_| on)
            .map(|(index, (key, label, low, high))| {
                let value = balance[*key].as_f64().unwrap_or(1.0);
                self.labelled_slider(
                    format!("{label} {value:.2}"),
                    SliderId::BalanceParam(index),
                    ((value - low) / (high - low)) as f32,
                    cx,
                )
            })
            .collect::<Vec<_>>();
        column()
            .child(Self::heading("Balance"))
            .child(switch)
            .children(sliders)
            .children(Self::help(
                "Matches OIII, and SII, to H-alpha: each by a curve worked out from the stretched channels, which the canvas draws once it is resolved. Shadow protection keeps the darkest sky as it is.",
            ))
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
            } if operation == flow_operations::STRETCH
                || operation == flow_operations::OUTPUT_STRETCH =>
            {
                Some(self.stretch_controls(id, settings, cx))
            }
            StepKind::Operation {
                operation,
                settings,
                ..
            } if operation == crate::tone_step::OPERATION => Some(self.tone_controls(settings, cx)),
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
        // Where a heal copies from, from the healed pixels.
        let copies = match &step.kind {
            StepKind::Heal { offset: [x, y], .. } => {
                let part = |amount: i64, positive: &str, negative: &str| match amount {
                    0 => None,
                    amount if amount > 0 => Some(format!("{amount} px {positive}")),
                    amount => Some(format!("{} px {negative}", -amount)),
                };
                let parts: Vec<String> = [part(*x, "right", "left"), part(*y, "down", "up")]
                    .into_iter()
                    .flatten()
                    .collect();
                Some(if parts.is_empty() {
                    "Copies from the healed pixels themselves.".to_string()
                } else {
                    format!("Copies from {}.", parts.join(" and "))
                })
            }
            StepKind::Operation { .. } => None,
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
            .children(copies.map(Self::note))
            .when(name == flow_operations::NARROWBAND, |pane| {
                pane.child(self.balance_block(step, cx))
            })
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
            .when(
                name == flow_operations::COLOR_CALIBRATION
                    && !self.band_info.calibration.is_empty(),
                |pane| pane.child(Self::note(self.band_info.calibration.clone())),
            )
            .when(
                !is_drawn(stage, step, &self.band_info)
                    && !(crate::studio_model::is_tool_step(step) && stage != StageKind::Linear),
                |pane| {
                    pane.children(Self::help(
                        "The document records this step. The canvas does not draw it yet.",
                    ))
                },
            )
            .when_some(mask_words(&self.document, &step.mask), |pane, words| {
                pane.child(Self::heading("Mask"))
                    .child(div().text_xs().child(words))
            })
            // A step on layers or on the picture can be limited by
            // brightness; an input's steps are baked into the input.
            .when(
                stage != StageKind::Linear && stage != StageKind::Stretch,
                |pane| {
                    pane.when(step.mask.is_none(), |pane| {
                        pane.child(Self::heading("Mask"))
                    })
                    .child(self.range_block(&step.mask, cx))
                },
            )
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

    /// A Stretch or Output stretch step's controls: which of Seiza's
    /// stretches it is, and that stretch's settings. A step that follows
    /// another Stretch step of the same input stretches what that one left,
    /// as an output stretch stretches what the output stretches before it
    /// left. An output stretch also chooses how it treats the channels.
    fn stretch_controls(
        &self,
        id: Uuid,
        settings: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let output = self
            .document
            .step(id)
            .is_some_and(|(_, step)| step.kind.name() == flow_operations::OUTPUT_STRETCH);
        let kind = settings["model"]["type"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let kinds = STRETCHES
            .iter()
            .enumerate()
            .map(|(index, (name, label))| {
                let (name, settings) = (*name, settings.clone());
                Self::pick(
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
            .child(Self::field(
                "Stretch",
                self.menu("stretch-kind", "Kind", kinds, cx),
            ))
            .when(!known, |block| {
                block.child(Self::note(format!(
                    "This step uses the stretch “{kind}”, which the canvas does not draw."
                )))
            });
        if output {
            block = block.child(self.channels_field(id, settings, cx));
        }
        for (index, (key, label, low, high)) in stretch_settings(&kind).iter().enumerate() {
            let value = settings["model"][*key].as_f64().unwrap_or(*low);
            block = block.child(self.labelled_slider(
                format!("{label} {value:.2}"),
                SliderId::StretchParam(index),
                ((value - low) / (high - low)) as f32,
                cx,
            ));
        }
        // A stretch with no sky floor of its own takes one after it.
        if !output && !kind.starts_with("layered") {
            let floor = settings[SKY_FLOOR].as_f64().unwrap_or(0.0);
            block = block
                .child(self.labelled_slider(
                    format!("Sky floor {floor:.2}"),
                    SliderId::SkyFloor,
                    (floor / MAX_SKY_FLOOR) as f32,
                    cx,
                ))
                .children(Self::help(
                    "Takes the stretched sky up to this level to black, and stretches the range above it so white stays white, as the layered stretches' Sky floor does. Use it for what background extraction leaves. 0 leaves the stretch as it is.",
                ));
        }
        // Core compression before any stretch but the layered ones, which
        // read maps of their own (Layered, HDR has its own compression).
        if !output && !kind.starts_with("layered") {
            let core: seiza_stretch::CoreCompression = settings
                .get(super::panes::CORE_COMPRESSION)
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or(seiza_stretch::CoreCompression {
                    amount: 0.0,
                    ..Default::default()
                });
            let (low, high) = super::panes::CORE_THRESHOLD;
            block = block
                .child(Self::heading("Core compression"))
                .child(self.labelled_slider(
                    format!("Amount {:.2}", core.amount),
                    SliderId::CoreAmount,
                    core.amount as f32,
                    cx,
                ))
                .when(core.amount > 0.0, |block| {
                    block
                        .child(self.labelled_slider(
                            format!("Threshold {:.0}σ", core.threshold),
                            SliderId::CoreThreshold,
                            ((core.threshold / low).ln() / (high / low).ln()) as f32,
                            cx,
                        ))
                        .child(self.labelled_slider(
                            format!("Size {:.0} px", core.scale),
                            SliderId::CoreScale,
                            (core.scale.ln() / 256f64.ln()) as f32,
                            cx,
                        ))
                })
                .children(Self::help(
                    "Brings a bright core's broad glow down before the stretch, so the stretch can show more inside it. Structure smaller than Size keeps its contrast. Threshold is how far above the background, in noise sigmas, the compression starts. 0 leaves the stretch as it is.",
                ));
        }
        // In an HDR picture every stretch continues into the headroom; the
        // knee is the picture's, shown here where the core is worked on.
        if self.document.headroom.is_some() && kind != "layered-hdr" {
            block = block
                .child(Self::heading("HDR"))
                .child(self.knee_slider(cx))
                .children(Self::help(
                    "Above the knee, this stretch continues into the picture's headroom, up to the band's brightest sample. A lower knee gives the bright core more of the headroom and more contrast there. The knee is the picture's, so it moves every stretch.",
                ));
        }
        block.child(Self::act(
            "Add stretch",
            "stretch-add",
            false,
            cx,
            move |this| {
                let Some((stage, step)) = this.document.step(id) else {
                    return;
                };
                let band = step.band.clone();
                let output = step.kind.name() == flow_operations::OUTPUT_STRETCH;
                let Some(at) = this
                    .document
                    .stage(stage)
                    .steps
                    .iter()
                    .position(|other| other.id == id)
                else {
                    return;
                };
                let mut next = if output {
                    output_stretch_step()
                } else {
                    StepRecord::new(StepKind::Operation {
                        operation: flow_operations::STRETCH.to_string(),
                        accepts: Accepts::Linear,
                        settings: serde_json::json!({
                            "model": new_stretch("ghs"),
                            "color_strategy": "linked",
                        }),
                    })
                };
                next.band = band.clone();
                let new = next.id;
                if this.document.insert_step(stage, at + 1, next).is_ok() {
                    match &this.selected {
                        Row::InputStep { channel, .. } => {
                            this.selected = Row::InputStep {
                                channel: channel.clone(),
                                step: new,
                            };
                        }
                        Row::Finish(_) => this.selected = Row::Finish(new),
                        _ => {}
                    }
                }
            },
        ))
        .children(Self::help(if output {
            "Output stretches apply in order to the combined picture. Each is resolved from what the one before it left."
        } else {
            "Stretches of one input apply in order. Each is resolved from what the one before it left."
        }))
    }

    /// A Tone and color step's controls: Camera Raw's Basic sliders.
    fn tone_controls(&self, settings: &serde_json::Value, cx: &mut Context<Self>) -> gpui::Div {
        let tone = crate::tone_step::from_settings(settings);
        let values = [
            tone.exposure,
            tone.contrast,
            tone.highlights,
            tone.shadows,
            tone.whites,
            tone.blacks,
            tone.texture,
            tone.clarity,
            tone.dehaze,
            tone.vibrance,
            tone.saturation,
        ];
        let mut block = column();
        for (index, ((_, label, low, high), value)) in
            seiza_compose::tone::CONTROLS.iter().zip(values).enumerate()
        {
            // Camera Raw's Presence group: Texture, Clarity and Dehaze, then
            // Vibrance and Saturation.
            if index == 6 {
                block = block.child(Self::heading("Presence"));
            }
            block = block.child(self.labelled_slider(
                format!("{label} {value:+.2}"),
                SliderId::FinishParam(index),
                (value - low) / (high - low),
                cx,
            ));
        }
        block.children(Self::help(
            "Exposure, Contrast, Highlights, Shadows, Whites and Blacks change the brightness and keep each pixel's color. Whites up and Blacks down clip more of the picture to white and to black. Highlights and Shadows move the tones of each area and keep the detail in it, so a star stays as bright above a lifted sky. Texture raises or lowers fine detail, Clarity the contrast of larger structures in the midtones, and Dehaze takes away a gray veil such as sky glow while keeping colored nebulae. When the picture is HDR (set under Final picture), the brightest areas keep their place above SDR white instead of clipping; raise Exposure or Whites to put more there.",
        ))
    }

    /// How an output stretch treats the channels: one curve for all three,
    /// a curve for each, or one curve for the luminance that keeps each
    /// pixel's color.
    fn channels_field(
        &self,
        id: Uuid,
        settings: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let current = settings["color_strategy"].as_str().unwrap_or("linked");
        let mut choices = Self::segmented();
        for (index, (strategy, label)) in [
            ("linked", "Linked"),
            ("unlinked", "Each channel"),
            ("luminance-preserving", "Luminance"),
        ]
        .into_iter()
        .enumerate()
        {
            let settings = settings.clone();
            choices = choices.child(Self::act(
                label,
                ("stretch-channels", index),
                strategy == current,
                cx,
                move |this| {
                    let mut settings = settings.clone();
                    settings["color_strategy"] = strategy.into();
                    this.document.set_step_settings(id, settings);
                },
            ));
        }
        // The three choices do not fit beside a label, so they have a line
        // of their own.
        column()
            .child(Self::heading("Channels"))
            .child(choices)
            .children(Self::help(
            "Linked: one curve stretches red, green and blue. Each channel: Seiza resolves a separate curve for red, green and blue, which neutralizes a colored background. Luminance: one curve stretches the brightness and each pixel keeps its color.",
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
            .child(self.routes_block(channel, cx))
            .children(self.stars_stretch_block(channel, cx))
            .children(self.continuum_block(channel, cx))
            .child(self.tools_block(channel, cx))
            .child(self.zoom_notes(channel))
            .into_any_element()
    }

    /// For the stars a tool took out of an input: whether they follow the
    /// input's stretch or have one of their own, and a button for each.
    fn stars_stretch_block(&self, channel: &str, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let input = channel.strip_suffix("@stars")?.to_string();
        let own: Option<Uuid> = self
            .document
            .stage(StageKind::Stretch)
            .steps
            .iter()
            .find(|step| {
                step.band.as_deref() == Some(channel)
                    && step.kind.name() == flow_operations::STRETCH
            })
            .map(|step| step.id);
        let channel = channel.to_string();
        let words = match own {
            Some(_) => "Their own".to_string(),
            None => format!("Follows {}'s", channel_label(&input)),
        };
        Some(
            column()
                .child(Self::field("Stretch", Self::note(words)))
                .child(match own {
                    None => Self::act("Stretch on their own", "stars-own-stretch", false, cx, move |this| {
                        this.give_stars_a_stretch(&channel, &input)
                    }),
                    Some(id) => Self::act("Follow the input's stretch", "stars-follow", false, cx, move |this| {
                        this.document.remove_step(id);
                    }),
                })
                .children(Self::help(
                    "Stars follow their input's stretch, so they go back as they were taken out. Stretched on their own, they start from a copy of it, a Stretch step of the stars that changes without changing the nebula; select it to set it.",
                )),
        )
    }

    /// Gives the stars `channel` a Stretch step of their own, a copy of
    /// their input's, and selects it.
    fn give_stars_a_stretch(&mut self, channel: &str, input: &str) {
        let source = self
            .document
            .stage(StageKind::Stretch)
            .steps
            .iter()
            .rposition(|step| {
                step.band.as_deref() == Some(input) && step.kind.name() == flow_operations::STRETCH
            });
        let Some(at) = source else {
            return;
        };
        let mut step = StepRecord::new(
            self.document.stage(StageKind::Stretch).steps[at]
                .kind
                .clone(),
        );
        step.band = Some(channel.to_string());
        let id = step.id;
        if self
            .document
            .insert_step(StageKind::Stretch, at + 1, step)
            .is_ok()
        {
            self.selected = Row::InputStep {
                channel: channel.to_string(),
                step: id,
            };
        }
    }

    /// For an emission line: whether the continuum under it is subtracted,
    /// which adds the line as an input of its own.
    fn continuum_block(&self, channel: &str, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if !["ha", "oiii", "sii"].contains(&channel) {
            return None;
        }
        let on = self.continuum_step().is_some();
        Some(
            column()
                .child(Self::field(
                    "Continuum",
                    Self::segmented()
                        .child(Self::act("Keep", "continuum-keep", !on, cx, |this| {
                            this.set_continuum(false)
                        }))
                        .child(Self::act("Subtract", "continuum-subtract", on, cx, |this| {
                            this.set_continuum(true)
                        })),
                ))
                .children(Self::help(
                    "Subtract applies to Ha, OIII and SII together. It fits the broadband light under each line from the R, G and B stacks and adds each line without that light as an input, such as Ha line. Run tools to prepare the store with them.",
                )),
        )
    }

    fn inspect_input_step(&self, channel: &str, id: Uuid, cx: &mut Context<Self>) -> AnyElement {
        let Some((stage, step)) = self.document.step(id) else {
            return Self::note("This step is gone.").into_any_element();
        };
        let label = channel_label(channel);
        let shared = step.band.is_none();
        let applies_to = if let Some(group) = self.group_applies_to(step) {
            format!("Applies to {group}: their registered inputs combined into one color image.")
        } else if shared {
            "Applies to every input that has no step of its own of this kind.".to_string()
        } else {
            format!("Applies to {label} only.")
        };
        let is_operation = matches!(step.kind, StepKind::Operation { .. });
        let own = (shared && is_operation).then(|| {
            let channel = channel.to_string();
            Self::act(
                "Own settings".to_string(),
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
            .when(crate::studio_model::is_tool_step(step), |pane| {
                pane.child(self.tools_block(&channel, cx))
            })
            .children(self.linear_order_block(step, &channel, cx))
            .when(step.kind.name() == flow_operations::BACKGROUND, |pane| {
                pane.child(self.protect_block(id, cx))
            })
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
            .showable()
            .into_iter()
            .enumerate()
            .map(|(index, channel)| {
                let on = channel == *band;
                Self::pick(
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
            .child(self.shows_block(
                "Only shows this layer's input, in gray, without its color, opacity or blend.",
                cx,
            ))
            .child(Self::field(
                "Input",
                self.menu("layer-input", "Input", inputs, cx),
            ))
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
            .children(self.star_reduction_block(layer, cx))
            .child(self.blend_block(layer, cx))
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
            .children(Self::help(
                "A clipped layer shows only inside the mask of the layer beneath it, and not at all while that layer is hidden.",
            ))
            .child(Self::field("Adjust", self.adjustment_menu(
                "adjust-own",
                "Add to this",
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
            )))
            .child(Self::field("", self.adjustment_menu(
                "adjust-above",
                "Add above",
                cx,
                move |this, name| this.add_adjustment_above(Some(id), name),
            )))
            .child(Self::field(
                "Tools",
                self.tool_menu("layer-tool", "Run a tool on this", cx, move |this, step| {
                    let step_id = step.id;
                    if this.document.insert_layer_step(id, step).is_ok() {
                        this.selected = Row::LayerStep {
                            layer: id,
                            step: step_id,
                        };
                    }
                }),
            ))
            .children(Self::help(
                "A tool here runs on this layer alone, as its stretch and its edits before the tool leave it, when the tools run. The stars a star removal takes out go back right after it.",
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
                            // The copy is a person's layer, not the
                            // palette's: the palette fills each slot once.
                            copy.slot = None;
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
                    .when(
                        self.document.ancestors(id).len() < crate::studio_model::MAX_GROUP_DEPTH,
                        |row| {
                        row.child(Self::act(
                            "Group",
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
            .child(self.order_block(id, cx))
            .into_any_element()
    }

    /// Buttons that move a layer, or a group with its members, one place
    /// up or down the stack, as the list shows it.
    fn order_block(&self, id: Uuid, cx: &mut Context<Self>) -> gpui::Div {
        column()
            .child(Self::field(
                "Order",
                wrap()
                    .child(Self::act("Move up", "layer-up", false, cx, move |this| {
                        this.document.step_layer(id, true);
                    }))
                    .child(Self::act("Move down", "layer-down", false, cx, move |this| {
                        this.document.step_layer(id, false);
                    })),
            ))
            .children(Self::help(
                "A layer in a group moves among the group's layers; a group moves with its layers past the next layer or group. A row dragged onto a layer goes above it, or below it when dropped on the layer's lower half; dragged onto a group, it joins the group on top.",
            ))
    }

    /// The blend modes, with the layer's own marked.
    fn blend_block(&self, layer: &LayerRecord, cx: &mut Context<Self>) -> gpui::Div {
        Self::field("Blend", self.blend_menu(layer, cx))
    }

    /// Whether the canvas shows the stack up to the selected layer, or
    /// only the selected layer.
    fn shows_block(&self, only_help: &'static str, cx: &mut Context<Self>) -> gpui::Div {
        let alone = self.alone;
        column()
            .child(Self::field(
                "Show",
                Self::segmented()
                    .child(Self::act("Stack", "show-stack", !alone, cx, |this| {
                        this.alone = false
                    }))
                    .child(Self::act("Only", "show-alone", alone, cx, |this| {
                        this.alone = true
                    })),
            ))
            .children(Self::help(only_help))
    }

    /// Buttons that move a step that changes an input's samples, such as
    /// a tool or the background extraction, past the step before or after
    /// it in the input's list. The store runs them in that order.
    fn linear_order_block(
        &self,
        step: &StepRecord,
        channel: &str,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        if !crate::studio_model::changes_input(step, channel) {
            return None;
        }
        let id = step.id;
        let mut mover = |label: &'static str, key: &'static str, later: bool| {
            let channel = channel.to_string();
            Self::act(label, key, false, cx, move |this| {
                crate::studio_model::move_linear_step(&mut this.document, id, &channel, later);
            })
        };
        let (earlier, later) = (
            mover("Move earlier", "linear-earlier", false),
            mover("Move later", "linear-later", true),
        );
        Some(
            column()
                .child(Self::field("Order", wrap().child(earlier).child(later)))
                .children(Self::help(
                    "Moves the step past the one before or after it in this input's list. Preparing the store runs the tools, deconvolution and background extraction in that order: a tool after Background extraction runs on the stack with its background corrected. A step shared by every input, such as Background extraction, moves for every input. A new order takes effect when the tools run again; the toolbar lists the inputs.",
                )),
        )
    }

    /// The Star reduction slider, for a layer that shows stars a tool took
    /// out of an input, or a group with such layers.
    fn star_reduction_block(
        &self,
        layer: &LayerRecord,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let is_stars = |layer: &LayerRecord| matches!(&layer.source, LayerSource::Band { band } if band.ends_with("@stars"));
        let group = matches!(layer.source, LayerSource::Group);
        let shows_stars = if group {
            self.document.group_members(layer.id).any(is_stars)
        } else {
            is_stars(layer)
        };
        if !shows_stars {
            return None;
        }
        let from_group = (!group && layer.star_reduction == 0.0)
            .then(|| {
                self.document
                    .layers()
                    .iter()
                    .find(|other| Some(other.id) == layer.group)
                    .map(|group| group.star_reduction)
            })
            .flatten()
            .filter(|amount| *amount > 0.0);
        let help = if group {
            "Makes the group's stars smaller: each is blended toward a one-pixel erosion of itself. The stars of its layers are reduced together, so their colors keep. The first change works out the reduced stars from the whole image, which takes a moment."
        } else {
            "Makes this layer's stars smaller: each is blended toward a one-pixel erosion of itself. Above 0%, this layer's amount is used in place of its group's. The first change works out the reduced stars from the whole image, which takes a moment."
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(self.labelled_slider(
                    format!("Star reduction {:.0}%", layer.star_reduction * 100.0),
                    SliderId::StarReduction,
                    layer.star_reduction,
                    cx,
                ))
                .children(from_group.map(|amount| {
                    Self::note(format!(
                        "Reduced {:.0}% with the group's other stars.",
                        amount * 100.0
                    ))
                }))
                .children(Self::help(help)),
        )
    }

    fn inspect_group(&self, layer: &LayerRecord, cx: &mut Context<Self>) -> AnyElement {
        let id = layer.id;
        let members = self.document.group_members(id).count();
        let (pass_through, clipped) = (layer.pass_through, layer.clipped);
        // A group goes into a new group while every layer in it stays
        // within the depth groups go to.
        let can_nest = {
            let mut trial = self.document.clone();
            trial.group_layer(id).is_ok()
        };
        column()
            .child(Self::title(
                layer.name.clone(),
                format!(
                    "A group of {}. Its layers blend together on their own, then with the layers beneath as one layer.",
                    super::panes::counted(members, "layer")
                ),
            ))
            .child(self.shows_block(
                "Only shows the group's layers blended together, in color, without the layers beneath.",
                cx,
            ))
            .children(Self::help(
                "Drag a layer onto the group, or into the gap under one of its layers, to add it. Drag it out to remove it.",
            ))
            .child(self.labelled_slider(
                format!("Opacity {:.0}%", layer.opacity * 100.0),
                SliderId::Opacity,
                layer.opacity,
                cx,
            ))
            .children(self.star_reduction_block(layer, cx))
            .child(self.blend_block(layer, cx))
            .child(self.palette_block(Some(id), cx))
            .child(Self::field("Layers", self.add_layer_menu(Some(id), cx)))
            .child(self.mask_block(layer, cx))
            .child(self.shared_mask_block(layer, cx))
            .child(Self::field("Adjust", self.adjustment_menu(
                "adjust-own",
                "Add to this",
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
            )))
            .child(Self::field("", self.adjustment_menu(
                "adjust-above",
                "Add above",
                cx,
                move |this, name| this.add_adjustment_above(Some(id), name),
            )))
            .child(Self::heading("Group"))
            .child(Self::act(
                if pass_through {
                    "Layers pass through"
                } else {
                    "Layers blend together first"
                },
                "pass-through",
                pass_through,
                cx,
                move |this| {
                    if let Some(group) = this.document.layer_mut(id) {
                        group.pass_through = !pass_through;
                    }
                },
            ))
            .children(Self::help(
                "Passing through, each layer blends straight onto the layers beneath the group, as if it were not grouped; the group's opacity and masks still limit each one, and its own adjustments do not apply.",
            ))
            .child(Self::act(
                if clipped { "Clipped" } else { "Not clipped" },
                "group-clipped",
                clipped,
                cx,
                move |this| {
                    if let Some(group) = this.document.layer_mut(id) {
                        group.clipped = !clipped;
                    }
                },
            ))
            .children(Self::help(
                "A clipped group shows only inside the mask of the layer beneath it, and not at all while that layer is hidden.",
            ))
            .child(
                wrap()
                    .child(Self::act("Ungroup", "ungroup", false, cx, move |this| {
                        this.document.ungroup(id);
                        this.selected = Row::Combined;
                    }))
                    .when(can_nest, |row| {
                        row.child(Self::act("Group", "group-group", false, cx, move |this| {
                            if let Ok(group) = this.document.group_layer(id) {
                                this.selected = Row::Layer(group);
                            }
                        }))
                    })
                    .child(Self::act("Delete", "part-delete", false, cx, move |this| {
                        this.remove_part(id)
                    })),
            )
            .children(Self::help(
                "Ungrouping keeps the layers where they are and removes the group's own mask and adjustments.",
            ))
            .child(self.order_block(id, cx))
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
                Self::pick(
                    mask.name.clone(),
                    ("layer-mask", index),
                    on,
                    cx,
                    move |this| {
                        this.document
                            .set_layer_shared_mask(id, (!on).then_some((mask_id, false)));
                    },
                )
            })
            .collect::<Vec<_>>();
        let first_input = self.document.channels().into_iter().next();
        column()
            .child(Self::field(
                "Mask",
                self.menu("layer-mask", "None", masks, cx),
            ))
            .child(Self::field("", Self::act(
                "New mask",
                "layer-mask-new",
                false,
                cx,
                move |this| {
                    let Some(input) = first_input.clone() else {
                        return;
                    };
                    let mask = this.document.add_mask(SharedMask::new(vec![input]));
                    this.document.set_layer_shared_mask(id, Some((mask, false)));
                    this.selected = Row::Mask(mask);
                },
            )))
            .children(Self::help(
                "A shared mask follows the signal of one or two inputs. Several layers can be behind the same mask, or its inverse.",
            ))
            .when(painted || layer.painted_too.is_some(), |pane| {
                pane.children(Self::help(
                    "A painted mask stays with a shared mask: the layer shows only where both let it.",
                ))
            })
            .when_some(used, |pane, (mask, inverted)| {
                let set = move |this: &mut Self, inverted: bool| {
                    this.document
                        .set_layer_shared_mask(id, Some((mask, inverted)));
                };
                pane.child(
                    Self::segmented()
                        .child(Self::act(
                            "Strong",
                            "mask-strong",
                            !inverted,
                            cx,
                            move |this| set(this, false),
                        ))
                        .child(Self::act(
                            "Weak",
                            "mask-weak",
                            inverted,
                            cx,
                            move |this| set(this, true),
                        ))
                        .child(Self::act("Edit mask", "mask-edit", false, cx, move |this| {
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
            .child(Self::heading("View"))
            .child(
                Self::segmented()
                    .child(Self::act(
                        "Mask",
                        "mask-view-mask",
                        !self.mask_shows_picture,
                        cx,
                        |this| this.mask_shows_picture = false,
                    ))
                    .child(Self::act(
                        "Picture",
                        "mask-view-picture",
                        self.mask_shows_picture,
                        cx,
                        |this| this.mask_shows_picture = true,
                    )),
            )
            .children(Self::help(
                "The With final view shows the mask and the final picture side by side.",
            ))
            .child(Self::heading("Used by"))
            .child(div().text_xs().child(if users.is_empty() {
                "None.".to_string()
            } else {
                users.join(", ")
            }))
            .child(Self::heading("Inputs"))
            .child(wrap().children(inputs))
            .children(Self::help(
                "Each input is read as its own Stretch step leaves it, so the mask follows the inputs.",
            ))
            .child(Self::heading("Shape"))
            .child(
                Self::segmented()
                    .child(Self::act(
                        "Lift faint",
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
            .children(Self::help(
                "Below 1 the mask reaches further into faint signal. Above 1 it keeps to strong signal.",
            ))
            .child(self.labelled_slider(
                format!("Smoothing {smoothing:.1} px"),
                SliderId::Smoothing,
                smoothing / 32.0,
                cx,
            ))
            .children(Self::help(
                "Blurs the mask so the inputs' noise does not become speckle in the layers behind it. 0 leaves the mask as computed.",
            ))
            .child(Self::heading("Touch-ups"))
            .child(div().text_xs().child(format!(
                "{}. Paint with the mask brush.",
                super::panes::counted(touch_ups, "stroke")
            )))
            .child(
                Self::segmented()
                    .child(Self::act("Brush adds", "brush-adds", !removes, cx, |this| {
                        this.brush_removes = false
                    }))
                    .child(Self::act(
                        "Brush removes",
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
            .children(Self::help(
                "Removing the mask leaves the layers behind it showing everywhere.",
            ))
            .into_any_element()
    }

    /// The tone of every layer that shows an input, in one list: a palette
    /// is a starting point, and each layer's hue and saturation are set
    /// here without selecting the layers one by one.
    fn tones_block(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut block = column().children(Self::help(
            "Each layer's hue and saturation. A palette sets them to start with.",
        ));
        // Top layer first, as the tree lists them.
        for (index, layer) in self.document.layers().iter().enumerate().rev() {
            let Some(color) = layer.color.filter(|_| layer.source.shows_pixels()) else {
                continue;
            };
            if layer.blend == BlendMode::Luminance || index >= seiza_compose::gpu::MAX_LAYERS {
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

    /// A layer's mask, and the button that removes it.
    /// A mask by brightness: its sliders when the layer or step has one, or
    /// the button that adds one when it has no mask.
    pub(super) fn range_block(
        &self,
        mask: &Option<MaskRecord>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        match mask {
            Some(MaskRecord::Range { low, high, feather }) => {
                let to = if *high >= super::panes::RANGE_BRIGHTEST {
                    "To the brightest".to_string()
                } else {
                    format!("To {high:.2}")
                };
                column()
                    .child(self.labelled_slider(format!("From {low:.2}"), SliderId::RangeLow, *low, cx))
                    .child(self.labelled_slider(
                        to,
                        SliderId::RangeHigh,
                        high.min(1.0),
                        cx,
                    ))
                    .child(self.labelled_slider(
                        format!("Softness {feather:.2}"),
                        SliderId::RangeFeather,
                        feather / super::panes::RANGE_MAX_FEATHER,
                        cx,
                    ))
                    .children(Self::help(
                        "Applies only where the brightness is between From and To, fading out over Softness beyond them. A layer that shows an input reads its own brightness; an adjustment or a step reads the picture it changes.",
                    ))
            }
            None => column().child(Self::act(
                "Mask by brightness",
                "range-add",
                false,
                cx,
                |this| {
                    if let Some(mask) = this.range_mask_mut() {
                        *mask = Some(MaskRecord::Range {
                            low: 0.5,
                            high: super::panes::RANGE_BRIGHTEST,
                            feather: 0.1,
                        });
                    }
                },
            )),
            Some(_) => column(),
        }
    }

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
            .child(self.range_block(&layer.mask, cx))
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
        let Some(StepRecord {
            kind:
                StepKind::Operation {
                    operation,
                    settings,
                    ..
                },
            ..
        }) = layer.source.adjustment_step()
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
                    .child(Self::button("Beneath", "scope-stack", true))
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
            .children(Self::help(
                "Dragging does the same: onto a layer for that layer only, between layers for every layer beneath.",
            ))
            .child(self.mask_block(layer, cx))
            .child(self.shared_mask_block(layer, cx))
            .child(Self::act("Remove", "layer-remove", false, cx, move |this| {
                this.document.remove_layer(id);
                this.selected = Row::Combined;
            }))
            .child(self.order_block(id, cx))
            .into_any_element()
    }

    /// An adjustment layer that changes the layers beneath it as a Finish
    /// step changes the picture: a stretch, Tone, a heal or a tool.
    fn inspect_step_layer(&self, layer: &LayerRecord, cx: &mut Context<Self>) -> AnyElement {
        let Some(step) = layer.source.adjustment_step() else {
            return div().into_any_element();
        };
        let id = layer.id;
        let tool = crate::studio_model::is_tool_step(step);
        // The step applies while the layer is shown, at the layer's
        // opacity: what a step's own switch and opacity do elsewhere.
        let mut shown = step.clone();
        shown.enabled = layer.visible;
        shown.opacity = layer.opacity;
        // The layer's mask has a block of its own.
        shown.mask = None;
        let heal = matches!(step.kind, StepKind::Heal { .. });
        column()
            .child(Self::title(
                layer.name.clone(),
                if tool {
                    "A tool run on the picture as the layers beneath it make it. Its result covers them."
                } else {
                    "A step on the picture as the layers beneath it make it."
                },
            ))
            .child(self.histogram_block())
            .child(self.step_controls(StageKind::Finish, &shown, cx))
            .when(tool, |pane| {
                pane.child(self.late_tool_block(
                    step,
                    if layer.group.is_some() {
                        "its group's members beneath it"
                    } else {
                        "the layers beneath it"
                    },
                    cx,
                ))
            })
            .when(!heal, |pane| {
                pane.child(self.mask_block(layer, cx))
                    .child(self.shared_mask_block(layer, cx))
            })
            .child(Self::act("Remove", "layer-remove", false, cx, move |this| {
                this.document.remove_layer(id);
                this.selected = Row::Combined;
            }))
            .child(self.order_block(id, cx))
            .child(self.finish_menu(cx))
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
            .when(crate::studio_model::is_tool_step(step), |pane| {
                pane.child(self.late_tool_block(step, &format!("{} alone", owner.name), cx))
            })
            .when(
                is_adjustment && !crate::studio_model::is_tool_step(step),
                |pane| {
                    pane.child(Self::heading("Applies to"))
                        .child(
                            wrap()
                                .child(Self::button(
                                    format!("{} only", owner.name),
                                    "scope-own",
                                    true,
                                ))
                                .child(Self::act(
                                    "Beneath",
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
                        .children(Self::help(
                            "Paint with the mask brush to limit where this adjustment applies.",
                        ))
                },
            )
            .child(Self::act("Remove", "step-remove", false, cx, move |this| {
                this.document.remove_step(id);
                this.selected = Row::Layer(layer);
            }))
            .into_any_element()
    }

    /// Every input's tools in one table: what runs on each, in order, and
    /// whether a run is due. Clicking an input selects it.
    /// An input's processing tools, in the order they run: "BlurXTerminator
    /// · StarXTerminator", or "no tools".
    pub(super) fn tool_words(&self, channel: &str) -> String {
        let tools: Vec<String> = crate::studio_model::input_steps(&self.document, channel)
            .into_iter()
            .filter_map(|listed| {
                let (stage, step) = self.document.step(listed.id)?;
                (stage == StageKind::Linear
                    && step.enabled
                    && crate::studio_model::is_tool_step(step))
                .then(|| step.kind.name().to_string())
            })
            .collect();
        if tools.is_empty() {
            "no tools".to_string()
        } else {
            tools.join(" · ")
        }
    }

    fn tools_table(&self, cx: &mut Context<Self>) -> gpui::Div {
        let due = self.inputs_to_run();
        let rows = self
            .document
            .channels()
            .into_iter()
            .enumerate()
            .map(|(index, channel)| {
                let words = self.tool_words(&channel);
                let flag = if due.contains(&channel) {
                    " · to run"
                } else {
                    ""
                };
                let picked = channel.clone();
                div()
                    .id(("tools-table", index))
                    .flex()
                    .gap_2()
                    .text_xs()
                    .cursor_pointer()
                    .child(div().w(px(72.0)).child(channel_label(&channel)))
                    .child(div().flex_1().child(format!("{words}{flag}")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Row::Input(picked.clone());
                        this.refresh_canvas();
                        cx.notify();
                    }))
            })
            .collect::<Vec<_>>();
        column()
            .child(Self::heading("Tools by input"))
            .when_some(self.document.recipe.clone(), |pane, recipe| {
                pane.child(div().text_xs().child(format!("Made with the recipe {recipe}.")))
            })
            .children(rows)
            .children(Self::help(
                "Each input's processing tools, in the order they run. Click an input to change them.",
            ))
    }

    fn inspect_combined(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .child(Self::title(
                "Combined",
                "Every layer, blended bottom to top.",
            ))
            .child(self.tools_table(cx))
            .child(self.palette_block(None, cx))
            .child(self.section("tones", "Tones", false, cx, |this, cx| this.tones_block(cx)))
            .child(Self::act("Balance colors", "balance", false, cx, |this| {
                let mut layers = this.document.layers().to_vec();
                balance_colors(&mut layers);
                for balanced in layers {
                    if let Some(layer) = this.document.layer_mut(balanced.id) {
                        layer.color = balanced.color;
                    }
                }
            }))
            .children(Self::help(
                "Scales the added layers together so their sum is full scale.",
            ))
            .child(Self::field("Layers", self.add_layer_menu(None, cx)))
            .child(Self::field(
                "Adjust",
                self.adjustment_menu("adjust-top", "Add on top", cx, |this, name| {
                    this.add_adjustment_above(None, name)
                }),
            ))
            .child(self.finish_menu(cx))
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
            .child(self.histogram_block())
            .child(self.step_controls(stage, step, cx))
            .when(crate::studio_model::is_tool_step(step), |pane| {
                pane.child(self.late_tool_block(step, "the picture", cx))
            })
            .when(!crate::studio_model::is_tool_step(step), |pane| {
                pane.children(Self::help(
                    "Paint with the mask brush to limit where this step applies.",
                ))
            })
            .child(Self::act("Remove", "step-remove", false, cx, move |this| {
                this.document.remove_step(id);
                this.selected = Row::Combined;
            }))
            .child(self.finish_menu(cx))
            .into_any_element()
    }

    /// The histogram of the final picture: red, green and blue over the
    /// luminance, with the share of pixels clipped to black and to white.
    fn histogram_block(&self) -> gpui::Div {
        let Some(histogram) = self.histogram.clone() else {
            return column()
                .child(Self::heading("Histogram"))
                .child(Self::note("Reading the picture…"));
        };
        let (black, white) = (histogram.black * 100.0, histogram.white * 100.0);
        let above = histogram.above * 100.0;
        let empty = histogram.masked && histogram.pixels == 0;
        let title = if histogram.masked {
            "Histogram of the masked area"
        } else {
            "Histogram"
        };
        let chart = gpui::canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                let peak = histogram.peak() as f32;
                let width = f32::from(bounds.size.width) / crate::histogram::BINS as f32;
                let height = f32::from(bounds.size.height);
                // Luminance first, in gray, then each channel over it.
                let colors = [0x9a9a9aff, 0xe0505080, 0x50d05080, 0x5080f080];
                for (channel, color) in [3, 0, 1, 2].into_iter().zip(colors) {
                    for (index, count) in histogram.bins[channel].iter().enumerate() {
                        let tall = (*count as f32 / peak).min(1.0) * height;
                        if tall <= 0.0 {
                            continue;
                        }
                        let origin = gpui::point(
                            bounds.origin.x + gpui::px(index as f32 * width),
                            bounds.origin.y + gpui::px(height - tall),
                        );
                        window.paint_quad(gpui::fill(
                            gpui::Bounds::new(origin, gpui::size(gpui::px(width), gpui::px(tall))),
                            gpui::rgba(color),
                        ));
                    }
                }
            },
        )
        .w_full()
        .h(gpui::px(80.0));
        column()
            .child(Self::heading(title))
            .when(empty, |block| {
                block.child(Self::note(
                    "The mask covers none of the sampled picture. Paint it larger, or move it, to see its area here.",
                ))
            })
            .child(div().bg(gpui::rgb(0x111316)).child(chart))
            .child(Self::note(if above > 0.0 {
                format!(
                    "Clipped: {black:.2}% to black, {white:.2}% to white. Above SDR white: {above:.2}%"
                )
            } else {
                format!("Clipped: {black:.2}% to black, {white:.2}% to white")
            }))
    }

    /// HDR: whether the whole picture may be brighter than SDR white, and how
    /// far.
    fn hdr_block(&self, cx: &mut Context<Self>) -> gpui::Div {
        let headroom = self.document.headroom;
        let mut block = column().child(Self::field(
            "HDR",
            Self::act(
                if headroom.is_some() { "On" } else { "Off" },
                "picture-hdr",
                headroom.is_some(),
                cx,
                move |this| this.set_hdr(headroom.is_none()),
            ),
        ));
        if let Some(headroom) = headroom {
            let (_, label, low, high) = crate::tone_step::HEADROOM;
            block = block
                .child(self.labelled_slider(
                    format!("{label} {headroom:.1} stops"),
                    SliderId::Headroom,
                    (headroom - low) / (high - low),
                    cx,
                ))
                .child(self.knee_slider(cx));
        }
        block.children(Self::help(
            "With HDR on, the whole picture may be brighter than SDR white, up to the headroom (each stop doubles the light): the stretches, the combined picture and every Finish step. Every stretch continues above its knee into the headroom, so the band's brightest sample reaches the top; Layered, HDR also compresses the core. A lower knee gives the bright core more of the headroom and more contrast there. Every view of the picture shows it brighter than white on an HDR display. Export it as AVIF, a 32-bit float TIFF, FITS or XISF to keep it.",
        ))
    }

    /// The picture's HDR knee, as a slider.
    fn knee_slider(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (_, label, low, high) = crate::tone_step::HDR_KNEE;
        let knee = self
            .document
            .hdr_knee
            .unwrap_or(crate::tone_step::DEFAULT_HDR_KNEE);
        self.labelled_slider(
            format!("{label} {knee:.2}"),
            SliderId::HdrKnee,
            (knee - low) / (high - low),
            cx,
        )
    }

    /// Export: the final picture at full resolution, to a file.
    fn export_block(&self, cx: &mut Context<Self>) -> gpui::Div {
        let running = self.export.clone();
        column()
            .child(Self::field(
                "Export",
                match running {
                    Some(run) => Self::act("Stop export", "export-stop", false, cx, move |_| {
                        run.stop()
                    }),
                    None => Self::button("Export…", "export", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.open_menu = None;
                            this.pick_export(cx);
                            cx.stop_propagation();
                            cx.notify();
                        },
                    )),
                },
            ))
            .child(Self::field(
                "Format",
                gpui::div().flex().flex_wrap().children(
                    super::export::EXPORT_FORMATS
                        .iter()
                        .enumerate()
                        .map(|(index, (extension, label))| {
                            let extension: &'static str = extension;
                            Self::act(
                                *label,
                                ("export-format", index),
                                self.export_format() == extension,
                                cx,
                                move |this| this.export_format = Some(extension),
                            )
                        }),
                ),
            ))
            .when(self.export_format() == "tif", |block| {
                block.child(Self::field(
                    "TIFF",
                    gpui::div()
                        .flex()
                        .child(Self::act("16-bit", "tiff-16", !self.tiff_float, cx, |this| {
                            this.tiff_float = false
                        }))
                        .child(Self::act("32-bit float", "tiff-32", self.tiff_float, cx, |this| {
                            this.tiff_float = true
                        })),
                ))
            })
            .when(self.server_save.linked(), |block| {
                block.child(Self::field(
                    "Project",
                    Self::act(
                        "Also save to the project",
                        "export-to-project",
                        self.server_save.export_to_project,
                        cx,
                        |this| {
                            this.server_save.export_to_project = !this.server_save.export_to_project
                        },
                    ),
                ))
            })
            .when(!self.export_line.is_empty(), |block| {
                block.child(Self::note(self.export_line.clone()))
            })
            .children(Self::help(
                "Saves the picture as the canvas shows it, cropped, at full resolution: TIFF at 16 bits per channel or as 32-bit floats, PNG at 16 bits, FITS or XISF as 32-bit floats, or AVIF as HDR (PQ, Rec. 2020), which keeps an HDR picture's brightest areas above SDR white. Format sets the file's extension; an HDR picture starts as AVIF. A 32-bit float TIFF also keeps values above SDR white as they are. An extension typed in the name wins. When Studio was started with a working space's ICC profile, a TIFF or PNG carries it. For a picture on a PSF Guard server, Also save to the project uploads the file to the project as well, with the document revision it came from.",
            ))
    }

    /// The view stretch: what draws a channel nothing stretches yet, for
    /// looking only.
    fn inspect_view_stretch(&self, cx: &mut Context<Self>) -> AnyElement {
        let kind = self.view_stretch["type"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let kinds = STRETCHES
            .iter()
            .enumerate()
            .map(|(index, (name, label))| {
                let name = *name;
                Self::pick(
                    *label,
                    ("view-stretch-kind", index),
                    name == kind,
                    cx,
                    move |this| this.view_stretch = new_stretch(name),
                )
            })
            .collect::<Vec<_>>();
        let mut block = column()
            .child(Self::title(
                "View stretch",
                "For looking only. Not saved or exported.",
            ))
            .child(Self::field(
                "Stretch",
                self.menu("view-stretch-kind", "Kind", kinds, cx),
            ));
        for (index, (key, label, low, high)) in stretch_settings(&kind).iter().enumerate() {
            let value = self.view_stretch[*key].as_f64().unwrap_or(*low);
            block = block.child(self.labelled_slider(
                format!("{label} {value:.2}"),
                SliderId::ViewStretchParam(index),
                ((value - low) / (high - low)) as f32,
                cx,
            ));
        }
        block
            .child(Self::act("Done", "view-stretch-done", false, cx, |this| {
                this.view_stretch_open = false
            }))
            .children(Self::help(
                "The view stretch draws a channel that nothing in the view stretches yet: before the Stretch stage, or a channel with no Stretch step. A channel the document stretches is always drawn with the document's stretch. The view stretch only changes what you see: the document does not keep it, and an export leaves such a channel linear.",
            ))
            .into_any_element()
    }

    fn inspect_final(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .child(Self::title("Final picture", "Every step of the document."))
            .child(self.histogram_block())
            .child(self.hdr_block(cx))
            .child(self.export_block(cx))
            .child(self.crop_block(cx))
            .child(self.finish_menu(cx))
            .child(self.junction_block(cx))
            .into_any_element()
    }

    /// What the picture keeps: the automatic crop, the whole image, or a
    /// rectangle drawn with the Crop tool.
    fn crop_block(&self, cx: &mut Context<Self>) -> gpui::Div {
        let crop = self.document.crop;
        let drawn = matches!(crop, CropRecord::Region { .. });
        column()
            .child(Self::field(
                "Crop",
                Self::segmented()
                    .child(Self::act(
                        "Auto",
                        "crop-auto",
                        crop == CropRecord::Automatic,
                        cx,
                        |this| {
                            this.document.crop = CropRecord::Automatic;
                            this.refresh_canvas();
                            this.canvas.edit(|canvas| canvas.fit());
                        },
                    ))
                    .child(Self::act(
                        "Whole",
                        "crop-whole",
                        crop == CropRecord::Whole,
                        cx,
                        |this| {
                            this.document.crop = CropRecord::Whole;
                            this.refresh_canvas();
                            this.canvas.edit(|canvas| canvas.fit());
                        },
                    ))
                    .child(Self::act("Drawn", "crop-drawn", drawn, cx, |this| {
                        this.tool = Tool::Crop;
                    })),
            ))
            .when_some(
                match crop {
                    CropRecord::Region { width, height, .. } => Some((width, height)),
                    _ => self
                        .band_info
                        .automatic_crop
                        .filter(|_| crop == CropRecord::Automatic)
                        .map(|[_, _, width, height]| (width, height)),
                },
                |block, (width, height)| block.child(Self::note(format!("{width} × {height} pixels"))),
            )
            .children(Self::help(
                "Auto crops to the largest rectangle every input covers, removing the empty edges left by registration. Drawn crops to a rectangle drawn with the Crop tool.",
            ))
    }

    // ---- blocks under the row's controls --------------------------------

    /// The zoom and the pyramid level the view reads. Every step's result
    /// is drawn at any zoom: the tools ran on the whole stack when the store
    /// was prepared.
    fn zoom_notes(&self, _channel: &str) -> gpui::Div {
        let zoom = self.canvas.read(|shared| shared.canvas.view.zoom);
        let level = level_for_zoom(zoom, self.levels);
        column().child(Self::heading("Zoom")).child(
            div()
                .text_xs()
                .child(format!("Zoom {:.0}%, pyramid level {level}", zoom * 100.0)),
        )
    }

    /// The round trip to another editor: what the canvas shows, and the
    /// buttons that open the picture there and come back.
    fn junction_block(&self, cx: &mut Context<Self>) -> gpui::Div {
        let status = self.editor_status();
        let out = !matches!(status, JunctionStatus::Computed);
        let words = match status {
            JunctionStatus::Computed => "The canvas and Export show Studio's picture.",
            JunctionStatus::WaitingForEdit => {
                "The picture is open in another app. Save it there and Studio brings the edit back."
            }
            JunctionStatus::Edited => "The canvas and Export show the file the other app saved.",
            JunctionStatus::EditedFromOlderResult => {
                "The canvas and Export show the file the other app saved, which does not include the changes made in Studio since it was opened there."
            }
            JunctionStatus::WaitingOnOlderResult => {
                "The picture is open in another app, and has changed in Studio since. Open it again to edit the current picture."
            }
        };
        column()
            .child(Self::heading("External editor"))
            .child(div().text_xs().child(words))
            .child(
                wrap()
                    .child(Self::act(
                        if out { "Open again" } else { "Open in external editor" },
                        "editor-open",
                        false,
                        cx,
                        |this| this.open_in_editor(),
                    ))
                    .when(out, |row| {
                        row.child(Self::act(
                            "Use Studio's result",
                            "editor-back",
                            false,
                            cx,
                            |this| this.use_studio_result(),
                        ))
                    }),
            )
            .children(Self::help(
                "Opens the final picture as a 16-bit TIFF in its default app, such as Photoshop. The file is kept beside the document.",
            ))
            .children(Self::help(
                "Disabled: Studio cannot write the file for an editor yet.",
            ))
    }

    /// Where a heal stroke goes, for the selected row.
    fn heal_target(&self) -> String {
        match &self.selected {
            Row::Input(channel) | Row::InputStep { channel, .. } => {
                format!("the {} input only", channel_label(channel))
            }
            Row::Layer(layer) | Row::LayerStep { layer, .. } => match self.layer(*layer) {
                Some(layer) if layer.source.shows_pixels() => {
                    format!("the layer {} only", layer.name)
                }
                Some(layer) if layer.source.is_adjustment() => format!(
                    "the picture as {} leaves it, as a Heal layer right above it",
                    layer.name
                ),
                _ => "nothing: a group has no pixels to heal".to_string(),
            },
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
                "Option-click where the heal copies from, then drag over a blemish; later strokes copy from the same distance. Each stroke becomes a Heal step of {}.",
                self.heal_target()
            ),
            Tool::Crop => {
                "Drag a rectangle over the picture. Studio crops the picture to it.".to_string()
            }
            Tool::Protect => {
                "Press at a region's centre and drag out to set its width and height. The background extraction keeps it out of the sky it measures.".to_string()
            }
        };
        let brushing = matches!(self.tool, Tool::Brush | Tool::Heal);
        let painting = self.tool == Tool::Brush && can_mask;
        let removes = self.brush_removes;
        let target = self.mask_target();
        let blur = self.mask_blur();
        column()
            .child(Self::heading("Tools"))
            .child(
                Self::segmented()
                    .child(tool(Tool::Pan, "Pan", cx))
                    .when(can_mask, |row| {
                        row.child(tool(Tool::Brush, "Brush", cx))
                    })
                    .child(tool(Tool::Heal, "Heal", cx))
                    .child(tool(Tool::Crop, "Crop", cx))
                    .when(self.selected_background_step().is_some(), |row| {
                        row.child(tool(Tool::Protect, "Protect", cx))
                    }),
            )
            .children(Self::help(help))
            .when(brushing, |pane| {
                pane.child(self.labelled_slider(
                    format!("Size {:.0}", self.brush_size),
                    SliderId::BrushSize,
                    ((self.brush_size - 2.0) / 198.0).max(0.0).sqrt(),
                    cx,
                ))
            })
            .when(painting, |pane| {
                pane.child(Self::field(
                    "Brush",
                    Self::segmented()
                        .child(Self::act("Paint", "brush-paints", !removes, cx, |this| {
                            this.brush_removes = false
                        }))
                        .child(Self::act("Erase", "brush-erases", removes, cx, |this| {
                            this.brush_removes = true
                        })),
                ))
            })
            .when(self.can_undo_stroke(), |pane| {
                pane.child(Self::field(
                    "",
                    Self::act("Undo stroke", "undo-stroke", false, cx, move |this| {
                        if let Some(target) = target {
                            this.document.remove_last_stroke(target);
                        }
                    }),
                ))
            })
            .when_some(blur, |pane, blur| {
                pane.child(self.labelled_slider(
                    format!("Blur {blur:.0} px"),
                    SliderId::MaskBlur,
                    (blur / 64.0).sqrt(),
                    cx,
                ))
                .children(Self::help(
                    "Blur softens the edge of every stroke of this mask. The strokes are kept as painted.",
                ))
            })
            .when(can_mask || self.has_strokes(), |pane| {
                let shown = self.mask_shown;
                pane.child(Self::act(
                    if shown {
                        "Overlay on"
                    } else {
                        "Overlay off"
                    },
                    "mask-shown",
                    shown,
                    cx,
                    move |this| this.mask_shown = !shown,
                ))
                .children(Self::help(
                    "The overlay marks where the selected row's mask is painted. Hiding it does not change the mask.",
                ))
            })
            .when(!can_mask, |pane| {
                pane.children(Self::help(
                    "Mask brush: select a layer, a layer's adjustment or a Finish step.",
                ))
            })
    }

    // ---- the pane -------------------------------------------------------

    pub(super) fn inspector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let controls = match self.selected.clone() {
            _ if self.view_stretch_open => self.inspect_view_stretch(cx),
            Row::Input(channel) => self.inspect_input(&channel, cx),
            Row::InputStep { channel, step } => self.inspect_input_step(&channel, step, cx),
            Row::Layer(id) => match self.layer(id) {
                Some(layer) if self.selected_step_layer().is_some() => {
                    self.inspect_step_layer(layer, cx)
                }
                Some(layer) if layer.source.is_adjustment() => {
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
            .child(self.section("readout", "Canvas", false, cx, |this, _| {
                column().children(
                    this.readout()
                        .into_iter()
                        .map(|line| div().text_xs().child(line)),
                )
            }))
    }
}
