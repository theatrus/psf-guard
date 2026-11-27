//! Processing tools on a layer or on the picture: tool steps among a
//! layer's own steps and in the Finish stage.
//!
//! A tool step in the Linear stage runs on its input when the store is
//! prepared. One after it runs on what Studio draws there: the layer alone,
//! after its stretch and its own steps before the tool, or the picture
//! after the Finish steps before it. Run tools renders that image at full
//! resolution with Seiza's CPU render, has `psf-guard studio-tool` run the
//! tool on it, and keeps the result in a folder beside the store, named by
//! everything it depends on. The canvas then draws the result in place of
//! what came before the step, and a step whose result is missing or out of
//! date is listed as left out until the tools run again.
//!
//! The stars a star removal takes out of a layer or of the picture are
//! added back right after the step, screened over its result, unless the
//! step's `stars_shown` setting is false.

use super::Studio;
use crate::derived::{DerivedKey, DerivedKind, DerivedPlane};
use crate::studio_model::{
    flow_operations, is_tool_step, tool_of, Document, Place, StageKind, StepRecord,
    SHIFTED_PASS_KEY, STARS_SHOWN_KEY,
};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// A tool step after the Linear stage.
#[derive(Clone, Debug)]
pub(super) struct LateStep {
    pub id: Uuid,
    /// What the tool runs on.
    pub on: LateOn,
    pub enabled: bool,
    /// The tool: `{"provider": ..., "parameters": {...}}`.
    pub tool: Value,
    /// What the result depends on, as a name for its files: the store, the
    /// document up to the step, and the tool.
    pub identity: String,
    /// The step in a few words, for messages: "Noise reduction on the
    /// picture".
    pub words: String,
}

/// What a late tool step runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LateOn {
    /// One layer alone: the step is that layer's own.
    Layer(Uuid),
    /// The layers beneath an adjustment layer that holds the step.
    Beneath(Uuid),
    /// The picture after the Finish steps before it.
    Picture,
}

impl LateStep {
    fn removes_stars(&self) -> bool {
        crate::studio_model::tool_operation(self.tool["provider"].as_str().unwrap_or(""))
            == flow_operations::STAR_REMOVAL
    }

    fn shifted(&self) -> bool {
        self.tool[SHIFTED_PASS_KEY].as_bool().unwrap_or(false)
    }
}

/// The folder a store's late tool results are kept in.
fn folder(store: &Path) -> PathBuf {
    store.with_extension("late")
}

fn result_path(store: &Path, identity: &str) -> PathBuf {
    folder(store).join(format!("{identity}.fits"))
}

fn stars_path(store: &Path, identity: &str) -> PathBuf {
    folder(store).join(format!("{identity}.stars.fits"))
}

/// The store's size and modification time, which change when it is
/// prepared again.
fn store_version(store: &Path) -> String {
    std::fs::metadata(store)
        .ok()
        .map(|metadata| {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_nanos());
            format!("{}:{modified}", metadata.len())
        })
        .unwrap_or_default()
}

/// The document as far as a late tool step reads it, as JSON: for a
/// Finish step, everything but the Finish steps from it on and the Export
/// stage; for an adjustment layer's, the layers beneath it; for a layer's
/// own step, the inputs and that layer alone with its own steps before the
/// tool.
fn upstream(document: &Document, step: &StepRecord, on: LateOn) -> Value {
    let mut value: Value = serde_json::from_str(&document.to_json()).unwrap_or(Value::Null);
    value["name"] = Value::Null;
    // Whether a star removal's stars are shown changes no tool's image.
    strip_stars_shown(&mut value);
    let stage = |kind: StageKind| kind as usize;
    if let Some(stages) = value["stages"].as_array_mut() {
        if let Some(export) = stages.get_mut(stage(StageKind::Export)) {
            export["steps"] = Value::Array(Vec::new());
        }
        let truncate = |steps: &mut Value| {
            if let Some(list) = steps.as_array_mut() {
                let at = list
                    .iter()
                    .position(|other| other["id"] == step.id.to_string())
                    .unwrap_or(list.len());
                list.truncate(at);
            }
        };
        match on {
            LateOn::Picture => {
                if let Some(finish) = stages.get_mut(stage(StageKind::Finish)) {
                    truncate(&mut finish["steps"]);
                }
            }
            LateOn::Beneath(layer) => {
                if let Some(finish) = stages.get_mut(stage(StageKind::Finish)) {
                    finish["steps"] = Value::Array(Vec::new());
                }
                if let Some(combine) = stages.get_mut(stage(StageKind::Combine))
                    && let Some(layers) = combine["layers"].as_array_mut()
                {
                    let at = layers
                        .iter()
                        .position(|other| other["id"] == layer.to_string())
                        .unwrap_or(layers.len());
                    // In a group, the tool reads its group's members only.
                    let group = layers.get(at).map(|own| own["group"].clone());
                    layers.truncate(at);
                    if let Some(group) = group.filter(|group| !group.is_null()) {
                        layers.retain(|other| other["group"] == group);
                    }
                }
            }
            LateOn::Layer(layer) => {
                if let Some(finish) = stages.get_mut(stage(StageKind::Finish)) {
                    finish["steps"] = Value::Array(Vec::new());
                }
                if let Some(combine) = stages.get_mut(stage(StageKind::Combine))
                    && let Some(layers) = combine["layers"].as_array_mut()
                {
                    layers.retain(|other| other["id"] == layer.to_string());
                    for other in layers.iter_mut() {
                        other["group"] = Value::Null;
                        truncate(&mut other["steps"]);
                    }
                }
            }
        }
    }
    value
}

/// `value` with every step's `stars_shown` setting left out.
fn strip_stars_shown(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map.contains_key("provider") {
                map.remove(STARS_SHOWN_KEY);
            }
            for child in map.values_mut() {
                strip_stars_shown(child);
            }
        }
        Value::Array(list) => list.iter_mut().for_each(strip_stars_shown),
        _ => {}
    }
}

/// The document's tool steps after the Linear stage, in the order they
/// run: each layer's own, bottom layer first, then the Finish steps.
pub(super) fn late_steps(document: &Document, store: &Path) -> Vec<LateStep> {
    use sha2::{Digest, Sha256};
    let version = store_version(store);
    let make = |step: &StepRecord, on: LateOn, words: String| -> Option<LateStep> {
        let tool = tool_of(step)?.clone();
        // Whether its stars are shown changes what is drawn, not what the
        // tool makes.
        let mut made_by = tool.clone();
        if let Some(settings) = made_by.as_object_mut() {
            settings.remove(STARS_SHOWN_KEY);
        }
        let text = format!(
            "{version}|{}|{}",
            upstream(document, step, on),
            serde_json::to_string(&made_by).unwrap_or_default()
        );
        let identity = Sha256::digest(text.as_bytes())
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Some(LateStep {
            id: step.id,
            on,
            enabled: step.enabled,
            tool,
            identity,
            words,
        })
    };
    let mut steps = Vec::new();
    for layer in document.layers() {
        for step in layer.steps.iter().filter(|step| is_tool_step(step)) {
            let words = format!("{} on {}", step.kind.name(), layer.name);
            steps.extend(make(step, LateOn::Layer(layer.id), words));
        }
        // A tool layer runs on the layers beneath it, in a group on its
        // group's members beneath it.
        if let Some(step) = layer.source.adjustment_step()
            && is_tool_step(step)
        {
            let words = if layer.group.is_some() {
                format!("{} on the members beneath it", step.kind.name())
            } else {
                format!("{} on the layers beneath it", step.kind.name())
            };
            let mut late = make(step, LateOn::Beneath(layer.id), words);
            if let Some(late) = &mut late {
                late.enabled = layer.visible;
            }
            steps.extend(late);
        }
    }
    for step in &document.stage(StageKind::Finish).steps {
        if is_tool_step(step) {
            let words = format!("{} on the picture", step.kind.name());
            steps.extend(make(step, LateOn::Picture, words));
        }
    }
    steps
}

/// The planes of a late step's result, as they are kept beside the store.
fn read_result(store: &Path, step: &LateStep, levels: usize) -> Result<Vec<DerivedPlane>, String> {
    let mut planes = Vec::new();
    let mut read = |path: PathBuf, stars: bool| -> Result<(), String> {
        let image = seiza_fits::FitsImage::open(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let samples = match image.pixels {
            seiza_fits::Pixels::F32(samples) => samples,
            _ => return Err(format!("{} is not 32-bit floats", path.display())),
        };
        let plane = image.width * image.height;
        for channel in 0..image.planes.min(3) {
            let key = DerivedKey {
                kind: DerivedKind::Tool {
                    step: step.id,
                    channel: channel as u8,
                    stars,
                },
                bands: Vec::new(),
                states: Vec::new(),
                range: (0.0, 1.0),
            };
            planes.push(DerivedPlane::of_samples(
                key,
                image.width,
                image.height,
                samples[channel * plane..(channel + 1) * plane].to_vec(),
                levels,
            ));
        }
        Ok(())
    };
    read(result_path(store, &step.identity), false)?;
    let stars = stars_path(store, &step.identity);
    if stars.is_file() {
        read(stars, true)?;
    }
    Ok(planes)
}

/// The planes read from a result's files, once they are read.
type ReadPlanes = Arc<Mutex<Option<Result<Vec<DerivedPlane>, String>>>>;

/// A result being read from its file.
struct Reading {
    step: Uuid,
    identity: String,
    done: ReadPlanes,
}

/// The late tool steps' results the canvas has, and the one being read.
#[derive(Default)]
pub(super) struct Late {
    /// Each step whose result the canvas draws, with the result's identity.
    shown: HashMap<Uuid, String>,
    reading: Option<Reading>,
}

impl Late {
    /// Whether the canvas draws a step's result.
    pub(super) fn has(&self, step: Uuid) -> bool {
        self.shown.contains_key(&step)
    }
}

impl Studio {
    /// Whether a tool's result is being read, or one the document has is
    /// waiting to be.
    pub(super) fn late_reading(&self) -> bool {
        if self.late.reading.is_some() {
            return true;
        }
        let Some(store) = self.store_path() else {
            return false;
        };
        late_steps(&self.document, store).iter().any(|step| {
            step.enabled && !self.late.has(step.id) && result_path(store, &step.identity).is_file()
        })
    }

    /// The late tool steps whose result is missing or out of date, which
    /// Run tools would run.
    pub(super) fn late_to_run(&self) -> Vec<LateStep> {
        let Some(store) = self.store_path() else {
            return Vec::new();
        };
        late_steps(&self.document, store)
            .into_iter()
            .filter(|step| step.enabled && !result_path(store, &step.identity).is_file())
            .collect()
    }

    /// Keeps the canvas's late results those of the document as it is:
    /// drops one whose step changed or went, and reads one whose file is
    /// there.
    pub(super) fn follow_late(&mut self) {
        let Some(store) = self.store_path().map(Path::to_path_buf) else {
            return;
        };
        let steps = late_steps(&self.document, &store);
        let current = |id: Uuid| {
            steps
                .iter()
                .find(|step| step.id == id && step.enabled)
                .map(|step| step.identity.clone())
        };
        let mut changed = false;
        let gone: Vec<Uuid> = self
            .late
            .shown
            .iter()
            .filter(|(id, identity)| current(**id).as_ref() != Some(*identity))
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.late.shown.remove(&id);
            self.derived.retain(
                |plane| !matches!(plane.key.kind, DerivedKind::Tool { step, .. } if step == id),
            );
            changed = true;
        }
        let read = self
            .late
            .reading
            .as_ref()
            .and_then(|reading| reading.done.lock().expect("late result").take());
        if let Some(result) = read {
            let reading = self.late.reading.take().expect("a reading");
            match result {
                Ok(planes) if current(reading.step) == Some(reading.identity.clone()) => {
                    self.derived.retain(|plane| {
                        !matches!(plane.key.kind, DerivedKind::Tool { step, .. } if step == reading.step)
                    });
                    self.derived.extend(planes.into_iter().map(Arc::new));
                    self.late.shown.insert(reading.step, reading.identity);
                    changed = true;
                }
                Ok(_) => {}
                Err(error) => self.say(&format!("A tool's result was not read: {error}")),
            }
        }
        if self.late.reading.is_none()
            && let Some(step) = steps.iter().find(|step| {
                step.enabled
                    && self.late.shown.get(&step.id) != Some(&step.identity)
                    && result_path(&store, &step.identity).is_file()
            })
        {
            let done = Arc::new(Mutex::new(None));
            let (task, path, step_record, levels) = (
                done.clone(),
                store.clone(),
                step.clone(),
                usize::from(self.levels),
            );
            std::thread::Builder::new()
                .name("read tool result".into())
                .spawn(move || {
                    let result = read_result(&path, &step_record, levels);
                    *task.lock().expect("late result") = Some(result);
                })
                .expect("spawning a task");
            self.late.reading = Some(Reading {
                step: step.id,
                identity: step.identity.clone(),
                done,
            });
        }
        if changed {
            self.show_late_planes();
        }
    }

    /// Runs the late tool steps whose result is missing, one after another
    /// in the order they run, each on the image the steps before it leave.
    /// Shares the progress bar and Stop of a run of the tools.
    pub(super) fn run_late_tools(&mut self) {
        let pending = self.late_to_run();
        let (Some(binary), Some(store)) =
            (self.psf_guard(), self.store_path().map(Path::to_path_buf))
        else {
            return;
        };
        if pending.is_empty() || self.is_running() {
            return;
        }
        let document = self.document.clone();
        let base = self.base_bands.clone();
        let planes: Vec<Arc<DerivedPlane>> = self.derived.clone();
        let picture_samples = self.picture_samples.clone();
        let resolved = self.scene_worker.resolved();
        let levels = usize::from(self.levels);
        self.start_job(move |run| {
            let mut planes = planes;
            let folder = folder(&store);
            std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
            for step in &pending {
                // The image the tool takes, as the steps before it leave it.
                super::tools::set_line(run, &format!("{}: rendering its image", step.words));
                let image = render_input(
                    &document,
                    &base,
                    &planes,
                    &picture_samples,
                    &resolved,
                    &store,
                    step,
                )?;
                let input = folder.join(format!("{}.input.fits", step.identity));
                write_fits(&input, &image)?;
                let mut command = Command::new(&binary);
                command
                    .arg("studio-tool")
                    .arg("--input")
                    .arg(&input)
                    .arg("--tool")
                    .arg(step.tool.to_string())
                    .arg("--output")
                    .arg(result_path(&store, &step.identity))
                    .arg("--cache")
                    .arg(folder.join("cache"));
                if step.removes_stars() {
                    command
                        .arg("--stars")
                        .arg(stars_path(&store, &step.identity));
                }
                if step.shifted() {
                    command.arg("--shifted");
                }
                let ran = super::tools::run_program(run, command);
                if std::env::var_os("PSF_STUDIO_KEEP_TOOL_INPUTS").is_none() {
                    let _ = std::fs::remove_file(&input);
                }
                ran.map_err(|error| format!("{}: {error}", step.words))?;
                // The steps after it read its result.
                planes.retain(|plane| {
                    !matches!(plane.key.kind, DerivedKind::Tool { step: id, .. } if id == step.id)
                });
                planes.extend(read_result(&store, step, levels)?.into_iter().map(Arc::new));
            }
            Ok(())
        });
    }

    /// Draws the store with the planes worked out from it and the late
    /// tools' results.
    fn show_late_planes(&mut self) {
        self.show_derived_planes();
    }
}

/// The image a late step's tool takes, rendered at full resolution: the
/// layer alone in gray, as one channel, or the picture in the working
/// space, as three.
fn render_input(
    document: &Document,
    base: &Arc<crate::store_tiles::Bands>,
    planes: &[Arc<DerivedPlane>],
    picture_samples: &Option<(u64, Arc<Vec<f32>>)>,
    resolved: &crate::output_stretch::Resolved,
    store: &Path,
    step: &LateStep,
) -> Result<seiza_stacking_image::Image, String> {
    let place = match step.on {
        LateOn::Beneath(layer) => {
            let layers = document.layers();
            let at = layers
                .iter()
                .position(|other| other.id == layer)
                .ok_or("its layer is gone")?;
            // The layer right beneath, in the same group or at the top.
            let beneath = at
                .checked_sub(1)
                .filter(|beneath| layers[*beneath].group == layers[at].group)
                .ok_or("there is no layer beneath it to run on")?;
            match layers[at].group {
                // In a group: its members beneath, on their own.
                Some(_) => {
                    let view = crate::studio_model::View {
                        subject: crate::studio_model::Subject::MembersUpTo(layers[beneath].id),
                        up_to: crate::studio_model::UpTo::EndOf(StageKind::Combine),
                        stretch: crate::studio_model::DisplayStretch::Document,
                        own_until: None,
                    };
                    return render_view(
                        document,
                        base,
                        planes,
                        picture_samples,
                        resolved,
                        store,
                        step,
                        &view,
                    );
                }
                None => Place::Layer {
                    layer: layers[beneath].id,
                    alone: false,
                },
            }
        }
        LateOn::Layer(layer) => {
            let owner = document.layers().iter().find(|other| other.id == layer);
            let before = owner.and_then(|owner| {
                let at = owner.steps.iter().position(|other| other.id == step.id)?;
                at.checked_sub(1).map(|before| owner.steps[before].id)
            });
            Place::LayerEdit {
                layer,
                after: before,
            }
        }
        LateOn::Picture => {
            let finish = &document.stage(StageKind::Finish).steps;
            let at = finish
                .iter()
                .position(|other| other.id == step.id)
                .unwrap_or(0);
            match at.checked_sub(1) {
                Some(before) => Place::Finish(finish[before].id),
                None => Place::Combined,
            }
        }
    };
    render_view(
        document,
        base,
        planes,
        picture_samples,
        resolved,
        store,
        step,
        &place.view(),
    )
}

/// The image a late step's tool takes, of `view`, rendered at full
/// resolution, as [`render_input`] describes.
#[allow(clippy::too_many_arguments)]
fn render_view(
    document: &Document,
    base: &Arc<crate::store_tiles::Bands>,
    planes: &[Arc<DerivedPlane>],
    picture_samples: &Option<(u64, Arc<Vec<f32>>)>,
    resolved: &crate::output_stretch::Resolved,
    store: &Path,
    step: &LateStep,
    view: &crate::studio_model::View,
) -> Result<seiza_stacking_image::Image, String> {
    let bands = base.with_derived(planes);
    let (mut scene, states) = super::panes::scene(
        document,
        &bands,
        view,
        0,
        picture_samples
            .as_ref()
            .map(|(id, samples)| (*id, samples.as_slice())),
        &mut crate::output_stretch::ResolvedChains::from_resolved(resolved),
        &[],
    );
    if !scene.wanted_planes.is_empty() {
        return Err(
            "planes it needs are still being worked out; run the tools again in a moment"
                .to_string(),
        );
    }
    scene.stretches = Arc::new(bands.stretches(&states)?);
    let picture = scene.picture(&[]);
    let (tiles, _) = crate::store_tiles::open_store(store)?;
    let pyramid = psf_studio_render::TileSource::pyramid(&tiles);
    let region = seiza_compose::ReferenceRegion {
        x: 0,
        y: 0,
        width: pyramid.width as usize,
        height: pyramid.height as usize,
    };
    let source = crate::store_tiles::StoreSource::new(tiles).with_planes(planes.to_vec());
    let rendered = seiza_compose::render::render_image(
        &picture,
        &source,
        region,
        &seiza_compose::render::RenderOptions::default(),
    )
    .map_err(|error| error.to_string())?;
    let (width, height) = (rendered.width, rendered.height);
    Ok(match step.on {
        // A layer alone is gray: its first channel is the layer.
        LateOn::Layer(_) => seiza_stacking_image::Image {
            width,
            height,
            channels: 1,
            data: rendered
                .data
                .iter()
                .step_by(rendered.channels)
                .copied()
                .collect(),
        },
        LateOn::Beneath(_) | LateOn::Picture => seiza_stacking_image::Image {
            width,
            height,
            channels: rendered.channels,
            data: rendered.data,
        },
    })
}

/// An image as a 32-bit FITS file, written beside its place and renamed.
fn write_fits(path: &Path, image: &seiza_stacking_image::Image) -> Result<(), String> {
    let data = match image.channels {
        1 => seiza_fits::F32ImageData::Mono(&image.data),
        3 => seiza_fits::F32ImageData::RgbInterleaved(&image.data),
        other => return Err(format!("an image of {other} channels")),
    };
    let partial = path.with_extension("partial.fits");
    seiza_fits::write_f32_image(&partial, image.width, image.height, data, &[])
        .map_err(|error| format!("writing {}: {error}", path.display()))?;
    std::fs::rename(&partial, path).map_err(|error| error.to_string())
}

/// An image of one channel or three, interleaved.
mod seiza_stacking_image {
    pub struct Image {
        pub width: usize,
        pub height: usize,
        pub channels: usize,
        pub data: Vec<f32>,
    }
}

/// Whether a star removal step adds its stars back right after it.
pub(super) fn stars_shown(step: &StepRecord) -> bool {
    tool_of(step).is_none_or(|tool| tool[STARS_SHOWN_KEY].as_bool().unwrap_or(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio_model::{tool_step, StepKind};

    fn identity(document: &Document, step: Uuid) -> String {
        late_steps(document, Path::new("/nonexistent/picture.pgtiles"))
            .into_iter()
            .find(|late| late.id == step)
            .map(|late| late.identity)
            .unwrap()
    }

    #[test]
    fn a_late_result_follows_what_comes_before_its_step_only() {
        let inputs = ["ha".to_string(), "oiii".to_string()];
        let mut document = super::super::panes::flow_document(&inputs);
        let curves = |amount: f64| {
            StepRecord::new(StepKind::Operation {
                operation: "Curves".to_string(),
                accepts: crate::image_steps::Accepts::Display,
                settings: serde_json::json!({ "amount": amount }),
            })
        };
        let before = curves(0.2);
        let before_id = before.id;
        document
            .insert_step(StageKind::Finish, usize::MAX, before)
            .unwrap();
        let noise = tool_step(serde_json::json!({ "provider": "nxt" }));
        let noise_id = noise.id;
        document
            .insert_step(StageKind::Finish, usize::MAX, noise)
            .unwrap();
        let after = curves(0.5);
        let after_id = after.id;
        document
            .insert_step(StageKind::Finish, usize::MAX, after)
            .unwrap();
        let first = identity(&document, noise_id);
        // A step after it does not change what it ran on.
        document.set_step_settings(after_id, serde_json::json!({ "amount": 0.9 }));
        assert_eq!(identity(&document, noise_id), first);
        // A step before it does, and so do its own settings.
        document.set_step_settings(before_id, serde_json::json!({ "amount": 0.3 }));
        let second = identity(&document, noise_id);
        assert_ne!(second, first);
        document.set_step_settings(
            noise_id,
            serde_json::json!({ "provider": "nxt", "parameters": { "denoise": 0.5 } }),
        );
        let third = identity(&document, noise_id);
        assert_ne!(third, second);
        // Showing or leaving out its stars runs nothing again.
        document.set_step_settings(
            noise_id,
            serde_json::json!({ "provider": "nxt", "parameters": { "denoise": 0.5 }, "stars_shown": false }),
        );
        assert_eq!(identity(&document, noise_id), third);

        // A layer's tool follows that layer, not the others.
        let layers: Vec<Uuid> = document.layers().iter().map(|layer| layer.id).collect();
        let tool = tool_step(serde_json::json!({ "provider": "nxt" }));
        let tool_id = tool.id;
        document.insert_layer_step(layers[0], tool).unwrap();
        let on_layer = identity(&document, tool_id);
        if let Some(other) = document.layer_mut(layers[1]) {
            other.opacity = 0.5;
        }
        assert_eq!(identity(&document, tool_id), on_layer);
        if let Some(own) = document.layer_mut(layers[0]) {
            own.opacity = 0.5;
        }
        assert_ne!(identity(&document, tool_id), on_layer);
        let late = late_steps(&document, Path::new("/nonexistent/picture.pgtiles"));
        assert_eq!(late[0].on, LateOn::Layer(layers[0]));
        assert_eq!(
            late[0].words,
            format!("Noise reduction on {}", document.layers()[0].name)
        );
        assert!(late[1..].iter().all(|step| step.on == LateOn::Picture));
    }

    #[test]
    fn a_tool_layer_runs_on_the_layers_beneath_it() {
        let inputs = ["ha".to_string(), "oiii".to_string()];
        let mut document = super::super::panes::flow_document(&inputs);
        let bottom = document.layers()[0].id;
        let tool = tool_step(serde_json::json!({ "provider": "sxt" }));
        let tool_id = tool.id;
        let layer = document.add_picture_step(tool);
        // A star removal adds a layer of the stars it took out above it.
        let top = document.layers().last().unwrap();
        assert!(matches!(
            top.source,
            crate::studio_model::LayerSource::Step { step, .. } if step == tool_id
        ));
        let late = late_steps(&document, Path::new("/nonexistent/picture.pgtiles"));
        let found = late.iter().find(|late| late.id == tool_id).unwrap();
        assert_eq!(found.on, LateOn::Beneath(layer));
        let first = identity(&document, tool_id);
        // A layer above it, its stars among them, does not change its input.
        let stars = document.layers().last().unwrap().id;
        document.layer_mut(stars).unwrap().visible = false;
        assert_eq!(identity(&document, tool_id), first);
        // A layer beneath it does.
        document.layer_mut(bottom).unwrap().opacity = 0.5;
        assert_ne!(identity(&document, tool_id), first);
    }

    #[test]
    fn a_tool_in_a_group_runs_on_its_members_beneath_it() {
        use crate::studio_model::{LayerRecord, LayerSource};
        let inputs = ["ha".to_string(), "oiii".to_string()];
        let mut document = super::super::panes::flow_document(&inputs);
        let bands: Vec<Uuid> = document
            .layers()
            .iter()
            .filter(|layer| matches!(layer.source, LayerSource::Band { .. }))
            .map(|layer| layer.id)
            .collect();
        let (outside, member) = (bands[0], bands[1]);
        // A group holding the second band layer, at the top.
        let group = LayerRecord::new("Group", LayerSource::Group);
        let group_id = group.id;
        let top = document.layers().len();
        document.insert_layer(top, group).unwrap();
        let at = document
            .layers()
            .iter()
            .position(|layer| layer.id == group_id)
            .unwrap();
        document.place_layer(member, at, Some(group_id));
        let tool = tool_step(serde_json::json!({ "provider": "nxt" }));
        let tool_id = tool.id;
        let mut layer = crate::studio_model::adjustment_layer(tool);
        layer.group = Some(group_id);
        let at = document
            .layers()
            .iter()
            .position(|layer| layer.id == group_id)
            .unwrap();
        document.insert_layer(at, layer).unwrap();
        let late = late_steps(&document, Path::new("/nonexistent/picture.pgtiles"));
        let found = late.iter().find(|late| late.id == tool_id).unwrap();
        assert!(
            found.words.contains("members beneath it"),
            "{}",
            found.words
        );
        let first = identity(&document, tool_id);
        // A layer outside the group does not change what it runs on.
        document.layer_mut(outside).unwrap().opacity = 0.3;
        assert_eq!(identity(&document, tool_id), first);
        // Its member beneath it does.
        document.layer_mut(member).unwrap().opacity = 0.3;
        assert_ne!(identity(&document, tool_id), first);
    }
}
