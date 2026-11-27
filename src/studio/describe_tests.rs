use super::*;
use serde_json::Value;

fn document() -> Document {
    let settings: Value =
        serde_json::from_str(include_str!("../processing_setups/default_color.json")).unwrap();
    let mut document = document_from_color_setup("Test", &settings, &["red", "green"]).unwrap();
    // Only the input layers.
    let adjustments: Vec<uuid::Uuid> = document
        .layers()
        .iter()
        .filter(|layer| layer.source.is_adjustment())
        .map(|layer| layer.id)
        .collect();
    for id in adjustments {
        document.remove_layer(id);
    }
    document
}

#[test]
fn an_edit_is_named_by_what_it_changed() {
    let before = document();
    let ids: Vec<uuid::Uuid> = before.layers().iter().map(|layer| layer.id).collect();
    let name = |at: usize| before.layers()[at].name.clone();

    let mut after = before.clone();
    after.layer_mut(ids[0]).unwrap().opacity = 0.5;
    assert_eq!(
        describe_edit(&before, &after),
        format!("{}: opacity 100% → 50%", name(0))
    );

    let mut after = before.clone();
    after.layer_mut(ids[1]).unwrap().visible = false;
    assert_eq!(describe_edit(&before, &after), format!("Hid {}", name(1)));

    let mut after = before.clone();
    after.step_layer(ids[1], false);
    let moved = describe_edit(&before, &after);
    assert!(moved.starts_with("Moved "), "{moved}");

    let mut after = before.clone();
    let step = StepRecord::new(StepKind::Operation {
        operation: "Curves".to_string(),
        accepts: crate::image_steps::Accepts::Display,
        settings: serde_json::json!({ "amount": 0.25 }),
    });
    let step_id = step.id;
    after.insert_layer_step(ids[0], step).unwrap();
    assert_eq!(
        describe_edit(&before, &after),
        format!("Added Curves to {}", name(0))
    );
    let mut changed = after.clone();
    changed.set_step_settings(step_id, serde_json::json!({ "amount": 0.5 }));
    assert_eq!(
        describe_edit(&after, &changed),
        format!("Curves of {}: amount 0.25 → 0.50", name(0))
    );

    let mut after = before.clone();
    after.headroom = Some(2.0);
    after.layer_mut(ids[0]).unwrap().opacity = 0.4;
    after.layer_mut(ids[1]).unwrap().opacity = 0.3;
    assert_eq!(
        describe_edit(&before, &after),
        format!(
            "Made the picture HDR, 2 stops; {}: opacity 100% → 40% and 1 more",
            name(0)
        )
    );
    assert_eq!(describe_edit(&before, &before), "Changed the document");
}
