use super::flow_operations as op;
use super::*;
use serde_json::Value;
use uuid::Uuid;

fn document(channels: &[&str]) -> Document {
    let settings: Value =
        serde_json::from_str(include_str!("../processing_setups/default_color.json")).unwrap();
    document_from_color_setup("Default", &settings, channels).unwrap()
}

fn names(channels: &[&str]) -> Vec<String> {
    channels.iter().map(|channel| channel.to_string()).collect()
}

fn adjustment(name: &str) -> LayerRecord {
    LayerRecord::new(
        name,
        LayerSource::Adjustment {
            operation: name.to_string(),
            settings: serde_json::json!({ "amount": 0.7 }),
        },
    )
}

#[test]
fn a_narrowband_document_gets_narrowband_layers() {
    let document = document(&["ha", "oiii", "sii"]);
    let layers: Vec<&str> = document
        .layers()
        .iter()
        .map(|layer| layer.name.as_str())
        .collect();
    assert_eq!(layers, ["OIII as blue", "Ha as green", "SII as red"]);
    assert!(document.uses_palette(&PALETTES[2]));
}

#[test]
fn palettes_are_offered_for_the_inputs_a_document_has() {
    let offered = |channels: &[&str]| -> Vec<&str> {
        palettes_for(&names(channels))
            .into_iter()
            .map(|palette| palette.name)
            .collect()
    };
    assert_eq!(offered(&["red", "green", "blue"]), ["RGB"]);
    assert_eq!(
        offered(&["luminance", "red", "green", "blue"]),
        ["RGB", "LRGB"]
    );
    assert_eq!(
        offered(&["ha", "oiii"]),
        ["HOO", "HOO, red and teal", "HOO, Foraxx"]
    );
    assert_eq!(
        default_palette(&names(&["luminance", "red", "green", "blue"]))
            .unwrap()
            .name,
        "LRGB"
    );
}

#[test]
fn a_palette_of_any_hues_sums_to_full_scale() {
    let mut document = document(&["ha", "oiii", "sii"]);
    let palette = PALETTES
        .iter()
        .find(|palette| palette.name == "SHO, gold and teal")
        .unwrap();
    document.apply_palette(palette);
    assert!(document.uses_palette(palette));
    let sum = document.layers().iter().fold([0.0f32; 3], |sum, layer| {
        let color = layer.color.unwrap();
        [sum[0] + color[0], sum[1] + color[1], sum[2] + color[2]]
    });
    let brightest = sum.iter().copied().fold(0.0, f32::max);
    assert!((brightest - 1.0).abs() < 1e-5, "{sum:?}");
    assert_eq!(document.layers()[1].name, "Ha as gold");
}

#[test]
fn another_palette_keeps_a_layers_mask_and_the_layers_above() {
    let mut document = document(&["ha", "oiii", "sii"]);
    let ha = document.layers()[1].id;
    document
        .paint_layer_mask(
            ha,
            Stroke {
                points: vec![[1.0, 1.0]],
                radius: 4.0,
                hardness: 0.5,
                mode: StrokeMode::Add,
            },
        )
        .unwrap();
    document
        .insert_layer(usize::MAX, adjustment("Exposure"))
        .unwrap();
    document.apply_palette(&PALETTES[4]);
    let layers = document.layers();
    assert_eq!(layers.len(), 4);
    // HSO reads Ha, SII, OIII from the top.
    assert_eq!(layers[2].name, "Ha as red");
    assert!(layers[2].mask.is_some());
    assert_eq!(layers[3].name, "Exposure");
}

#[test]
fn hue_and_color_convert_both_ways() {
    for (hue, saturation, value) in [(0.0, 1.0, 1.0), (45.0, 0.9, 0.6), (195.0, 0.5, 1.0)] {
        let (h, s, v) = hue_of(color_from_hue(hue, saturation, value));
        assert!(
            (h - hue).abs() < 1e-3 && (s - saturation).abs() < 1e-4 && (v - value).abs() < 1e-5
        );
    }
    assert_eq!(hue_of([0.5; 3]), (0.0, 0.0, 0.5));
}

#[test]
fn an_adjustment_belongs_to_one_layer_or_to_the_stack() {
    let mut document = document(&["red", "green", "blue"]);
    let red = document.layers()[0].id;
    let exposure = adjustment("Exposure");
    let id = exposure.id;
    document.insert_layer(1, exposure).unwrap();
    assert_eq!(document.layers().len(), 4);

    let step = document.attach_adjustment(id, red).unwrap();
    assert_eq!(document.layers().len(), 3);
    assert_eq!(document.layer_of_step(step), Some(red));
    let (stage, own) = document.step(step).unwrap();
    assert_eq!((stage, own.kind.name()), (StageKind::Combine, "Exposure"));

    // The layer's own adjustment moves with the layer.
    document.move_layer(red, 2);
    assert_eq!(document.layers()[2].steps.len(), 1);

    // What was set on the layer's own adjustment comes back with it.
    assert!(document.set_step_opacity(step, 0.4));
    let layer = document.detach_adjustment(step).unwrap();
    let layers = document.layers();
    assert_eq!(layers.len(), 4);
    assert_eq!(layers[3].opacity, 0.4);
    assert_eq!(layers[3].id, layer);
    assert!(layers[2].steps.is_empty());
    assert_eq!(
        layers[3].source,
        LayerSource::Adjustment {
            operation: "Exposure".to_string(),
            settings: serde_json::json!({ "amount": 0.7 }),
        }
    );
}

#[test]
fn an_adjustment_cannot_belong_to_another_adjustment() {
    let mut document = document(&["red", "green", "blue"]);
    let (first, second) = (adjustment("Exposure"), adjustment("Curves"));
    let (a, b) = (first.id, second.id);
    document.insert_layer(usize::MAX, first).unwrap();
    document.insert_layer(usize::MAX, second).unwrap();
    assert!(document.attach_adjustment(a, b).is_err());
    assert!(document
        .attach_adjustment(document.layers()[0].id, document.layers()[1].id)
        .is_err());
    assert_eq!(document.layers().len(), 5);
}

#[test]
fn an_input_lists_its_own_steps_and_the_shared_ones_it_has_not_replaced() {
    let mut document = document(&["red", "green", "blue"]);
    let named = |document: &Document, channel: &str| -> Vec<(String, bool)> {
        input_steps(document, channel)
            .into_iter()
            .map(|row| {
                let (_, step) = document.step(row.id).unwrap();
                (step.kind.name().to_string(), row.shared)
            })
            .collect()
    };
    let before = named(&document, "green");
    assert!(before.contains(&(op::BACKGROUND.to_string(), true)));
    assert!(before.contains(&(op::STRETCH.to_string(), false)));

    let shared = input_steps(&document, "green")
        .into_iter()
        .find(|row| document.step(row.id).unwrap().1.kind.name() == op::BACKGROUND)
        .unwrap()
        .id;
    own_step_for_channel(&mut document, shared, "green").unwrap();
    let after = named(&document, "green");
    assert!(after.contains(&(op::BACKGROUND.to_string(), false)));
    assert!(!after.contains(&(op::BACKGROUND.to_string(), true)));
    // The other inputs still follow the shared step.
    assert!(named(&document, "red").contains(&(op::BACKGROUND.to_string(), true)));
}

#[test]
fn a_place_before_the_stretch_is_labelled_as_a_preview() {
    let document = document(&["ha", "oiii", "sii"]);
    let steps = input_steps(&document, "ha");
    let last = steps.last().unwrap().id;
    let stretched = Place::Input {
        channel: "ha".to_string(),
        after: Some(last),
    };
    assert_eq!(stretched.label(&document), "Ha · after Stretch");
    assert_eq!(
        Row::Input("ha".to_string()).place(false).label(&document),
        "Ha · as stacked · view stretch, not saved"
    );
    let before = stretched.before(&document);
    assert_ne!(before, stretched);

    // A second stretch of the input is told from the first.
    let mut twice = document.clone();
    let (stage, first) = twice.step(last).unwrap();
    let second = StepRecord::new(first.kind.clone()).for_band("ha");
    let second_id = second.id;
    twice.insert_step(stage, usize::MAX, second).unwrap();
    assert_eq!(stretched.label(&twice), "Ha · after Stretch 1");
    let again = Place::Input {
        channel: "ha".to_string(),
        after: Some(second_id),
    };
    assert_eq!(again.label(&twice), "Ha · after Stretch 2");
    assert_eq!(again.before(&twice), stretched);
    assert_eq!(
        plan(&twice, &again.view()).channels[0].stretch,
        PlannedStretch::Document(vec![last, second_id])
    );
    assert!(before.label(&document).ends_with("view stretch, not saved"));
}

#[test]
fn before_a_layer_is_the_stack_beneath_it() {
    let mut document = document(&["red", "green", "blue"]);
    let exposure = adjustment("Exposure");
    let id = exposure.id;
    document.insert_layer(usize::MAX, exposure).unwrap();
    let blue = document.layers()[2].id;
    let place = Row::Layer(id).place(false);
    assert_eq!(
        place.before(&document),
        Place::Layer {
            layer: blue,
            alone: false
        }
    );
    assert_eq!(Place::Final.before(&document), Place::Combined);
    let bottom = Place::Layer {
        layer: document.layers()[0].id,
        alone: false,
    };
    assert_eq!(bottom.before(&document), bottom);
}

#[test]
fn splits_choose_their_panes_from_the_selection() {
    let document = document(&["red", "green", "blue"]);
    let green = Row::Input("green".to_string()).place(false);
    assert_eq!(
        panes(Split::One, &green, &document),
        std::slice::from_ref(&green)
    );

    // The final picture is always the last pane, and is not repeated.
    assert_eq!(
        panes(Split::WithPicture, &green, &document),
        [green.clone(), Place::Final]
    );
    assert_eq!(
        panes(Split::WithPicture, &Place::Final, &document),
        [Place::Final]
    );
    let mask = Place::Mask(Uuid::new_v4());
    assert_eq!(
        panes(Split::WithPicture, &mask, &document),
        [mask.clone(), Place::Final]
    );
    assert_eq!(
        panes(Split::BeforeAndAfter, &Place::Final, &document),
        [Place::Combined, Place::Final]
    );
    assert_eq!(
        panes(Split::BeforeAndAfter, &Place::Combined, &document).last(),
        Some(&Place::Final)
    );
}

#[test]
fn a_step_of_one_layer_is_found_changed_and_removed_like_any_step() {
    let mut document = document(&["red", "green", "blue"]);
    let red = document.layers()[0].id;
    let exposure = adjustment("Exposure");
    let id = exposure.id;
    document.insert_layer(usize::MAX, exposure).unwrap();
    let step: Uuid = document.attach_adjustment(id, red).unwrap();
    assert!(document.set_step_settings(step, serde_json::json!({ "amount": 1.5 })));
    assert!(document.set_step_enabled(step, false));
    let removed = document.remove_step(step).unwrap();
    assert!(!removed.enabled);
    assert!(document.layers()[0].steps.is_empty());
}

#[test]
fn before_a_layers_own_edit_is_the_layer_without_it() {
    let mut document = document(&["red", "green", "blue"]);
    let red = document
        .layers()
        .iter()
        .find(|layer| layer.name == "R as red")
        .unwrap()
        .id;
    let mut steps = Vec::new();
    for name in ["Exposure", "Curves"] {
        let layer = adjustment(name);
        let id = layer.id;
        document.insert_layer(usize::MAX, layer).unwrap();
        steps.push(document.attach_adjustment(id, red).unwrap());
    }
    let own = |document: &Document, place: &Place| plan(document, &place.view()).own_steps;

    let second = Row::LayerStep {
        layer: red,
        step: steps[1],
    }
    .place(false);
    assert_eq!(own(&document, &second), steps);
    assert_eq!(second.label(&document), "R as red · alone · after Curves");

    let first = second.before(&document);
    assert_eq!(own(&document, &first), [steps[0]]);

    let none = first.before(&document);
    assert!(own(&document, &none).is_empty());
    assert_eq!(
        none.label(&document),
        "R as red · alone · before its own edits"
    );
    assert_eq!(none.before(&document), none);

    // A view of the whole stack applies every own edit that is on.
    document.set_step_enabled(steps[0], false);
    assert_eq!(own(&document, &Place::Combined), [steps[1]]);
    assert_eq!(
        panes(Split::Wipe, &second, &document),
        [second.before(&document), second.clone()]
    );
}

/// The stack's layers, bottom first, as "name" or "name in group".
fn stack_of(document: &Document) -> Vec<String> {
    document
        .layers()
        .iter()
        .map(|layer| match layer.group {
            Some(_) => format!("{} in group", layer.name),
            None => layer.name.clone(),
        })
        .collect()
}

#[test]
fn a_group_keeps_its_members_beneath_it_and_moves_with_them() {
    let mut document = document(&["red", "green", "blue"]);
    let ids: Vec<Uuid> = document.layers().iter().map(|layer| layer.id).collect();
    let group = document.group_layer(ids[1]).unwrap();
    assert_eq!(
        stack_of(&document),
        ["B as blue", "G as green in group", "Group", "R as red"]
    );
    // The group blends as the layer it was made around did.
    assert_eq!(document.layers()[2].blend, BlendMode::Add);
    // A group does not hold a group.
    assert!(document.group_layer(group).is_err());
    assert!(document.group_layer(ids[1]).is_err());

    // Red joins the group, on top of green.
    assert!(document.place_layer(ids[2], 2, Some(group)));
    assert_eq!(
        stack_of(&document),
        [
            "B as blue",
            "G as green in group",
            "R as red in group",
            "Group"
        ]
    );
    assert_eq!(document.group_members(group).count(), 2);

    // The group moves to the bottom with its members.
    assert!(document.place_layer(group, 0, None));
    assert_eq!(
        stack_of(&document),
        [
            "G as green in group",
            "R as red in group",
            "Group",
            "B as blue"
        ]
    );

    // Green leaves the group for the top of the stack.
    assert!(document.place_layer(ids[1], 4, None));
    assert_eq!(
        stack_of(&document),
        ["R as red in group", "Group", "B as blue", "G as green"]
    );

    assert!(document.ungroup(group));
    assert_eq!(stack_of(&document), ["R as red", "B as blue", "G as green"]);
    assert!(!document.ungroup(ids[0]));
    assert_eq!(document.layers().len(), 3);
}

#[test]
fn a_view_blends_a_group_on_its_own() {
    let mut document = document(&["red", "green", "blue"]);
    let ids: Vec<Uuid> = document.layers().iter().map(|layer| layer.id).collect();
    let group = document.group_layer(ids[1]).unwrap();
    document.place_layer(ids[2], 2, Some(group));
    let entries = |place: Place| -> Vec<&'static str> {
        plan(&document, &place.view())
            .stack
            .iter()
            .map(|entry| match entry {
                StackEntry::Band { .. } => "band",
                StackEntry::Adjustment { .. } => "adjustment",
                StackEntry::GroupBegin { .. } => "begin",
                StackEntry::GroupEnd { .. } => "end",
            })
            .collect()
    };
    assert_eq!(
        entries(Place::Combined),
        ["band", "begin", "band", "band", "end"]
    );
    // The group alone is its members and itself.
    let alone = Place::Layer {
        layer: group,
        alone: true,
    };
    assert_eq!(entries(alone.clone()), ["begin", "band", "band", "end"]);
    // A group on its own keeps its colors.
    assert!(!plan(&document, &alone.view()).gray);
    // A view that stops inside the group shows the group so far.
    let part = Place::Layer {
        layer: ids[1],
        alone: false,
    };
    assert_eq!(entries(part), ["band", "begin", "band", "end"]);
    // One member alone is shown as it is.
    let member = Place::Layer {
        layer: ids[1],
        alone: true,
    };
    assert_eq!(entries(member.clone()), ["band"]);
    // A member on its own is one input, drawn in gray.
    assert!(plan(&document, &member.view()).gray);
    // Before the group is the stack beneath its members.
    let whole = Place::Layer {
        layer: group,
        alone: false,
    };
    assert_eq!(
        whole.before(&document),
        Place::Layer {
            layer: ids[0],
            alone: false
        }
    );
}

#[test]
fn a_clipped_layer_is_clipped_to_the_layer_beneath_it() {
    let mut document = document(&["red", "green", "blue"]);
    let ids: Vec<Uuid> = document.layers().iter().map(|layer| layer.id).collect();
    let clip_of = |document: &Document, layer: Uuid| {
        plan(document, &Place::Combined.view())
            .stack
            .iter()
            .find_map(|entry| match entry {
                StackEntry::Band {
                    layer: own, clip, ..
                } if *own == layer => Some(*clip),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(clip_of(&document, ids[2]), None);

    // Two clipped layers share the unclipped layer beneath them.
    document.layer_mut(ids[1]).unwrap().clipped = true;
    document.layer_mut(ids[2]).unwrap().clipped = true;
    assert_eq!(clip_of(&document, ids[1]), Some(ids[0]));
    assert_eq!(clip_of(&document, ids[2]), Some(ids[0]));
    // The bottom layer has nothing to be clipped to.
    document.layer_mut(ids[0]).unwrap().clipped = true;
    assert_eq!(clip_of(&document, ids[0]), None);
    document.layer_mut(ids[0]).unwrap().clipped = false;

    // A layer above a group is clipped to the group, not to a member.
    document.layer_mut(ids[1]).unwrap().clipped = false;
    let group = document.group_layer(ids[1]).unwrap();
    assert_eq!(clip_of(&document, ids[2]), Some(group));
}

#[test]
fn a_foraxx_palette_mixes_inputs_by_their_signal() {
    let mut document = document(&["ha", "oiii", "sii"]);
    let foraxx = PALETTES
        .iter()
        .find(|palette| palette.name == "SHO, Foraxx")
        .unwrap();
    // A plain palette is the default; a person chooses a dynamic one.
    assert_eq!(
        default_palette(&names(&["ha", "oiii", "sii"]))
            .unwrap()
            .name,
        "SHO"
    );
    assert!(palettes_for(&names(&["ha", "oiii"]))
        .iter()
        .any(|palette| palette.name == "HOO, Foraxx"));

    document.apply_palette(foraxx);
    assert!(document.uses_palette(foraxx));
    assert!(!document.uses_palette(&PALETTES[2]));
    let layers = document.layers();
    assert_eq!(layers.len(), 5);
    // Each layer keeps its full color: masks that sum to 1 share red and
    // green between two inputs.
    assert!(layers.iter().all(|layer| {
        let color = layer.color.unwrap();
        color.iter().copied().fold(0.0, f32::max) == 1.0
    }));
    // Two masks serve four layers: each mask and its inverse.
    let masks = document.masks();
    assert_eq!(masks.len(), 2);
    assert_eq!(masks[0].name, "OIII signal");
    assert_eq!(masks[1].name, "Ha × OIII signal");
    assert_eq!(
        layers[4].mask,
        Some(MaskRecord::Shared {
            mask: masks[0].id,
            inverted: false,
        })
    );
    assert_eq!(
        layers[3].mask,
        Some(MaskRecord::Shared {
            mask: masks[0].id,
            inverted: true,
        })
    );
    assert_eq!(document.layers_using_mask(masks[1].id).count(), 2);
    assert_eq!(layers[0].mask, None);

    // Back to a plain palette, the shared masks are gone.
    document.apply_palette(&PALETTES[2]);
    assert!(document.layers().iter().all(|layer| layer.mask.is_none()));
    assert!(document.masks().is_empty());

    // The power of inverted pixels lifts faint signal and keeps the ends.
    assert_eq!(MaskShape::PowerOfInverse.apply(0.0), 0.0);
    assert_eq!(MaskShape::PowerOfInverse.apply(1.0), 1.0);
    assert!(MaskShape::PowerOfInverse.apply(0.1) > 0.1);
    assert_eq!(MaskShape::Value.apply(0.25), 0.25);
}

#[test]
fn a_view_lists_every_input_for_computed_masks() {
    let document = document(&["ha", "oiii", "sii"]);
    let first = document.layers()[0].id;
    let alone = Place::Layer {
        layer: first,
        alone: true,
    };
    let planned = plan(&document, &alone.view());
    assert_eq!(planned.channels.len(), 1);
    let inputs: Vec<&str> = planned
        .inputs
        .iter()
        .map(|input| input.channel.as_str())
        .collect();
    assert_eq!(inputs, ["ha", "oiii", "sii"]);
    assert!(planned
        .inputs
        .iter()
        .all(|input| matches!(input.stretch, PlannedStretch::Document(_))));
}

#[test]
fn another_input_for_a_slot_remakes_the_layers_and_masks_and_keeps_the_tweaks() {
    let mut document = document(&["ha", "oiii", "sii"]);
    let foraxx = PALETTES
        .iter()
        .find(|palette| palette.name == "SHO, Foraxx")
        .unwrap();
    document.apply_palette(foraxx);
    assert_eq!(foraxx.slots(), ["ha", "oiii", "sii"]);

    // A person tweaks a mask and a layer.
    let mask = document.masks()[0].id;
    document.mask_mut(mask).unwrap().reach = 0.6;
    assert!(document.touch_up_mask(
        mask,
        Stroke {
            points: vec![[10.0, 10.0]],
            radius: 5.0,
            hardness: 0.5,
            mode: StrokeMode::Subtract,
        }
    ));
    let sii_layer = document.layers()[4].id;
    document.layer_mut(sii_layer).unwrap().opacity = 0.5;

    // The OIII slot is given the SII input.
    assert!(document.set_palette_input("oiii", "sii"));
    let layers = document.layers();
    assert_eq!(layers.len(), 5);
    // The layer is the one it was.
    assert_eq!((layers[4].id, layers[4].opacity), (sii_layer, 0.5));
    // The layers that showed the slot show its new input.
    assert_eq!(layers[0].name, "SII as blue");
    assert!(matches!(&layers[1].source, LayerSource::Band { band } if band == "sii"));
    // The mask is the one it was, reading the new input, tweaks kept.
    let remade = document.mask(mask).unwrap();
    assert_eq!(remade.inputs, ["sii"]);
    assert_eq!(remade.name, "SII signal");
    assert_eq!((remade.reach, remade.touch_ups.len()), (0.6, 1));
    assert_eq!(document.masks()[1].inputs, ["ha", "sii"]);
    assert_eq!(document.palette.as_ref().unwrap().inputs["oiii"], "sii");
    assert!(document.uses_palette(foraxx));

    // A slot the palette does not have is refused.
    assert!(!document.set_palette_input("red", "ha"));

    // Removing a mask leaves its layers showing everywhere.
    document.remove_mask(mask);
    assert!(document.layers()[4].mask.is_none());
    assert_eq!(document.masks().len(), 1);
}

#[test]
fn the_stars_of_an_input_are_stretched_as_the_input_is() {
    let document = document(&["ha", "oiii", "sii"]);
    let stretch = |channel: &str| {
        let view = View {
            subject: Subject::Channel(channel.to_string()),
            up_to: UpTo::Final,
            stretch: DisplayStretch::Document,
            own_until: None,
        };
        plan(&document, &view).channels[0].stretch.clone()
    };
    assert!(matches!(stretch("ha"), PlannedStretch::Document(_)));
    assert_eq!(stretch("ha@stars"), stretch("ha"));
    assert_eq!(channel_label("ha@stars"), "Ha stars");
    // A palette leaves the stars layer where it is.
    let mut layered = document.clone();
    let mut stars = LayerRecord::new(
        "Ha stars",
        LayerSource::Band {
            band: "ha@stars".to_string(),
        },
    );
    stars.blend = BlendMode::Screen;
    let stars_id = stars.id;
    layered.insert_layer(usize::MAX, stars).unwrap();
    layered.apply_palette(&PALETTES[4]);
    assert_eq!(layered.layers().last().unwrap().id, stars_id);
    assert!(layered.uses_palette(&PALETTES[4]));

    // PSF Guard's settings have no place for the stars input.
    let mut with_stars = document.clone();
    let stars = StepRecord::new(StepKind::Operation {
        operation: op::STRETCH.to_string(),
        accepts: crate::image_steps::Accepts::Linear,
        settings: serde_json::json!({ "model": { "type": "identity" } }),
    })
    .for_band("ha@stars");
    with_stars
        .insert_step(StageKind::Stretch, usize::MAX, stars)
        .unwrap();
    assert_eq!(
        color_setup_from_document(&with_stars).unwrap(),
        color_setup_from_document(&document).unwrap()
    );
}

#[test]
fn a_color_groups_tools_belong_to_its_inputs_and_stay_out_of_psf_guards_setup() {
    let mut document = document(&["red", "green", "blue", "ha", "oiii"]);
    let before = color_setup_from_document(&document).unwrap();
    let chain = serde_json::json!({ "steps": [{ "provider": "sxt", "parameters": {} }] });
    let group_step = |band: &str| {
        let mut step = StepRecord::new(StepKind::Operation {
            operation: op::RC_ASTRO_COLOR.to_string(),
            accepts: crate::image_steps::Accepts::Linear,
            settings: chain.clone(),
        })
        .for_band(band);
        step.enabled = true;
        step
    };
    let rgb = group_step("rgb");
    let lines = group_step("lines");
    let (rgb_id, lines_id) = (rgb.id, lines.id);
    document.insert_step(StageKind::Linear, 0, rgb).unwrap();
    document.insert_step(StageKind::Linear, 0, lines).unwrap();
    let listed = |channel: &str| -> Vec<Uuid> {
        input_steps(&document, channel)
            .into_iter()
            .map(|row| row.id)
            .collect()
    };
    for channel in ["red", "green", "blue"] {
        assert!(listed(channel).contains(&rgb_id), "{channel}");
        assert!(!listed(channel).contains(&lines_id), "{channel}");
    }
    for channel in ["ha", "oiii"] {
        assert!(listed(channel).contains(&lines_id), "{channel}");
        assert!(!listed(channel).contains(&rgb_id), "{channel}");
    }
    // PSF Guard removes stars on each channel stack alone: the groups'
    // steps are not part of the color setup.
    assert_eq!(color_setup_from_document(&document).unwrap(), before);
}

/// The layers' names, top first, as the layer list shows them.
fn listed(document: &Document) -> Vec<String> {
    document
        .layers()
        .iter()
        .rev()
        .map(|layer| layer.name.clone())
        .collect()
}

#[test]
fn a_palette_reads_top_down_in_its_order_with_luminance_on_top() {
    assert_eq!(
        listed(&document(&["red", "green", "blue"])),
        ["R as red", "G as green", "B as blue"]
    );
    // A luminance layer does not add, so it stays on top.
    assert_eq!(
        listed(&document(&["red", "green", "blue", "luminance"])),
        ["L as luminance", "R as red", "G as green", "B as blue"]
    );
}

#[test]
fn a_document_saved_bottom_up_reads_top_down_when_opened() {
    let made = document(&["ha", "oiii", "sii"]);
    // Before, a palette stacked its layers in its own order from the
    // bottom, so SHO read OIII, Ha, SII from the top.
    let mut saved: Value = serde_json::to_value(&made).unwrap();
    let combine = saved["stages"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|stage| stage["kind"] == "combine")
        .unwrap();
    combine["layers"].as_array_mut().unwrap().reverse();
    let mut old: Document = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(listed(&old), ["OIII as blue", "Ha as green", "SII as red"]);
    assert!(old.list_palette_layers_top_down());
    assert_eq!(listed(&old), ["SII as red", "Ha as green", "OIII as blue"]);
    // Done once: a second pass leaves the layers as they are.
    assert!(!old.list_palette_layers_top_down());

    // A run with a clipped layer is left as it was: clipping depends on
    // the order.
    let mut clipped: Document = serde_json::from_value(saved).unwrap();
    let top = clipped.layers().last().unwrap().id;
    clipped.layer_mut(top).unwrap().clipped = true;
    assert!(!clipped.list_palette_layers_top_down());
    assert_eq!(
        listed(&clipped),
        ["OIII as blue", "Ha as green", "SII as red"]
    );
}

#[test]
fn a_layer_steps_past_its_neighbour_and_a_group_moves_whole() {
    // Bottom first: R as red, then a group of G and B, then an adjustment.
    let mut document = document(&["red", "green", "blue"]);
    let ids: Vec<Uuid> = document.layers().iter().map(|layer| layer.id).collect();
    // Stored bottom first as B, G, R; R is on top.
    let (blue, green, red) = (ids[0], ids[1], ids[2]);
    let group = document.group_layer(green).unwrap();
    document.place_layer(blue, 2, Some(group));
    let exposure = adjustment("Exposure");
    document.insert_layer(usize::MAX, exposure.clone()).unwrap();
    assert_eq!(
        listed(&document),
        ["Exposure", "R as red", "Group", "B as blue", "G as green"]
    );

    // A member moves within its group, and stops at its ends.
    assert!(document.step_layer(green, true));
    assert_eq!(listed(&document)[3..], ["G as green", "B as blue"]);
    assert!(!document.step_layer(green, true));

    // A top-level layer steps past a whole group.
    assert!(document.step_layer(red, false));
    assert_eq!(
        listed(&document),
        ["Exposure", "Group", "G as green", "B as blue", "R as red"]
    );
    assert!(!document.step_layer(red, false));

    // A group moves with its members, past the next top-level entry.
    assert!(document.step_layer(group, true));
    assert_eq!(
        listed(&document),
        ["Group", "G as green", "B as blue", "Exposure", "R as red"]
    );
    assert!(!document.step_layer(group, true));
    assert!(document.step_layer(group, false));
    assert!(document.step_layer(group, false));
    assert_eq!(
        listed(&document),
        ["Exposure", "R as red", "Group", "G as green", "B as blue"]
    );
}
