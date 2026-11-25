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
