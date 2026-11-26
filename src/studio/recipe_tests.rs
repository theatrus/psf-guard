use serde_json::Value;
use uuid::Uuid;

use super::*;

fn document(channels: &[&str]) -> Document {
    let settings: Value =
        serde_json::from_str(include_str!("../processing_setups/default_color.json")).unwrap();
    document_from_color_setup("Default", &settings, channels).unwrap()
}

fn names(channels: &[&str]) -> Vec<String> {
    channels.iter().map(|channel| channel.to_string()).collect()
}

fn recipe(name: &str) -> &'static Recipe {
    RECIPES.iter().find(|recipe| recipe.name == name).unwrap()
}

/// What each part shows, bottom part first: its name and its layers' bands.
fn parts(document: &Document) -> Vec<(String, Vec<String>)> {
    document
        .parts()
        .into_iter()
        .map(|part| {
            let shown = document
                .group_members(part.id)
                .filter_map(|layer| match &layer.source {
                    LayerSource::Band { band } => Some(band.clone()),
                    _ => None,
                })
                .collect();
            (part.name.clone(), shown)
        })
        .collect()
}

#[test]
fn the_recipes_offered_are_those_the_inputs_can_fill() {
    let offered = |channels: &[&str]| -> Vec<&str> {
        recipes_for(&names(channels))
            .into_iter()
            .map(|recipe| recipe.name)
            .collect()
    };
    assert_eq!(
        offered(&["ha", "oiii", "sii"]),
        ["Narrowband with its own stars", "Narrowband"]
    );
    assert_eq!(offered(&["red", "green", "blue"]), ["RGB"]);
    assert!(
        offered(&["red", "green", "blue", "luminance", "ha"]).contains(&"LRGB with emission lines")
    );
    assert!(offered(&["ha", "oiii", "red", "green", "blue"]).contains(&"Narrowband with RGB stars"));
    assert!(!offered(&["ha", "oiii"]).contains(&"Narrowband with RGB stars"));
}

#[test]
fn a_recipe_makes_its_parts_and_keeps_adjustment_layers_above_them() {
    let inputs = names(&["ha", "oiii", "red", "green", "blue"]);
    let mut document = document(&["ha", "oiii", "red", "green", "blue"]);
    let curves = LayerRecord::new(
        "Curves",
        LayerSource::Adjustment {
            operation: "Curves".to_string(),
            settings: serde_json::json!({ "amount": 0.5 }),
        },
    );
    let curves_id = curves.id;
    document.insert_layer(usize::MAX, curves).unwrap();

    assert!(document.use_recipe(recipe("Narrowband with RGB stars"), &inputs));
    assert_eq!(
        parts(&document),
        [
            ("Nebula".to_string(), names(&["ha", "oiii"])),
            (
                "RGB stars".to_string(),
                names(&["red@stars", "green@stars", "blue@stars"])
            ),
        ]
    );
    assert_eq!(document.layers().last().unwrap().id, curves_id);
    let stars = document.parts()[1];
    assert_eq!(stars.blend, BlendMode::Screen);
    assert_eq!(stars.palette.as_ref().unwrap().name, "RGB");
    assert_eq!(document.parts()[0].palette.as_ref().unwrap().name, "HOO");

    // RGB stars bring the Star stretch step, once, and it is listed for
    // the stars of red, green and blue only.
    let star_stretches = |document: &Document| {
        document
            .stage(StageKind::Stretch)
            .steps
            .iter()
            .filter(|step| step.kind.name() == flow_operations::STAR_STRETCH)
            .map(|step| step.id)
            .collect::<Vec<_>>()
    };
    let stretch = star_stretches(&document);
    assert_eq!(stretch.len(), 1);
    let listed = |channel: &str| {
        input_steps(&document, channel)
            .iter()
            .any(|step| step.id == stretch[0])
    };
    assert!(listed("red@stars") && listed("blue@stars"));
    assert!(!listed("red") && !listed("ha") && !listed("ha@stars"));
    let mut again = document.clone();
    assert!(again.use_recipe(recipe("Narrowband with RGB stars"), &inputs));
    assert_eq!(star_stretches(&again).len(), 1);

    // A recipe the inputs cannot fill changes nothing.
    let before = document.clone();
    assert!(!document.use_recipe(recipe("LRGB"), &inputs));
    assert_eq!(document.to_json(), before.to_json());
}

#[test]
fn an_input_joins_a_part_without_the_rest_changing() {
    let inputs = names(&["red", "green", "blue", "luminance", "ha", "nii"]);
    let mut document = document(&["red", "green", "blue", "luminance", "ha", "nii"]);
    assert!(document.use_recipe(recipe("LRGB"), &inputs));
    let color = document.parts()[0].id;
    let before: Vec<LayerRecord> = document.layers().to_vec();

    // Ha goes into the color of an LRGB picture.
    let (hue, saturation, blend) = Document::default_tone("ha");
    let ha = document.add_layer_showing("ha", Some(color), hue, saturation, blend);
    assert_eq!(
        parts(&document),
        [
            ("Color".to_string(), names(&["red", "green", "blue", "ha"])),
            ("Luminance".to_string(), names(&["luminance"])),
        ]
    );
    let added = document
        .layers()
        .iter()
        .find(|layer| layer.id == ha)
        .unwrap();
    assert_eq!((added.blend, added.group), (BlendMode::Screen, Some(color)));
    // Every other layer is as it was.
    let rest: Vec<&LayerRecord> = document
        .layers()
        .iter()
        .filter(|layer| layer.id != ha)
        .collect();
    assert_eq!(rest, before.iter().collect::<Vec<_>>());
    // The part still follows its palette, and the added layer stays when a
    // slot is given another input.
    assert!(document.set_palette_input_in(Some(color), "red", "nii"));
    assert_eq!(
        parts(&document)[0].1,
        names(&["nii", "green", "blue", "ha"])
    );

    // An input can also go on top of everything, in no part.
    let nii = document.add_layer_showing("nii", None, 12.0, 1.0, BlendMode::Screen);
    assert_eq!(document.layers().last().unwrap().id, nii);

    let routes = document.routes();
    let of = |input: &str| -> Vec<Option<Uuid>> {
        routes
            .iter()
            .filter(|route| route.input == input)
            .map(|route| route.part)
            .collect()
    };
    assert_eq!(of("ha"), [Some(color)]);
    assert_eq!(of("nii"), [Some(color), None]);
    assert!(of("red").is_empty());
    let slot = routes.iter().find(|route| route.shows == "nii").unwrap();
    assert_eq!(slot.slot.as_deref(), Some("red"));
}

#[test]
fn two_parts_keep_a_palette_and_masks_each() {
    let inputs = names(&["ha", "oiii", "sii"]);
    let mut document = document(&["ha", "oiii", "sii"]);
    assert!(document.use_recipe(recipe("Narrowband"), &inputs));
    let first = document.parts()[0].id;
    let second = document.add_part(PartKind::Nebula, &inputs).unwrap();
    let foraxx = PALETTES
        .iter()
        .find(|palette| palette.name == "SHO, Foraxx")
        .unwrap();
    document.apply_palette_in(Some(first), foraxx, &Default::default());
    document.apply_palette_in(Some(second), foraxx, &Default::default());
    assert_eq!(document.masks().len(), 4);
    assert!(document.set_palette_input_in(Some(second), "oiii", "sii"));
    let reads = |part: Uuid| -> Vec<Vec<String>> {
        document
            .masks()
            .iter()
            .filter(|mask| mask.part == Some(part))
            .map(|mask| mask.inputs.clone())
            .collect()
    };
    assert_eq!(reads(first), [names(&["oiii"]), names(&["ha", "oiii"])]);
    assert_eq!(reads(second), [names(&["sii"]), names(&["ha", "sii"])]);
    assert!(document.uses_palette_in(Some(first), foraxx));
    assert_eq!(parts(&document).len(), 2);

    // The document survives a round trip with its parts' palettes.
    let again = Document::from_json(&document.to_json()).unwrap();
    assert_eq!(again.to_json(), document.to_json());
}

#[test]
fn every_kind_of_part_is_made_from_inputs_that_fit_it() {
    let inputs = names(&["red", "green", "blue", "luminance", "ha", "oiii"]);
    for kind in PartKind::ALL {
        let mut document = document(&["red", "green", "blue", "luminance", "ha", "oiii"]);
        let layers = document.layers().len();
        let part = document.add_part(kind, &inputs).unwrap();
        assert_eq!(document.layers().last().unwrap().id, part, "{kind:?}");
        let members = document.group_members(part).count();
        assert_eq!(document.layers().len(), layers + members + 1);
        assert_eq!(members == 0, kind == PartKind::Empty, "{kind:?}");
    }
    let mut narrow = document(&["ha"]);
    assert!(narrow.add_part(PartKind::Color, &names(&["ha"])).is_none());
    assert!(narrow
        .add_part(PartKind::Luminance, &names(&["ha"]))
        .is_none());
}

#[test]
fn a_palette_keeps_each_layer_by_its_slot_however_they_are_ordered() {
    let inputs = names(&["ha", "oiii", "sii", "nii"]);
    let mut document = document(&["ha", "oiii", "sii", "nii"]);
    assert!(document.use_recipe(recipe("Narrowband"), &inputs));
    let part = document.parts()[0].id;
    let layer_of = |document: &Document, slot: &str| -> LayerRecord {
        document
            .group_members(part)
            .find(|layer| layer.slot.as_deref() == Some(slot))
            .unwrap()
            .clone()
    };
    // A person reorders the part's layers and changes one's opacity.
    let ha = layer_of(&document, "ha");
    let first = document
        .layers()
        .iter()
        .position(|layer| layer.group == Some(part))
        .unwrap();
    document.place_layer(ha.id, first, Some(part));
    document.layer_mut(ha.id).unwrap().opacity = 0.4;
    let colors = ["ha", "oiii", "sii"].map(|slot| layer_of(&document, slot).color);

    // Giving another slot a new input keeps every layer's color and the
    // opacity, each with the slot it had.
    assert!(document.set_palette_input_in(Some(part), "oiii", "nii"));
    assert_eq!(
        ["ha", "oiii", "sii"].map(|slot| layer_of(&document, slot).color),
        colors
    );
    assert_eq!(layer_of(&document, "ha").opacity, 0.4);
    assert!(matches!(
        &layer_of(&document, "oiii").source,
        LayerSource::Band { band } if band == "nii"
    ));
}

#[test]
fn a_layer_that_leaves_its_part_is_not_replaced_by_another_palette() {
    let inputs = names(&["red", "green", "blue", "ha", "oiii"]);
    let mut document = document(&["red", "green", "blue", "ha", "oiii"]);
    assert!(document.use_recipe(recipe("RGB"), &inputs));
    let color = document.parts()[0].id;
    let nebula = document.add_part(PartKind::Nebula, &inputs).unwrap();
    let ha = document
        .group_members(nebula)
        .find(|layer| layer.slot.as_deref() == Some("ha"))
        .unwrap()
        .id;
    // Ha moves into the Color part, on top of its layers.
    let top = document
        .layers()
        .iter()
        .position(|layer| layer.id == color)
        .unwrap();
    assert!(document.place_layer(ha, top, Some(color)));
    assert!(document
        .layers()
        .iter()
        .any(|layer| layer.id == ha && layer.slot.is_none()));
    let red_color = document
        .group_members(color)
        .find(|layer| layer.slot.as_deref() == Some("red"))
        .unwrap()
        .color;

    // The Color part's palette changes an input: Ha stays, and red keeps
    // its color.
    assert!(document.set_palette_input_in(Some(color), "blue", "oiii"));
    assert!(document.layers().iter().any(|layer| layer.id == ha));
    assert_eq!(
        document
            .group_members(color)
            .find(|layer| layer.slot.as_deref() == Some("red"))
            .unwrap()
            .color,
        red_color
    );

    // Ungrouping leaves the layers no palette's.
    assert!(document.ungroup(color));
    assert!(document
        .layers()
        .iter()
        .filter(|layer| layer.group.is_none())
        .all(|layer| layer.slot.is_none()));

    // A recipe without RGB stars has no Star stretch step.
    let mut stars = document.clone();
    stars.add_star_stretch();
    assert!(stars.use_recipe(recipe("RGB"), &inputs));
    assert!(stars
        .stage(StageKind::Stretch)
        .steps
        .iter()
        .all(|step| step.kind.name() != flow_operations::STAR_STRETCH));
}

#[test]
fn edits_keep_groups_whole_and_masks_with_what_uses_them() {
    let inputs = names(&["ha", "oiii", "sii"]);
    let mut document = document(&["ha", "oiii", "sii"]);
    assert!(document.use_recipe(recipe("Narrowband"), &inputs));
    let part = document.parts()[0].id;
    let member = document.group_members(part).next().unwrap().id;
    let curves = StepRecord::new(StepKind::Operation {
        operation: "Curves".to_string(),
        accepts: crate::image_steps::Accepts::Display,
        settings: serde_json::json!({}),
    });
    let step = curves.id;
    document.insert_layer_step(member, curves).unwrap();

    // A member's adjustment made a layer stays in the member's group, and
    // the group stays one block.
    let made = document.detach_adjustment(step).unwrap();
    let layers = document.layers();
    let group_at = layers.iter().position(|layer| layer.id == part).unwrap();
    let made_at = layers.iter().position(|layer| layer.id == made).unwrap();
    assert_eq!(layers[made_at].group, Some(part));
    assert!(made_at < group_at);
    assert!(layers[made_at..group_at]
        .iter()
        .all(|layer| layer.group == Some(part)));

    // A kept palette layer keeps its place when a slot changes input.
    let ha = document
        .group_members(part)
        .find(|layer| layer.slot.as_deref() == Some("ha"))
        .unwrap()
        .id;
    let top_of_part = group_at - 1;
    document.place_layer(ha, top_of_part + 1, Some(part));
    let before = document.layers().iter().position(|layer| layer.id == ha);
    assert!(document.set_palette_input_in(Some(part), "sii", "oiii"));
    assert_eq!(
        document.layers().iter().position(|layer| layer.id == ha),
        before
    );

    // A shared mask used only by a layer's own step is in use, and removing
    // it clears the step's mask.
    let mask = document.add_mask(SharedMask::new(names(&["ha"])));
    let own = StepRecord::new(StepKind::Operation {
        operation: "Curves".to_string(),
        accepts: crate::image_steps::Accepts::Display,
        settings: serde_json::json!({}),
    });
    let own_id = own.id;
    let mut own = own;
    own.mask = Some(MaskRecord::Shared {
        mask,
        inverted: false,
    });
    document.insert_layer_step(ha, own).unwrap();
    assert!(document.mask_in_use(mask));
    document.remove_mask(mask);
    assert!(document.step(own_id).unwrap().1.mask.is_none());

    // Removing the part removes its layers and the masks its palette made.
    let foraxx = PALETTES
        .iter()
        .find(|palette| palette.name == "SHO, Foraxx")
        .unwrap();
    document.apply_palette_in(Some(part), foraxx, &Default::default());
    assert!(!document.masks().is_empty());
    assert!(document.remove_part(part));
    assert!(document.parts().is_empty());
    assert!(document.masks().iter().all(|mask| mask.part != Some(part)));
}

#[test]
fn emission_lines_use_each_line_with_its_continuum_subtracted_when_there_is_one() {
    let inputs = names(&["red", "green", "blue", "ha", "oiii"]);
    let mut document = document(&["red", "green", "blue", "ha", "oiii"]);
    let line = StepRecord::new(StepKind::Operation {
        operation: flow_operations::CHANNEL_STACK.to_string(),
        accepts: crate::image_steps::Accepts::Linear,
        settings: serde_json::json!({}),
    })
    .for_band("ha@line");
    document
        .insert_step(StageKind::Sources, usize::MAX, line)
        .unwrap();
    let part = document.add_part(PartKind::Lines, &inputs).unwrap();
    let shown: Vec<String> = document
        .group_members(part)
        .filter_map(|layer| match &layer.source {
            LayerSource::Band { band } => Some(band.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(shown, names(&["ha@line", "oiii"]));
    assert_eq!(channel_label("ha@line"), "Ha line");
    assert_eq!(
        Document::default_tone("ha@line").0,
        Document::default_tone("ha").0
    );
}

#[test]
fn a_color_recipe_brings_back_the_stars_a_star_removal_took() {
    let inputs = names(&["red", "green", "blue", "ha"]);
    let mut document = document(&["red", "green", "blue", "ha"]);
    // Without the stars of R, G and B, RGB is the color part alone.
    assert!(document.use_recipe(recipe("RGB"), &inputs));
    assert_eq!(parts(&document).len(), 1);
    for name in ["red", "green", "blue"] {
        let stars = StepRecord::new(StepKind::Operation {
            operation: flow_operations::CHANNEL_STACK.to_string(),
            accepts: crate::image_steps::Accepts::Linear,
            settings: serde_json::json!({}),
        })
        .for_band(&format!("{name}@stars"));
        document
            .insert_step(StageKind::Sources, usize::MAX, stars)
            .unwrap();
    }
    assert!(document.use_recipe(recipe("RGB with emission lines"), &inputs));
    let made: Vec<String> = parts(&document).into_iter().map(|(name, _)| name).collect();
    assert_eq!(made, ["Color", "Emission lines", "RGB stars"]);
    let lines = document
        .parts()
        .into_iter()
        .find(|part| part.name == "Emission lines")
        .unwrap();
    assert_eq!(lines.opacity, 0.5);
}
