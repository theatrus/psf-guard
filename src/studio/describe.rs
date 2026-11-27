//! An edit in a few words, for the History panel: what changed between a
//! document before an edit and after it.
//!
//! The words name what a person changed, as the tree shows it: the layer or
//! the step by its name, and a setting with its value before and after.
//! When an edit changes several things, the first two are named and the
//! rest counted.

use super::palette::channel_label;
use super::{Document, LayerRecord, MaskRecord, StageKind, StepKind, StepRecord};
use serde_json::Value;
use uuid::Uuid;

/// What changed from `before` to `after`, in a few words.
pub fn describe_edit(before: &Document, after: &Document) -> String {
    let mut changes = Vec::new();
    document_changes(before, after, &mut changes);
    layer_changes(before, after, &mut changes);
    step_changes(before, after, &mut changes);
    mask_changes(before, after, &mut changes);
    match changes.len() {
        0 => "Changed the document".to_string(),
        1 | 2 => changes.join("; "),
        more => format!("{}; {} and {} more", changes[0], changes[1], more - 2),
    }
}

/// A number as a person reads it in the panel.
fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e6 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

fn percent(value: f32) -> String {
    format!("{:.0}%", value * 100.0)
}

fn document_changes(before: &Document, after: &Document, changes: &mut Vec<String>) {
    if before.name != after.name {
        changes.push(format!("Renamed the picture to {}", after.name));
    }
    match (before.headroom, after.headroom) {
        (None, Some(stops)) => changes.push(format!(
            "Made the picture HDR, {} stops",
            number(stops.into())
        )),
        (Some(_), None) => changes.push("Made the picture SDR".to_string()),
        (Some(a), Some(b)) if a != b => changes.push(format!(
            "Headroom {} → {} stops",
            number(a.into()),
            number(b.into())
        )),
        _ => {}
    }
    if before.hdr_knee != after.hdr_knee {
        changes.push("Changed the HDR knee".to_string());
    }
    if before.crop != after.crop {
        changes.push("Changed the crop".to_string());
    }
}

/// Where a step applies, as the tree words it.
fn owner_of(document: &Document, stage: StageKind, step: &StepRecord) -> String {
    if let Some(layer) = document
        .layers()
        .iter()
        .find(|layer| layer.steps.iter().any(|own| own.id == step.id))
    {
        return layer.name.clone();
    }
    match (&step.band, stage) {
        (Some(band), _) => channel_label(band),
        (None, StageKind::Finish) => "the picture".to_string(),
        (None, _) => "every input".to_string(),
    }
}

/// Every step of the document by its id, with where it applies: a stage's,
/// a layer's own, or an adjustment layer's, which the layer's name stands
/// for.
fn steps_of(document: &Document) -> Vec<(Uuid, String, &StepRecord)> {
    let mut steps = Vec::new();
    for stage in StageKind::ALL {
        for step in &document.stage(stage).steps {
            steps.push((step.id, owner_of(document, stage, step), step));
        }
    }
    for layer in document.layers() {
        for step in &layer.steps {
            steps.push((step.id, layer.name.clone(), step));
        }
        if let Some(step) = layer.source.adjustment_step() {
            steps.push((step.id, String::new(), step));
        }
    }
    steps
}

/// The first setting that differs between two step settings, as its key
/// path in words, with both values.
fn setting_change(before: &Value, after: &Value, path: &str) -> Option<String> {
    match (before, after) {
        (Value::Object(a), Value::Object(b)) => {
            let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
            keys.sort();
            keys.dedup();
            keys.into_iter().find_map(|key| {
                setting_change(
                    a.get(key).unwrap_or(&Value::Null),
                    b.get(key).unwrap_or(&Value::Null),
                    key,
                )
            })
        }
        (a, b) if a == b => None,
        (Value::Number(a), Value::Number(b)) => Some(format!(
            "{} {} → {}",
            words_of(path),
            number(a.as_f64().unwrap_or(0.0)),
            number(b.as_f64().unwrap_or(0.0))
        )),
        (Value::Bool(_), Value::Bool(b)) => Some(format!(
            "{} {}",
            words_of(path),
            if *b { "on" } else { "off" }
        )),
        (Value::String(a), Value::String(b)) => Some(format!(
            "{} {} → {}",
            words_of(path),
            words_of(a),
            words_of(b)
        )),
        _ => Some(format!("{} changed", words_of(path))),
    }
}

/// A key or a value from settings as words: `target_background` reads
/// "target background".
fn words_of(key: &str) -> String {
    key.replace(['_', '-'], " ")
}

fn step_changes(before: &Document, after: &Document, changes: &mut Vec<String>) {
    let (old, new) = (steps_of(before), steps_of(after));
    let named = |owner: &str, step: &StepRecord| {
        if owner.is_empty() {
            step.kind.name().to_string()
        } else {
            format!("{} of {owner}", step.kind.name())
        }
    };
    for (id, owner, step) in &new {
        let Some((_, _, was)) = old.iter().find(|(other, _, _)| other == id) else {
            // An adjustment layer's step comes and goes with its layer.
            if !owner.is_empty() {
                changes.push(format!("Added {} to {owner}", step.kind.name()));
            }
            continue;
        };
        if was == step {
            continue;
        }
        let name = named(owner, step);
        if was.enabled != step.enabled {
            changes.push(format!(
                "Turned {name} {}",
                if step.enabled { "on" } else { "off" }
            ));
        } else if was.opacity != step.opacity {
            changes.push(format!(
                "{name}: opacity {} → {}",
                percent(was.opacity),
                percent(step.opacity)
            ));
        } else if was.mask != step.mask {
            changes.push(format!("Painted the mask of {name}"));
        } else {
            let change = match (&was.kind, &step.kind) {
                (
                    StepKind::Operation { settings: a, .. },
                    StepKind::Operation { settings: b, .. },
                ) => setting_change(a, b, "settings"),
                _ => None,
            };
            changes.push(match change {
                Some(change) => format!("{name}: {change}"),
                None => format!("Changed {name}"),
            });
        }
    }
    for (id, owner, step) in &old {
        if !owner.is_empty() && !new.iter().any(|(other, _, _)| other == id) {
            changes.push(format!("Removed {} from {owner}", step.kind.name()));
        }
    }
}

fn layer_changes(before: &Document, after: &Document, changes: &mut Vec<String>) {
    let (old, new) = (before.layers(), after.layers());
    let find =
        |layers: &[LayerRecord], id: Uuid| layers.iter().find(|layer| layer.id == id).cloned();
    let group_name = |document: &Document, id: Option<Uuid>| {
        id.and_then(|id| document.layers().iter().find(|layer| layer.id == id))
            .map(|layer| layer.name.clone())
    };
    for layer in new {
        let Some(was) = find(old, layer.id) else {
            changes.push(format!("Added {}", layer.name));
            continue;
        };
        let name = &layer.name;
        if was.name != layer.name {
            changes.push(format!("Renamed {} to {name}", was.name));
        }
        if was.visible != layer.visible {
            changes.push(format!(
                "{} {name}",
                if layer.visible { "Showed" } else { "Hid" }
            ));
        }
        if was.opacity != layer.opacity {
            changes.push(format!(
                "{name}: opacity {} → {}",
                percent(was.opacity),
                percent(layer.opacity)
            ));
        }
        if was.blend != layer.blend {
            changes.push(format!("{name}: blend {} → {}", was.blend, layer.blend));
        }
        if was.color != layer.color {
            changes.push(format!("{name}: color"));
        }
        if was.mask != layer.mask || was.painted_too != layer.painted_too {
            let shared =
                |mask: &Option<MaskRecord>| matches!(mask, Some(MaskRecord::Shared { .. }));
            changes.push(if shared(&layer.mask) && !shared(&was.mask) {
                format!("Put {name} behind a shared mask")
            } else if layer.mask.is_none() && layer.painted_too.is_none() {
                format!("Removed the mask of {name}")
            } else {
                format!("Painted the mask of {name}")
            });
        }
        if was.group != layer.group {
            changes.push(match group_name(after, layer.group) {
                Some(group) => format!("Moved {name} into {group}"),
                None => format!("Moved {name} out of its group"),
            });
        }
        if was.pass_through != layer.pass_through {
            changes.push(format!(
                "{name}: layers {}",
                if layer.pass_through {
                    "pass through"
                } else {
                    "blend together first"
                }
            ));
        }
        if was.clipped != layer.clipped {
            changes.push(format!(
                "{} {name}",
                if layer.clipped {
                    "Clipped"
                } else {
                    "Unclipped"
                }
            ));
        }
        if was.star_reduction != layer.star_reduction {
            changes.push(format!(
                "{name}: star reduction {} → {}",
                percent(was.star_reduction),
                percent(layer.star_reduction)
            ));
        }
        // An adjustment layer's step changes are named by the layer.
        if let (Some(a), Some(b)) = (was.source.adjustment_step(), layer.source.adjustment_step())
            && a.kind != b.kind
        {
            let change = match (&a.kind, &b.kind) {
                (
                    StepKind::Operation { settings: x, .. },
                    StepKind::Operation { settings: y, .. },
                ) => setting_change(x, y, "settings"),
                _ => None,
            };
            changes.push(match change {
                Some(change) => format!("{name}: {change}"),
                None => format!("Changed {name}"),
            });
        }
    }
    for layer in old {
        if find(new, layer.id).is_none() {
            changes.push(format!("Removed {}", layer.name));
        }
    }
    // The same layers in another order: the one that moved.
    let ids = |layers: &[LayerRecord]| layers.iter().map(|layer| layer.id).collect::<Vec<_>>();
    let (a, b) = (ids(old), ids(new));
    if a != b && a.len() == b.len() && a.iter().all(|id| b.contains(id)) {
        let moved = b.iter().find(|id| {
            let without = |list: &[Uuid]| {
                list.iter()
                    .filter(|other| other != id)
                    .copied()
                    .collect::<Vec<_>>()
            };
            without(&a) == without(&b)
        });
        let name = moved
            .and_then(|id| find(new, *id))
            .map_or("layers".to_string(), |layer| layer.name);
        changes.push(format!("Moved {name}"));
    }
}

fn mask_changes(before: &Document, after: &Document, changes: &mut Vec<String>) {
    for mask in after.masks() {
        match before.mask(mask.id) {
            None => changes.push(format!("Added the shared mask {}", mask.name)),
            Some(was) if was != mask => {
                changes.push(format!("Changed the shared mask {}", mask.name))
            }
            _ => {}
        }
    }
    for mask in before.masks() {
        if after.mask(mask.id).is_none() {
            changes.push(format!("Removed the shared mask {}", mask.name));
        }
    }
}
