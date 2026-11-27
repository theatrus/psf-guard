//! A local control socket, so a program can drive Studio and see what it
//! shows: `psf-studio-mcp` uses it to give an assistant the same view a
//! person has.
//!
//! Each running Studio listens on a Unix socket of its own,
//! `$TMPDIR/psf-studio/<pid>.sock`, which only this user can open. A
//! request is one line of JSON, `{"id": 1, "method": "state", "params":
//! {}}`, and the reply is one line, `{"id": 1, "result": …}` or `{"id": 1,
//! "error": "…"}`. Requests are handled on the UI thread, as a person's
//! clicks are; a capture or a sample is finished on a thread of its own and
//! answered when it is done.

use super::panes::scene;
use super::Studio;
use crate::store_tiles::StoreSource;
use crate::studio_model::{
    channel_label, input_steps, Place, Row, Split, StageKind, StepKind, StepRecord,
};
use psf_studio_render::Captured;
use seiza_compose::render::{render_image, RenderOptions};
use seiza_compose::ReferenceRegion;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

type Reply = Sender<Result<Value, String>>;

/// A request read from the socket, with the way back to its sender.
struct Request {
    method: String,
    params: Value,
    reply: Reply,
}

/// A capture asked for and not yet drawn.
struct PendingCapture {
    id: u64,
    reply: Reply,
    pane: Option<usize>,
    divide: f32,
    max_width: u32,
}

/// The UI thread's side of the control socket.
pub(super) struct Control {
    incoming: Receiver<Request>,
    captures: Vec<PendingCapture>,
    next_capture: u64,
    pub path: PathBuf,
}

/// Where this user's Studio sockets and captures are kept:
/// `PSF_STUDIO_CONTROL_DIR`, or `psf-studio` in the temporary directory.
pub(super) fn control_dir() -> PathBuf {
    std::env::var_os("PSF_STUDIO_CONTROL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("psf-studio"))
}

impl Control {
    /// Listens on this process's socket. `None`, with the reason printed,
    /// when the socket cannot be made.
    pub(super) fn start() -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;
        let dir = control_dir();
        let made = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)));
        if let Err(error) = made {
            eprintln!("control: could not make {}: {error}", dir.display());
            return None;
        }
        let path = dir.join(format!("{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("control: could not listen on {}: {error}", path.display());
                return None;
            }
        };
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        let (send, incoming) = channel();
        std::thread::Builder::new()
            .name("control".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let send = send.clone();
                    let _ = std::thread::Builder::new()
                        .name("control connection".into())
                        .spawn(move || serve(stream, &send));
                }
            })
            .ok()?;
        Some(Self {
            incoming,
            captures: Vec::new(),
            next_capture: 1,
            path,
        })
    }
}

/// Answers one connection's requests, one line each, until it closes.
fn serve(stream: UnixStream, send: &Sender<Request>) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else {
            return;
        };
        if line.trim().is_empty() {
            continue;
        }
        let (id, answer) = match serde_json::from_str::<Value>(&line) {
            Ok(request) => {
                let id = request["id"].clone();
                let (reply, answer) = channel();
                let sent = send.send(Request {
                    method: request["method"].as_str().unwrap_or_default().to_string(),
                    params: request["params"].clone(),
                    reply,
                });
                let answer = match sent {
                    Ok(()) => answer
                        .recv_timeout(Duration::from_secs(300))
                        .unwrap_or_else(|_| Err("Studio did not answer in time".to_string())),
                    Err(_) => Err("Studio is closing".to_string()),
                };
                (id, answer)
            }
            Err(error) => (Value::Null, Err(format!("not JSON: {error}"))),
        };
        let line = match answer {
            Ok(result) => json!({ "id": id, "result": result }),
            Err(error) => json!({ "id": id, "error": error }),
        };
        if writeln!(writer, "{line}").is_err() {
            return;
        }
    }
}

/// A step's settings, or null for a step that has none.
fn settings_of(step: &StepRecord) -> Value {
    match &step.kind {
        StepKind::Operation { settings, .. } => settings.clone(),
        _ => Value::Null,
    }
}

/// `patch` merged into `base`: objects key by key, a `null` removing its
/// key, and anything else replaced. An object whose `type` the patch
/// changes, such as a stretch model of another kind, is replaced whole, so
/// the old kind's settings do not stay behind.
fn merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(base), Value::Object(patch))
            if patch
                .get("type")
                .is_none_or(|kind| base.get("type").is_none_or(|was| was == kind)) =>
        {
            for (key, value) in patch {
                if value.is_null() {
                    base.remove(key);
                } else {
                    merge(base.entry(key.clone()).or_insert(Value::Null), value);
                }
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

fn uuid_param(params: &Value, name: &str) -> Result<uuid::Uuid, String> {
    params[name]
        .as_str()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| format!("`{name}` must be a step or layer id"))
}

impl Studio {
    /// Answers the requests the control socket has read, and the captures
    /// the render thread has drawn.
    pub(super) fn follow_control(&mut self) {
        let Some(control) = self.control.as_mut() else {
            return;
        };
        let requests: Vec<Request> = control.incoming.try_iter().collect();
        for request in requests {
            let answer = self.handle(&request.method, &request.params, &request.reply);
            if let Some(answer) = answer {
                let _ = request.reply.send(answer);
            }
        }
        self.follow_captures();
    }

    /// One request's answer, or `None` when it is answered later.
    fn handle(
        &mut self,
        method: &str,
        params: &Value,
        reply: &Reply,
    ) -> Option<Result<Value, String>> {
        Some(match method {
            "info" => Ok(self.control_info()),
            "state" => Ok(self.control_state()),
            "busy" => Ok(json!({ "busy": self.busy() })),
            "select" => serde_json::from_value::<Row>(params["row"].clone())
                .map_err(|error| format!("`row` is not a row: {error}"))
                .map(|row| {
                    self.select(row);
                    self.refresh_canvas();
                    json!({ "selected": self.selected })
                }),
            "set_split" => serde_json::from_value::<Split>(params["split"].clone())
                .map_err(|error| format!("`split` is not a split: {error}"))
                .map(|split| {
                    self.set_split(split);
                    json!({ "split": split })
                }),
            "set_step" => self.control_set_step(params),
            "move_layer" => match uuid_param(params, "layer") {
                Ok(id) => {
                    let up = params["up"].as_bool().unwrap_or(true);
                    let moved = self.document.step_layer(id, up);
                    self.refresh_canvas();
                    Ok(json!({ "moved": moved }))
                }
                Err(error) => Err(error),
            },
            "show_clipping" => {
                let on = params["on"].as_bool().unwrap_or(true);
                self.canvas.edit(|canvas| canvas.show_clipping = on);
                Ok(json!({ "show_clipping": on }))
            }
            "export" => match params["path"].as_str() {
                Some(path) => {
                    let format = params["format"]
                        .as_str()
                        .and_then(|format| {
                            super::export::EXPORT_FORMATS
                                .iter()
                                .find(|(known, _)| *known == format)
                                .map(|(known, _)| *known)
                        })
                        .unwrap_or_else(|| self.export_format());
                    let path = super::export::with_format(PathBuf::from(path), format);
                    self.start_export(path.clone());
                    Ok(json!({ "path": path, "status": self.export_line }))
                }
                None => Err("`path` is needed".to_string()),
            },
            "undo" | "redo" => {
                if method == "undo" {
                    self.undo();
                } else {
                    self.redo();
                }
                Ok(json!({
                    "can_undo": self.history_can(false),
                    "can_redo": self.history_can(true),
                }))
            }
            // A Linear step moved past its neighbour in one input's list.
            "move_step" => match (
                params["step"]
                    .as_str()
                    .and_then(|id| id.parse::<uuid::Uuid>().ok()),
                params["channel"].as_str(),
            ) {
                (Some(step), Some(channel)) => {
                    let later = params["later"].as_bool().unwrap_or(true);
                    let moved = crate::studio_model::move_linear_step(
                        &mut self.document,
                        step,
                        channel,
                        later,
                    );
                    self.refresh_canvas();
                    Ok(json!({ "moved": moved }))
                }
                _ => Err("`step` (an id) and `channel` are needed".to_string()),
            },
            // A processing tool added as a step of an input or of a color
            // group, where a new tool goes.
            "add_tool" => match (params["band"].as_str(), params["provider"].as_str()) {
                (Some(band), Some(provider)) => {
                    let tool = json!({
                        "provider": provider,
                        "parameters": params.get("parameters").cloned().unwrap_or_else(|| json!({})),
                    });
                    let operation = crate::studio_model::tool_operation(provider);
                    let at =
                        crate::studio_model::new_tool_step_index(&self.document, band, operation);
                    let step = crate::studio_model::tool_step(tool).for_band(band);
                    let id = step.id;
                    match self.document.insert_step(StageKind::Linear, at, step) {
                        Ok(()) => {
                            self.refresh_canvas();
                            Ok(json!({ "step": id.to_string(), "index": at }))
                        }
                        Err(error) => Err(error.to_string()),
                    }
                }
                _ => Err("`band` and `provider` are needed".to_string()),
            },
            // A step added at the end of a stage, as the inspector's Add
            // step does: a Finish step, a stretch of an input, and so on.
            "add_step" => {
                let stage = params["stage"]
                    .as_str()
                    .ok_or_else(|| "`stage` is needed".to_string())
                    .and_then(|stage| {
                        stage
                            .parse::<StageKind>()
                            .map_err(|error| error.to_string())
                    });
                let operation = params["operation"].as_str();
                match (stage, operation) {
                    (Ok(stage), Some(operation)) => {
                        let accepts = if stage.works_on() == crate::image_steps::Domain::Linear {
                            crate::image_steps::Accepts::Either
                        } else {
                            crate::image_steps::Accepts::Display
                        };
                        let mut step = StepRecord::new(StepKind::Operation {
                            operation: operation.to_string(),
                            accepts,
                            settings: params.get("settings").cloned().unwrap_or(Value::Null),
                        });
                        if let Some(band) = params["band"].as_str() {
                            step = step.for_band(band);
                        }
                        // A layer's own step, given the layer.
                        let layer = params["layer"]
                            .as_str()
                            .and_then(|id| id.parse::<uuid::Uuid>().ok());
                        let mask = params
                            .get("mask")
                            .filter(|mask| !mask.is_null())
                            .map(|mask| serde_json::from_value(mask.clone()));
                        match mask {
                            Some(Err(error)) => Err(format!("`mask` is not a mask: {error}")),
                            mask => {
                                step.mask = mask.and_then(Result::ok);
                                let id = step.id;
                                // A step on the picture is an adjustment
                                // layer on top of every layer.
                                let inserted = match layer {
                                    Some(layer) => {
                                        self.document.insert_layer_step(layer, step).map(|()| None)
                                    }
                                    None if stage == StageKind::Finish => {
                                        Ok(Some(self.document.add_picture_step(step)))
                                    }
                                    None => self
                                        .document
                                        .insert_step(stage, usize::MAX, step)
                                        .map(|()| None),
                                };
                                match inserted {
                                    Ok(layer) => {
                                        self.refresh_canvas();
                                        Ok(json!({
                                            "step": id.to_string(),
                                            "layer": layer.map(|layer| layer.to_string()),
                                        }))
                                    }
                                    Err(error) => Err(error.to_string()),
                                }
                            }
                        }
                    }
                    (Err(error), _) => Err(error),
                    (_, None) => Err("`operation` is needed".to_string()),
                }
            }
            "remove_step" => match params["step"]
                .as_str()
                .and_then(|id| id.parse::<uuid::Uuid>().ok())
            {
                Some(step) => {
                    let removed = self.document.remove_step(step).is_some();
                    if self.selected_step() == Some(step) {
                        self.selected = Row::Combined;
                    }
                    self.refresh_canvas();
                    Ok(json!({ "removed": removed }))
                }
                None => Err("`step` (an id) is needed".to_string()),
            },
            "set_headroom" => {
                // A number of stops makes the picture HDR; null makes it SDR.
                let stops = params["stops"]
                    .as_f64()
                    .map(|stops| stops.clamp(0.0, 4.0) as f32);
                self.document.headroom = stops.filter(|stops| *stops > 0.0);
                // The knee, when given, as the HDR block's Knee sets it.
                if let Some(knee) = params["knee"].as_f64() {
                    let (_, _, low, high) = crate::tone_step::HDR_KNEE;
                    self.document.hdr_knee = Some((knee as f32).clamp(low, high));
                }
                self.refresh_canvas();
                Ok(json!({
                    "headroom": self.document.headroom,
                    "knee": self.document.hdr_knee.unwrap_or(crate::tone_step::DEFAULT_HDR_KNEE),
                }))
            }
            "use_recipe" => {
                let name = params["name"].as_str().unwrap_or_default();
                match crate::studio_model::recipes_for(&self.inputs())
                    .into_iter()
                    .find(|recipe| recipe.name == name)
                {
                    Some(recipe) => {
                        self.choose_recipe(recipe);
                        self.refresh_canvas();
                        Ok(json!({ "recipe": recipe.name, "status": self.run_status() }))
                    }
                    None => Err(format!(
                        "no recipe `{name}` fits these inputs; these do: {:?}",
                        crate::studio_model::recipes_for(&self.inputs())
                            .iter()
                            .map(|recipe| recipe.name)
                            .collect::<Vec<_>>()
                    )),
                }
            }
            "add_part" => {
                let name = params["kind"].as_str().unwrap_or_default();
                let inputs = self.inputs();
                match crate::studio_model::PartKind::ALL
                    .into_iter()
                    .find(|kind| kind.label() == name && kind.fits(&inputs))
                {
                    Some(kind) => {
                        self.add_part_of(kind);
                        self.refresh_canvas();
                        Ok(json!({ "added": kind.label(), "selected": self.selected }))
                    }
                    None => Err(format!("no part `{name}` fits these inputs")),
                }
            }
            "set_tool" => {
                let input = params["input"].as_str().unwrap_or_default().to_string();
                let category = params["category"].as_str().unwrap_or_default().to_string();
                if !self.document.channels().contains(&input) {
                    Err(format!("no input `{input}`"))
                } else if !["sharpen", "denoise", "star_removal"].contains(&category.as_str()) {
                    Err("`category` is sharpen, denoise or star_removal".to_string())
                } else {
                    self.set_tool(&input, &category, params["provider"].as_str());
                    self.refresh_canvas();
                    Ok(json!({ "tools_not_run": self.inputs_to_run() }))
                }
            }
            "set_layer" => self.control_set_layer(params),
            "run_tools" => {
                if let Some(why) = self.cannot_run() {
                    Err(why.to_string())
                } else {
                    let running = self.is_running();
                    self.run_tools();
                    Ok(json!({ "started": !running, "queued": self.run_is_queued() }))
                }
            }
            "capture" => return self.start_capture(params, reply.clone()),
            "sample" => return self.start_sample(params, reply.clone()),
            other => Err(format!("unknown method `{other}`")),
        })
    }

    fn control_info(&self) -> Value {
        json!({
            "pid": std::process::id(),
            "document": self.document_path(),
            "store": self.store_path(),
            "title": self.window_title,
            "socket": self.control.as_ref().map(|control| control.path.clone()),
        })
    }

    /// What Studio shows: the rows a person can select with their ids, the
    /// selection, each pane's label, what is running and what needs a run.
    fn control_state(&self) -> Value {
        let document = &self.document;
        let inputs: Vec<Value> = document
            .channels()
            .into_iter()
            .map(|channel| {
                let steps: Vec<Value> = input_steps(document, &channel)
                    .into_iter()
                    .filter_map(|listed| {
                        let (stage, step) = document.step(listed.id)?;
                        Some(json!({
                            "row": Row::InputStep { channel: channel.clone(), step: listed.id },
                            "name": step.kind.name(),
                            "stage": format!("{stage:?}"),
                            "band": step.band,
                            "enabled": step.enabled,
                            "shared": listed.shared,
                            "settings": settings_of(step),
                        }))
                    })
                    .collect();
                json!({
                    "row": Row::Input(channel.clone()),
                    "label": channel_label(&channel),
                    "goes_to": self.goes_to(&channel),
                    "steps": steps,
                })
            })
            .collect();
        // Top first, as the layer list shows them.
        let layers: Vec<Value> = document
            .layers()
            .iter()
            .rev()
            .map(|layer| {
                let mut value = serde_json::to_value(layer).unwrap_or(Value::Null);
                value["row"] = json!(Row::Layer(layer.id));
                value
            })
            .collect();
        let finish: Vec<Value> = document
            .stage(StageKind::Finish)
            .steps
            .iter()
            .map(|step| {
                json!({
                    "row": Row::Finish(step.id),
                    "name": step.kind.name(),
                    "enabled": step.enabled,
                    "settings": settings_of(step),
                })
            })
            .collect();
        let (drawn, omitted): (Vec<Option<Place>>, Vec<Vec<String>>) = self.canvas.read(|shared| {
            shared
                .canvas
                .scenes
                .iter()
                .map(|scene| (scene.place.clone(), scene.omitted.clone()))
                .unzip()
        });
        let panes: Vec<Value> = self
            .places
            .iter()
            .enumerate()
            .map(|(index, wanted)| {
                let drawn = drawn.get(index).cloned().flatten();
                json!({
                    "pane": index,
                    "wanted": wanted.label(document),
                    "drawn": drawn.as_ref().map(|place| place.label(document)),
                    "left_out": omitted.get(index).cloned().unwrap_or_default(),
                })
            })
            .collect();
        let histogram = self.histogram.as_ref().map(|histogram| {
            json!({
                "clipped_black": histogram.black,
                "clipped_white": histogram.white,
                "above_sdr_white": histogram.above,
                "masked": histogram.masked,
            })
        });
        json!({
            "info": self.control_info(),
            "selected": self.selected,
            "split": self.split,
            "headroom": document.headroom,
            "can_undo": self.history_can(false),
            "can_redo": self.history_can(true),
            "panes": panes,
            "inputs": inputs,
            "layers": layers,
            "finish": finish,
            "status": self.run_status(),
            "export": {
                "running": self.export.is_some(),
                "status": self.export_line,
                "format": self.export_format(),
            },
            "progress": self.run_progress().map(|(what, share, elapsed)| json!({
                "what": what, "share": share, "seconds": elapsed.as_secs_f32(),
            })),
            "tools_not_run": self.inputs_to_run(),
            "tool_steps_not_run": self
                .late_to_run()
                .into_iter()
                .map(|step| step.words)
                .collect::<Vec<_>>(),
            "histogram": histogram,
            "busy": self.busy(),
        })
    }

    /// What Studio is still doing before the canvas shows the document as
    /// it is: empty when it is idle.
    pub(super) fn busy(&self) -> Vec<String> {
        let mut busy = Vec::new();
        if self.scene_worker.busy() {
            busy.push("building scenes".to_string());
        }
        let (drawn, stats, dirty) = self.canvas.read(|shared| {
            (
                shared
                    .canvas
                    .scenes
                    .iter()
                    .map(|scene| scene.place.clone())
                    .collect::<Vec<_>>(),
                shared.readout.stats,
                shared.dirty,
            )
        });
        if drawn.len() != self.places.len()
            || drawn
                .iter()
                .zip(&self.places)
                .any(|(drawn, wanted)| drawn.as_ref() != Some(wanted))
        {
            busy.push("panes waiting for their scenes".to_string());
        }
        if dirty {
            busy.push("a frame to draw".to_string());
        }
        let loading = stats.tiles_coarse + stats.tiles_in_flight;
        if loading > 0 {
            busy.push(format!("{loading} tiles loading"));
        }
        if let Some((what, _, _)) = self.run_progress() {
            busy.push(what);
        }
        if self.late_reading() {
            busy.push("reading a tool's result".to_string());
        }
        if self.run_is_queued() {
            busy.push("a run of the tools to start".to_string());
        }
        for task in &self.derive_tasks {
            busy.push(task.describe().to_string());
        }
        // A plane a scene reads that no task has started yet.
        let waiting = self.canvas.read(|shared| {
            shared
                .canvas
                .scenes
                .iter()
                .flat_map(|scene| scene.missing_planes())
                .filter(|key| self.derived.iter().all(|plane| plane.key != *key))
                .count()
        });
        if self.derive_tasks.is_empty() && waiting > 0 {
            busy.push(format!("{waiting} planes to work out"));
        }
        if self.export.is_some() {
            busy.push(self.export_line.clone());
        }
        if self.picture_wanted.is_some()
            && self.picture_samples.as_ref().map(|(id, _)| *id) != self.picture_wanted
        {
            busy.push("sampling the picture".to_string());
        }
        if self.final_wanted.is_some()
            && self.final_samples.as_ref().map(|(id, _)| *id) != self.final_wanted
        {
            busy.push("sampling the final picture for the histogram".to_string());
        }
        busy
    }

    /// Merges `settings` into a step's settings, and turns it on or off.
    fn control_set_step(&mut self, params: &Value) -> Result<Value, String> {
        let id = uuid_param(params, "step")?;
        let (_, step) = self.document.step(id).ok_or("no step has that id")?;
        if !params["settings"].is_null() {
            let mut settings = settings_of(step);
            if settings.is_null() {
                return Err("that step has no settings".to_string());
            }
            // A stretch switched to another kind starts from that kind's
            // settings, as the inspector's choice does.
            let kind = params["settings"]["model"]["type"].as_str();
            if let Some(kind) = kind.filter(|kind| settings["model"]["type"] != *kind) {
                settings["model"] = super::panes::new_stretch(kind);
            }
            merge(&mut settings, &params["settings"]);
            if !self.document.set_step_settings(id, settings) {
                return Err("the settings were refused".to_string());
            }
        }
        if let Some(enabled) = params["enabled"].as_bool() {
            self.document.set_step_enabled(id, enabled);
        }
        self.refresh_canvas();
        let (_, step) = self.document.step(id).ok_or("the step is gone")?;
        Ok(json!({ "enabled": step.enabled, "settings": settings_of(step) }))
    }

    /// Merges `patch` into a layer, as the document stores it: opacity,
    /// visible, blend, color and the rest. The layer keeps its id.
    fn control_set_layer(&mut self, params: &Value) -> Result<Value, String> {
        let id = uuid_param(params, "layer")?;
        let layer = self.document.layer_mut(id).ok_or("no layer has that id")?;
        let mut value = serde_json::to_value(&*layer).map_err(|error| error.to_string())?;
        merge(&mut value, &params["patch"]);
        value["id"] = json!(id);
        *layer = serde_json::from_value(value)
            .map_err(|error| format!("the layer would not be valid: {error}"))?;
        let value = serde_json::to_value(&*layer).map_err(|error| error.to_string())?;
        self.refresh_canvas();
        Ok(value)
    }

    /// Asks the render thread to copy its next frame.
    fn start_capture(&mut self, params: &Value, reply: Reply) -> Option<Result<Value, String>> {
        let control = self.control.as_mut()?;
        let id = control.next_capture;
        control.next_capture += 1;
        control.captures.push(PendingCapture {
            id,
            reply,
            pane: params["pane"].as_u64().map(|pane| pane as usize),
            divide: params["divide"].as_f64().unwrap_or(1.0) as f32,
            max_width: params["max_width"].as_u64().unwrap_or(1400) as u32,
        });
        self.canvas.edit(|canvas| canvas.control_capture = Some(id));
        None
    }

    /// Writes the frames the render thread copied to PNG files, on threads
    /// of their own, and answers their requests.
    fn follow_captures(&mut self) {
        let Some(control) = self.control.as_mut() else {
            return;
        };
        if control.captures.is_empty() {
            return;
        }
        let Some((id, captured)) = self
            .canvas
            .0
             .0
            .lock()
            .expect("canvas state")
            .control_captured
            .take()
        else {
            return;
        };
        let Some(at) = control.captures.iter().position(|pending| pending.id == id) else {
            return;
        };
        let pending = control.captures.remove(at);
        // A capture asked for later is drawn on a later frame.
        if !control.captures.is_empty() {
            let next = control.captures[0].id;
            self.canvas
                .edit(|canvas| canvas.control_capture = Some(next));
        }
        let region = self.canvas.read(|shared| match pending.pane {
            Some(pane) if pane < shared.canvas.scenes.len() => shared.canvas.pane_rects(pane).1,
            _ => [0, 0, captured.width, captured.height],
        });
        let label = pending
            .pane
            .and_then(|pane| self.places.get(pane))
            .map(|place| place.label(&self.document));
        let path = control_dir().join(format!("capture-{}-{id}.png", std::process::id()));
        std::thread::spawn(move || {
            let _ = pending
                .reply
                .send(write_capture(&captured, region, &pending, &path, label));
        });
    }

    /// Renders a region of the final picture on the CPU, as an export does,
    /// and answers with its values and a PNG of it.
    fn start_sample(&mut self, params: &Value, reply: Reply) -> Option<Result<Value, String>> {
        let Some(store) = self.store_path().map(Path::to_path_buf) else {
            return Some(Err(
                "only a picture opened from a store can be sampled".to_string()
            ));
        };
        let (width, height) = self.canvas.read(|shared| shared.canvas.document_size);
        let number = |name: &str, default: u64| params[name].as_u64().unwrap_or(default) as usize;
        let x = number("x", 0).min(width.saturating_sub(1) as usize);
        let y = number("y", 0).min(height.saturating_sub(1) as usize);
        let region = ReferenceRegion {
            x,
            y,
            width: number("width", 256).clamp(1, 2048).min(width as usize - x),
            height: number("height", 256)
                .clamp(1, 2048)
                .min(height as usize - y),
        };
        let divide = params["divide"].as_f64().unwrap_or(1.0) as f32;
        let view_stretch = self.view_stretch_stages();
        let (mut final_scene, states) = scene(
            &self.document,
            &self.band_info,
            &Place::Final.view(),
            self.scene_version,
            self.picture_samples
                .as_ref()
                .map(|(id, samples)| (*id, samples.as_slice())),
            &mut self.resolved_chains,
            &view_stretch,
        );
        final_scene.stretches = self.analysis(&states);
        let picture = final_scene.picture(&[]);
        let planes = self.derived.clone();
        let path = control_dir().join(format!(
            "sample-{}-{}-{x}-{y}.png",
            std::process::id(),
            self.scene_version
        ));
        std::thread::spawn(move || {
            let answer = (|| -> Result<Value, String> {
                let (store, _) = crate::store_tiles::open_store(&store)?;
                let source = StoreSource::new(store).with_planes(planes);
                let image = render_image(&picture, &source, region, &RenderOptions::default())
                    .map_err(|error| error.to_string())?;
                let peak = picture.peak();
                let shown = super::export::for_display(
                    &image.data,
                    seiza_compose::space::WorkingSpace::DisplayP3,
                );
                let rgb: Vec<u8> = shown
                    .iter()
                    .map(|value| ((value / divide.max(1e-6)).clamp(0.0, 1.0) * 255.0).round() as u8)
                    .collect();
                super::write_png(
                    &path.to_string_lossy(),
                    region.width as u32,
                    region.height as u32,
                    &rgb,
                )?;
                Ok(json!({
                    "region": [region.x, region.y, region.width, region.height],
                    "peak": peak,
                    "channels": channel_stats(&image.data, peak),
                    "png": path,
                    "png_divided_by": divide,
                }))
            })();
            let _ = reply.send(answer);
        });
        None
    }
}

/// Each channel's least, median, 99th percentile and largest value, and
/// the shares above SDR white and at the picture's peak.
fn channel_stats(rgb: &[f32], peak: f32) -> Value {
    let stats: Vec<Value> = (0..3)
        .map(|channel| {
            let mut values: Vec<f32> = rgb
                .iter()
                .skip(channel)
                .step_by(3)
                .copied()
                .filter(|value| value.is_finite())
                .collect();
            values.sort_by(f32::total_cmp);
            let count = values.len().max(1) as f32;
            let at = |share: f32| {
                values
                    .get(((values.len().saturating_sub(1)) as f32 * share) as usize)
                    .copied()
                    .unwrap_or(f32::NAN)
            };
            json!({
                "min": at(0.0),
                "median": at(0.5),
                "p99": at(0.99),
                "max": at(1.0),
                "above_sdr_white": values.iter().filter(|value| **value > 1.0).count() as f32 / count,
                "at_peak": values.iter().filter(|value| **value >= peak * 0.995).count() as f32 / count,
            })
        })
        .collect();
    json!({ "red": stats[0], "green": stats[1], "blue": stats[2] })
}

/// Writes a captured frame's region to a PNG file, at most `max_width`
/// wide, and says what it holds.
fn write_capture(
    captured: &Captured,
    region: [u32; 4],
    pending: &PendingCapture,
    path: &Path,
    label: Option<String>,
) -> Result<Value, String> {
    let step = region[2].div_ceil(pending.max_width.max(1)).max(1);
    let (width, height, rgb) = captured.rgb8(region, pending.divide, step);
    super::write_png(&path.to_string_lossy(), width, height, &rgb)?;
    Ok(json!({
        "png": path,
        "width": width,
        "height": height,
        "region": region,
        "brightest": captured.brightest(region),
        "divided_by": pending.divide,
        "pane_label": label,
    }))
}

#[cfg(test)]
mod merge_tests {
    use super::merge;
    use serde_json::json;

    #[test]
    fn a_patch_merges_removes_and_replaces_a_changed_kind() {
        let mut settings = json!({
            "color_strategy": "linked",
            "model": { "type": "layered", "knee": 0.75, "sky_floor": 0.1 },
        });
        merge(
            &mut settings,
            &json!({ "model": { "knee": 0.6, "sky_floor": null } }),
        );
        assert_eq!(
            settings,
            json!({ "color_strategy": "linked", "model": { "type": "layered", "knee": 0.6 } })
        );
        merge(&mut settings, &json!({ "model": { "type": "auto-mtf" } }));
        assert_eq!(
            settings,
            json!({ "color_strategy": "linked", "model": { "type": "auto-mtf" } })
        );
    }
}
