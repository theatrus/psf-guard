use super::flow_operations as op;
use super::*;
use serde_json::{json, Map, Value};

/// The settings of PSF Guard's built-in default color setup.
fn default_settings() -> Value {
    serde_json::from_str(include_str!("../processing_setups/default_color.json")).unwrap()
}

/// A value that stands for "not set": absent, null, or an empty map or list.
fn is_unset(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Object(map)) => map.is_empty(),
        Some(Value::Array(list)) => list.is_empty(),
        Some(_) => false,
    }
}

/// Whether two sets of settings mean the same: equal once every key that
/// stands for "not set" is left out. PSF Guard writes an empty map for some
/// unset settings and leaves others out.
fn same_settings(a: &Value, b: &Value) -> bool {
    fn set_keys(value: &Value) -> Map<String, Value> {
        value
            .as_object()
            .map(|map| {
                map.iter()
                    .filter(|(_, value)| !is_unset(Some(value)))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
    set_keys(a) == set_keys(b)
}

const SHO: [&str; 3] = ["sii", "ha", "oiii"];

fn names(document: &Document, stage: StageKind) -> Vec<String> {
    document
        .stage(stage)
        .steps
        .iter()
        .map(|step| match &step.band {
            Some(band) => format!("{} ({band})", step.kind.name()),
            None => step.kind.name().to_string(),
        })
        .collect()
}

#[test]
fn the_default_setup_becomes_psf_guards_flow() {
    let document = document_from_color_setup("Default", &default_settings(), &SHO).unwrap();

    assert_eq!(
        names(&document, StageKind::Sources),
        [
            "Channel stack (sii)",
            "Channel stack (ha)",
            "Channel stack (oiii)"
        ]
    );
    // The choice that inputs follow their cards, which decides their tools,
    // then the background, then color calibration: the order the pipeline's
    // phases run.
    assert_eq!(
        names(&document, StageKind::Linear),
        [op::FOLLOW_CHANNELS, op::BACKGROUND, op::COLOR_CALIBRATION]
    );
    // The card's channels first, in its order, then the channels the setup
    // saved that this card lacks.
    assert_eq!(
        names(&document, StageKind::Stretch),
        [
            "Stretch (sii)",
            "Stretch (ha)",
            "Stretch (oiii)",
            "Stretch (blue)",
            "Stretch (green)",
            "Stretch (luminance)",
            "Stretch (red)",
        ]
    );
    assert!(document.stage(StageKind::Finish).steps.is_empty());
    let layers: Vec<&str> = document
        .layers()
        .iter()
        .map(|layer| layer.name.as_str())
        .collect();
    // The layers are the SHO palette's, whatever order the card lists them
    // in, bottom first: the list reads SII, Ha, OIII from the top.
    assert_eq!(layers, ["OIII as blue", "Ha as green", "SII as red"]);

    // A step holds PSF Guard's own settings, untouched.
    let stretch = &document.stage(StageKind::Stretch).steps[1];
    let StepKind::Operation { settings, .. } = &stretch.kind else {
        panic!("a stretch is an operation");
    };
    assert_eq!(settings["model"]["type"], "auto-mtf");
    assert_eq!(settings["model"]["target_median"], 0.2);
    assert_eq!(settings["model"]["shadows_clip"], -2.8);
}

#[test]
fn a_setup_comes_back_from_its_document_unchanged() {
    let original = default_settings();
    let document = document_from_color_setup("Default", &original, &SHO).unwrap();
    let back = color_setup_from_document(&document).unwrap();
    assert!(same_settings(&back, &original), "{back:#}");

    // And through a save and a load of the document.
    let loaded = Document::from_json(&document.to_json()).unwrap();
    let back = color_setup_from_document(&loaded).unwrap();
    assert!(same_settings(&back, &original));
}

#[test]
fn every_kind_of_setting_survives_the_round_trip() {
    let mut settings = default_settings();
    let object = settings.as_object_mut().unwrap();
    object.insert(
        "input_deconvolutions".into(),
        json!({ "ha": { "psf_fwhm_pixels": 3.1, "iterations": 4, "amount": 0.35,
                        "noise_fraction": 0.001, "max_correction": 2.0 } }),
    );
    object.insert(
        "input_rc_astro".into(),
        json!({ "oiii": { "blur": { "sharpen_stars": 0.25 } }, "ha": { "noise": { "denoise": 0.9 } } }),
    );
    object.insert("emission".into(), json!({ "ha_amount": 0.6 }));
    object.insert(
        "color_rc_astro".into(),
        json!({ "noise": { "denoise": 0.5 } }),
    );
    object.insert(
        "narrowband".into(),
        json!({ "palette": "foraxx_sho", "green_reduction": 0.4 }),
    );
    object.insert(
        "output_stretches".into(),
        json!([{ "model": { "type": "ghs", "d": 2.0 } }, { "model": { "type": "identity" } }]),
    );
    // Two stretches on one channel, in order.
    object["input_stretches"]["ha"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "model": { "type": "asinh", "strength": 40.0 } }));
    // A setting from a newer PSF Guard.
    object.insert("future_setting".into(), json!({ "enabled": true }));

    let document = document_from_color_setup("Everything", &settings, &SHO).unwrap();
    assert_eq!(
        names(&document, StageKind::Linear),
        [
            op::FOLLOW_CHANNELS.to_string(),
            "Deconvolution (ha)".to_string(),
            "Processing tools (ha)".to_string(),
            "Processing tools (oiii)".to_string(),
            op::BACKGROUND.to_string(),
            op::EMISSION.to_string(),
            op::RC_ASTRO_COLOR.to_string(),
            op::COLOR_CALIBRATION.to_string(),
        ]
    );
    assert_eq!(
        names(&document, StageKind::Stretch)[..4],
        [
            "Stretch (sii)",
            "Stretch (ha)",
            "Stretch (ha)",
            "Stretch (oiii)"
        ]
    );
    assert_eq!(
        names(&document, StageKind::Stretch).last().unwrap(),
        op::NARROWBAND
    );
    assert_eq!(
        names(&document, StageKind::Finish),
        [op::OUTPUT_STRETCH, op::OUTPUT_STRETCH]
    );

    let back = color_setup_from_document(&document).unwrap();
    assert!(same_settings(&back, &settings), "{back:#}");
    assert_eq!(back["future_setting"]["enabled"], true);
    // The order of a channel's stretches, and of the output stretches, holds.
    assert_eq!(back["input_stretches"]["ha"][1]["model"]["type"], "asinh");
    assert_eq!(back["output_stretches"][0]["model"]["type"], "ghs");
}

#[test]
fn a_step_turned_off_leaves_the_settings() {
    let mut settings = default_settings();
    settings.as_object_mut().unwrap().insert(
        "input_deconvolutions".into(),
        json!({ "ha": { "psf_fwhm_pixels": 3.1 } }),
    );
    let mut document = document_from_color_setup("Default", &settings, &SHO).unwrap();
    let deconvolution = document
        .stage(StageKind::Linear)
        .steps
        .iter()
        .find(|step| step.kind.name() == op::DECONVOLUTION)
        .unwrap()
        .id;
    document.set_step_enabled(deconvolution, false);
    let back = color_setup_from_document(&document).unwrap();
    assert!(back.get("input_deconvolutions").is_none());
    assert!(same_settings(&back, &default_settings()));
}

#[test]
fn steps_studio_adds_stay_out_of_psf_guards_settings() {
    let original = default_settings();
    let mut document = document_from_color_setup("Default", &original, &SHO).unwrap();
    document
        .insert_step(
            StageKind::Finish,
            usize::MAX,
            StepRecord::new(StepKind::Operation {
                operation: "Curves".to_string(),
                accepts: crate::image_steps::Accepts::Display,
                settings: json!({ "amount": 0.5 }),
            }),
        )
        .unwrap();
    let back = color_setup_from_document(&document).unwrap();
    assert!(same_settings(&back, &original));
}

#[test]
fn a_channel_the_setup_did_not_save_gets_the_shared_stretch() {
    // Every saved channel has the same stretch, so a new channel follows it.
    let document =
        document_from_color_setup("Default", &default_settings(), &["nii", "ha"]).unwrap();
    assert_eq!(names(&document, StageKind::Stretch)[0], "Stretch (nii)");

    // Saved channels that differ give a new channel nothing to follow.
    let mut settings = default_settings();
    settings["input_stretches"]["ha"][0]["model"]["target_median"] = json!(0.3);
    let document = document_from_color_setup("Mixed", &settings, &["nii", "ha"]).unwrap();
    assert_eq!(names(&document, StageKind::Stretch)[0], "Stretch (ha)");
}

#[test]
fn settings_of_the_wrong_shape_are_refused() {
    for bad in [
        json!([1, 2]),
        json!({ "input_stretches": [] }),
        json!({ "input_stretches": { "ha": {} } }),
        json!({ "output_stretches": {} }),
        json!({ "input_deconvolutions": 3 }),
    ] {
        assert!(
            matches!(
                document_from_color_setup("Bad", &bad, &SHO),
                Err(StudioError::Document(_))
            ),
            "{bad}"
        );
    }
}

#[test]
fn studio_starts_from_the_layered_stretch_and_keeps_a_chosen_one() {
    let settings: Value =
        serde_json::from_str(include_str!("../processing_setups/default_color.json")).unwrap();
    let mut document = document_from_color_setup("Default", &settings, &["ha", "oiii"]).unwrap();
    let kinds = |document: &Document| -> Vec<String> {
        document
            .stage(StageKind::Stretch)
            .steps
            .iter()
            .filter(|step| step.kind.name() == op::STRETCH)
            .filter_map(|step| match &step.kind {
                StepKind::Operation { settings, .. } => {
                    Some(settings["model"]["type"].as_str()?.to_string())
                }
                _ => None,
            })
            .collect()
    };
    assert!(kinds(&document).iter().all(|kind| kind == "auto-mtf"));
    // One stretch a person chose stays.
    let first = document.stage(StageKind::Stretch).steps[0].id;
    document.set_step_settings(
        first,
        serde_json::json!({ "model": { "type": "asinh", "black": 0.0, "white": 1.0, "strength": 50.0 } }),
    );
    document.use_layered_stretch();
    let after = kinds(&document);
    assert_eq!(after[0], "asinh");
    assert!(after[1..].iter().all(|kind| kind == "layered"));
    assert_eq!(stretched_channel("ha"), "ha@stretched");
    // PSF Guard's settings take the layered stretch as any other.
    assert!(color_setup_from_document(&document).is_ok());
}

#[test]
fn the_background_moves_before_or_after_every_tools_step() {
    use crate::image_steps::Accepts;
    let operation = |name: &str, accepts: Accepts, settings: &Value| {
        StepRecord::new(StepKind::Operation {
            operation: name.to_string(),
            accepts,
            settings: settings.clone(),
        })
    };
    let mut document = Document::new("Order");
    let linear = |document: &Document| -> Vec<String> {
        document
            .stage(StageKind::Linear)
            .steps
            .iter()
            .map(|step| match &step.band {
                Some(band) => format!("{} {band}", step.kind.name()),
                None => step.kind.name().to_string(),
            })
            .collect()
    };
    for (name, band) in [
        (op::RC_ASTRO_INPUT, Some("ha")),
        (op::RC_ASTRO_INPUT, Some("oiii")),
        (op::RC_ASTRO_COLOR, Some("lines")),
        (op::BACKGROUND, None),
        (op::COLOR_CALIBRATION, None),
    ] {
        let mut step = operation(name, Accepts::Linear, &json!({}));
        if let Some(band) = band {
            step = step.for_band(band);
        }
        document
            .insert_step(StageKind::Linear, usize::MAX, step)
            .unwrap();
    }
    // As made, the tools run on the stacks and the background is fitted on
    // their results.
    assert!(!background_before_tools(&document));
    assert_eq!(new_tools_step_index(&document), 3);

    assert!(set_background_before_tools(&mut document, true));
    assert!(background_before_tools(&document));
    assert_eq!(
        linear(&document),
        [
            "Background extraction",
            "Processing tools ha",
            "Processing tools oiii",
            "Processing tools on color lines",
            "Color calibration",
        ]
    );
    // A new tools step goes after the ones already there, so the order
    // keeps.
    assert_eq!(new_tools_step_index(&document), 4);
    assert!(!set_background_before_tools(&mut document, true));

    assert!(set_background_before_tools(&mut document, false));
    assert_eq!(
        linear(&document),
        [
            "Processing tools ha",
            "Processing tools oiii",
            "Processing tools on color lines",
            "Background extraction",
            "Color calibration",
        ]
    );

    // With no tools step there is no order to choose.
    let mut plain = Document::new("Plain");
    plain
        .insert_step(
            StageKind::Linear,
            0,
            operation(op::BACKGROUND, Accepts::Linear, &json!({})),
        )
        .unwrap();
    assert!(!background_before_tools(&plain));
    assert!(!set_background_before_tools(&mut plain, true));
}
