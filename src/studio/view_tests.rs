use super::flow_operations as op;
use super::*;
use serde_json::Value;
use uuid::Uuid;

const RGB: [&str; 3] = ["red", "green", "blue"];

fn document() -> Document {
    let settings: Value =
        serde_json::from_str(include_str!("../processing_setups/default_color.json")).unwrap();
    document_from_color_setup("Default", &settings, &RGB).unwrap()
}

fn step(document: &Document, stage: StageKind, name: &str, band: Option<&str>) -> Uuid {
    document
        .stage(stage)
        .steps
        .iter()
        .find(|step| step.kind.name() == name && step.band.as_deref() == band)
        .unwrap()
        .id
}

/// The background step that applies to a channel in its own view.
fn background(document: &Document, channel: &str) -> Option<Uuid> {
    plan(
        document,
        &view(
            Subject::Channel(channel.into()),
            UpTo::Final,
            DisplayStretch::Document,
        ),
    )
    .channels[0]
        .background
}

fn view(subject: Subject, up_to: UpTo, stretch: DisplayStretch) -> View {
    View {
        subject,
        up_to,
        stretch,
        own_until: None,
    }
}

#[test]
fn a_documents_channels_are_its_sources() {
    assert_eq!(document().channels(), RGB);
}

#[test]
fn a_channel_view_draws_one_channel_in_gray_with_no_finish_steps() {
    let mut document = document();
    document
        .insert_step(
            StageKind::Finish,
            usize::MAX,
            StepRecord::new(StepKind::Operation {
                operation: "Curves".into(),
                accepts: crate::image_steps::Accepts::Display,
                settings: Value::Null,
            }),
        )
        .unwrap();
    let plan = plan(
        &document,
        &view(
            Subject::Channel("green".into()),
            UpTo::Final,
            DisplayStretch::Document,
        ),
    );
    assert!(!plan.combined);
    assert!(plan.finish.is_empty());
    assert_eq!(plan.channels.len(), 1);
    let green = &plan.channels[0];
    assert_eq!(green.channel, "green");
    assert_eq!(
        green.background,
        Some(step(&document, StageKind::Linear, op::BACKGROUND, None))
    );
    assert_eq!(
        green.stretch,
        PlannedStretch::Document(vec![step(
            &document,
            StageKind::Stretch,
            op::STRETCH,
            Some("green")
        )])
    );
    assert_eq!(
        plan.description,
        "green on its own, with every step, drawn with the document's stretch."
    );

    // The combined view applies the Finish step and draws every layer.
    let combined = super::view::plan(
        &document,
        &view(Subject::Combined, UpTo::Final, DisplayStretch::Document),
    );
    assert!(combined.combined);
    assert_eq!(combined.finish.len(), 1);
    let drawn: Vec<&str> = combined
        .channels
        .iter()
        .map(|c| c.channel.as_str())
        .collect();
    assert_eq!(drawn, RGB);
}

#[test]
fn a_view_applies_the_steps_up_to_where_it_stops() {
    let document = document();
    let background = step(&document, StageKind::Linear, op::BACKGROUND, None);
    let red = || Subject::Channel("red".into());

    // Before the Linear stage: no background, and nothing stretched.
    let before = plan(
        &document,
        &view(
            red(),
            UpTo::StartOf(StageKind::Linear),
            DisplayStretch::Document,
        ),
    );
    assert_eq!(before.channels[0].background, None);
    assert_eq!(before.channels[0].stretch, PlannedStretch::Preview);
    assert_eq!(
        before.description,
        "red on its own, before the Linear stage, drawn with a preview stretch, not saved, because the view stops before the stretch."
    );

    // After the background step: corrected, still not stretched.
    let after = plan(
        &document,
        &view(red(), UpTo::Step(background), DisplayStretch::Document),
    );
    assert_eq!(after.channels[0].background, Some(background));
    assert_eq!(after.channels[0].stretch, PlannedStretch::Preview);
    assert!(after.description.contains("after Background extraction"));

    // After the Stretch stage: the document's stretch.
    let stretched = plan(
        &document,
        &view(
            red(),
            UpTo::EndOf(StageKind::Stretch),
            DisplayStretch::Document,
        ),
    );
    assert!(matches!(
        stretched.channels[0].stretch,
        PlannedStretch::Document(_)
    ));

    // A person may ask for the preview stretch anywhere.
    let preview = plan(
        &document,
        &view(red(), UpTo::Final, DisplayStretch::Preview),
    );
    assert_eq!(preview.channels[0].stretch, PlannedStretch::Preview);
    assert!(preview
        .description
        .ends_with("a preview stretch, not saved."));
}

#[test]
fn a_view_that_stops_between_two_channels_stretches_says_so() {
    let document = document();
    let red = step(&document, StageKind::Stretch, op::STRETCH, Some("red"));
    let plan = plan(
        &document,
        &view(Subject::Combined, UpTo::Step(red), DisplayStretch::Document),
    );
    let stretched: Vec<bool> = plan
        .channels
        .iter()
        .map(|channel| matches!(channel.stretch, PlannedStretch::Document(_)))
        .collect();
    assert_eq!(stretched, [true, false, false]);
    assert!(plan
        .description
        .contains("for the 2 channel(s) not stretched yet"));
}

#[test]
fn a_step_that_is_off_is_not_applied() {
    let mut document = document();
    let background = step(&document, StageKind::Linear, op::BACKGROUND, None);
    document.set_step_enabled(background, false);
    let plan = plan(
        &document,
        &view(
            Subject::Channel("red".into()),
            UpTo::Final,
            DisplayStretch::Document,
        ),
    );
    assert_eq!(plan.channels[0].background, None);
}

#[test]
fn a_channel_with_its_own_step_does_not_follow_the_shared_one() {
    let mut document = document();
    let shared = step(&document, StageKind::Linear, op::BACKGROUND, None);
    let own = own_step_for_channel(&mut document, shared, "blue").unwrap();
    assert_ne!(own, shared);
    // Asking again gives the same step, not another copy.
    assert_eq!(
        own_step_for_channel(&mut document, shared, "blue"),
        Some(own)
    );
    // The copy starts with the shared step's settings and sits after it.
    let linear = &document.stage(StageKind::Linear).steps;
    assert_eq!(linear[0].id, shared);
    assert_eq!(linear[1].id, own);
    assert_eq!(linear[1].band.as_deref(), Some("blue"));

    document.set_step_settings(own, serde_json::json!({ "strength": 0.25 }));
    assert_eq!(background(&document, "blue"), Some(own));
    assert_eq!(background(&document, "red"), Some(shared));

    // Turning the channel's own step off turns the correction off for that
    // channel only. It does not fall back to the shared step.
    document.set_step_enabled(own, false);
    assert_eq!(background(&document, "blue"), None);
    assert_eq!(background(&document, "red"), Some(shared));

    // A channel's own step is listed for that channel and the combined
    // view, not for another channel.
    let own_step = document.step(own).unwrap().1;
    assert!(step_shown_for(own_step, &Subject::Channel("blue".into())));
    assert!(step_shown_for(own_step, &Subject::Combined));
    assert!(!step_shown_for(own_step, &Subject::Channel("red".into())));
}

#[test]
fn a_channels_own_step_stays_out_of_psf_guards_settings() {
    let mut document = document();
    let shared = step(&document, StageKind::Linear, op::BACKGROUND, None);
    let before = color_setup_from_document(&document).unwrap();
    let own = own_step_for_channel(&mut document, shared, "blue").unwrap();
    document.set_step_settings(own, serde_json::json!({ "strength": 0.25 }));
    // PSF Guard's settings hold one background for every channel.
    assert_eq!(color_setup_from_document(&document).unwrap(), before);
}

fn adjustment(name: &str) -> LayerRecord {
    LayerRecord::new(
        name,
        LayerSource::Adjustment {
            operation: name.to_string(),
            settings: serde_json::json!({ "amount": 0.5 }),
        },
    )
}

#[test]
fn a_blended_view_draws_band_layers_and_adjustments_in_order() {
    let mut document = document();
    // red, Exposure, green, blue, Curves: an adjustment in the middle
    // changes only what is beneath it.
    let exposure = adjustment("Exposure");
    let curves = adjustment("Curves");
    let (exposure_id, curves_id) = (exposure.id, curves.id);
    document.insert_layer(1, exposure).unwrap();
    document.insert_layer(usize::MAX, curves).unwrap();
    let ids: Vec<Uuid> = document.layers().iter().map(|layer| layer.id).collect();

    let combined = plan(
        &document,
        &view(Subject::Combined, UpTo::Final, DisplayStretch::Document),
    );
    assert_eq!(
        combined.stack,
        [
            StackEntry::Band {
                layer: ids[0],
                channel: 0,
                clip: None
            },
            StackEntry::Adjustment { layer: exposure_id },
            StackEntry::Band {
                layer: ids[2],
                channel: 1,
                clip: None
            },
            StackEntry::Band {
                layer: ids[3],
                channel: 2,
                clip: None
            },
            StackEntry::Adjustment { layer: curves_id },
        ]
    );
    let drawn: Vec<&str> = combined
        .channels
        .iter()
        .map(|c| c.channel.as_str())
        .collect();
    assert_eq!(drawn, RGB);

    // Up to a layer: that layer and those beneath it.
    let up_to = plan(
        &document,
        &view(
            Subject::LayersUpTo(ids[2]),
            UpTo::Final,
            DisplayStretch::Document,
        ),
    );
    assert_eq!(up_to.stack.len(), 3);
    assert_eq!(
        up_to.description,
        "The layers up to G as green, with every step, drawn with the document's stretch."
    );

    // A band layer alone.
    let alone = plan(
        &document,
        &view(
            Subject::Layer(ids[3]),
            UpTo::Final,
            DisplayStretch::Document,
        ),
    );
    assert_eq!(
        alone.stack,
        [StackEntry::Band {
            layer: ids[3],
            channel: 0,
            clip: None
        }]
    );
    assert!(alone
        .description
        .starts_with("The layer B as blue on its own"));

    // An adjustment alone has nothing to show, so it comes with the layers
    // beneath it.
    let adjusted = plan(
        &document,
        &view(
            Subject::Layer(exposure_id),
            UpTo::Final,
            DisplayStretch::Document,
        ),
    );
    assert_eq!(adjusted.stack.len(), 2);
    assert!(adjusted
        .description
        .starts_with("Exposure over the layers beneath it"));

    // A channel view blends nothing.
    let channel = plan(
        &document,
        &view(
            Subject::Channel("red".into()),
            UpTo::Final,
            DisplayStretch::Document,
        ),
    );
    assert!(channel.stack.is_empty());
}

#[test]
fn a_layers_mask_is_painted_and_a_document_with_adjustments_round_trips() {
    let mut document = document();
    let exposure = adjustment("Exposure");
    let id = exposure.id;
    document.insert_layer(usize::MAX, exposure).unwrap();
    let stroke = Stroke {
        points: vec![[10.0, 10.0], [40.0, 30.0]],
        radius: 12.0,
        hardness: 0.6,
        mode: StrokeMode::Add,
    };
    document.paint_layer_mask(id, stroke.clone()).unwrap();
    document.paint_layer_mask(id, stroke).unwrap();
    let layer = document.layers().last().unwrap();
    let Some(MaskRecord::Painted(mask)) = &layer.mask else {
        panic!("the layer has a painted mask");
    };
    assert_eq!(mask.strokes.len(), 2);
    assert!(document
        .paint_layer_mask(Uuid::new_v4(), mask.strokes[0].clone())
        .is_err());

    let loaded = Document::from_json(&document.to_json()).unwrap();
    assert_eq!(loaded, document);
    // Adjustment layers are Studio's: PSF Guard's settings do not change.
    assert_eq!(
        color_setup_from_document(&loaded).unwrap(),
        color_setup_from_document(&super::view_tests::document()).unwrap()
    );
}
