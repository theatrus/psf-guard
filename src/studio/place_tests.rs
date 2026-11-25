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
    assert_eq!(layers, ["SII as red", "Ha as green", "OIII as blue"]);
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
    assert_eq!(offered(&["ha", "oiii"]), ["HOO", "HOO, red and teal"]);
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
    assert_eq!(layers[0].name, "Ha as red");
    assert!(layers[0].mask.is_some());
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

    let layer = document.detach_adjustment(step).unwrap();
    let layers = document.layers();
    assert_eq!(layers.len(), 4);
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
        "Ha · as stacked · preview stretch, not saved"
    );
    let before = stretched.before(&document);
    assert_ne!(before, stretched);
    assert!(before
        .label(&document)
        .ends_with("preview stretch, not saved"));
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

    let all = panes(Split::AllInputs, &green, &document);
    assert_eq!(all.len(), 3);
    assert_eq!(all[1], green);
    assert!(matches!(&all[0], Place::Input { channel, after: Some(_) } if channel == "red"));

    let pair = panes(Split::InputAndPicture, &green, &document);
    assert_eq!(pair, [green.clone(), Place::Final]);

    let layer = document.layers()[2].id;
    let selected = Place::Layer {
        layer,
        alone: false,
    };
    let pair = panes(Split::InputAndPicture, &selected, &document);
    assert!(matches!(&pair[0], Place::Input { channel, after: Some(_) } if channel == "blue"));
    assert_eq!(pair[1], selected);

    let pair = panes(Split::BeforeAndAfter, &Place::Final, &document);
    assert_eq!(pair, [Place::Combined, Place::Final]);
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
