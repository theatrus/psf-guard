//! Processing tools in the app: which tools this computer has, choosing
//! them for an input, their settings, and running them.
//!
//! The tools are PSF Guard's: sharpening, noise reduction and star removal,
//! each by a provider, such as an RC-Astro tool or Seiza's noise model. The
//! app does not run a tool itself. It asks the `psf-guard` program which
//! tools there are (`studio-tools`), keeps a person's choice in the
//! document as PSF Guard's own settings, and has `psf-guard` prepare the
//! store again (`studio-prepare`) when a person runs them. The canvas then
//! draws the new store.

use super::panes::operation;
use super::{Options, SliderId, Studio};
use crate::image_steps::Accepts;
use crate::studio_model::{
    channel_label, flow_operations, BlendMode, LayerRecord, LayerSource, Row, StageKind, StepKind,
    StepRecord,
};
use gpui::{div, prelude::*, Context};
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

/// The most tool settings one input shows sliders for.
pub(super) const MAX_TOOL_SETTINGS: usize = 24;

/// What a tool does, in the order a chain runs them: PSF Guard's name for
/// it, and the name a person reads.
const CATEGORIES: [(&str, &str); 3] = [
    ("sharpen", "Sharpen"),
    ("denoise", "Denoise"),
    ("star_removal", "Stars"),
];

/// One tool `psf-guard studio-tools` lists.
#[derive(Clone, Debug, Deserialize)]
struct ToolInfo {
    key: String,
    category: String,
    name: String,
    schema: Option<Schema>,
    /// Why this computer cannot run the tool, when it cannot.
    unavailable: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct Schema {
    #[serde(default)]
    parameters: Vec<Setting>,
}

/// One setting of a tool.
#[derive(Clone, Debug, Deserialize)]
struct Setting {
    name: String,
    /// The flag the tool takes it by. A setting without one is for the
    /// tool's own window only.
    flag: Option<String>,
    label: String,
    kind: SettingKind,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SettingKind {
    Float {
        default: f64,
        min: f64,
        max: f64,
    },
    Bool {
        default: bool,
    },
    Int {
        default: i64,
        min: i64,
        max: i64,
    },
    /// A kind this build does not know. The tool's default is kept.
    #[serde(other)]
    Other,
}

/// The program's list of tools once it has answered, or why it could not.
type Listed = Option<Result<Vec<ToolInfo>, String>>;

/// A run of `psf-guard studio-prepare`, as the thread that reads it
/// reports it.
#[derive(Default)]
struct Run {
    /// The last line the program printed.
    line: String,
    /// How it ended, once it has.
    done: Option<Result<(), String>>,
    /// For a run that prepares a picture to open: its store and document.
    opens: Option<(PathBuf, PathBuf)>,
}

/// The processing tools of the app.
pub(super) struct Tools {
    /// The `psf-guard` program. Without it tools can be neither listed nor
    /// run.
    binary: Option<PathBuf>,
    /// The store the canvas draws, which a run prepares again.
    store: Option<PathBuf>,
    /// Where the document is saved for a run to read.
    document: Option<PathBuf>,
    /// The tools, once the program has listed them, or why it could not.
    listing: Arc<Mutex<Listed>>,
    tools: Vec<ToolInfo>,
    /// The run in hand.
    run: Option<Arc<Mutex<Run>>>,
    /// What the last run said, or why it failed.
    status: String,
    /// Each input's tools as they were when the store was prepared, to say
    /// when the document has moved on.
    prepared: Option<serde_json::Value>,
}

impl Tools {
    /// Asks the program for the tools, without waiting for the answer.
    pub(super) fn new(options: &Options) -> Self {
        let listing = Arc::new(Mutex::new(None));
        // The program given, or the one named by PSF_GUARD, or the one
        // beside this program, or the one on the PATH.
        let binary_path = options
            .psf_guard
            .clone()
            .or_else(|| std::env::var_os("PSF_GUARD").map(PathBuf::from))
            .or_else(|| {
                let beside = std::env::current_exe().ok()?.with_file_name("psf-guard");
                beside.is_file().then_some(beside)
            })
            .unwrap_or_else(|| PathBuf::from("psf-guard"));
        {
            let binary = binary_path.clone();
            let listing = listing.clone();
            std::thread::spawn(move || {
                let listed = Command::new(&binary)
                    .arg("studio-tools")
                    .output()
                    .map_err(|error| format!("{} did not start: {error}", binary.display()))
                    .and_then(|output| {
                        if output.status.success() {
                            serde_json::from_slice(&output.stdout)
                                .map_err(|error| format!("its list of tools was not read: {error}"))
                        } else {
                            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
                        }
                    });
                *listing.lock().expect("tool listing") = Some(listed);
            });
        }
        Self {
            binary: Some(binary_path),
            store: options.open.clone(),
            document: options
                .document
                .clone()
                .or_else(|| Some(options.open.as_ref()?.with_extension("json"))),
            listing,
            tools: Vec::new(),
            run: None,
            status: String::new(),
            prepared: None,
        }
    }
}

impl Studio {
    // ---- the document's tools -------------------------------------------

    /// The step that holds an input's tools.
    fn tools_step(&self, channel: &str) -> Option<&StepRecord> {
        self.document
            .stage(StageKind::Linear)
            .steps
            .iter()
            .find(|step| {
                step.kind.name() == flow_operations::RC_ASTRO_INPUT
                    && step.band.as_deref() == Some(channel)
            })
    }

    /// An input's tools in order: each provider with its settings, as PSF
    /// Guard keeps them.
    fn chain(&self, channel: &str) -> Vec<serde_json::Value> {
        match self.tools_step(channel).map(|step| &step.kind) {
            Some(StepKind::Operation { settings, .. }) => {
                settings["steps"].as_array().cloned().unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    fn provider_of(step: &serde_json::Value) -> &str {
        step["provider"]
            .as_str()
            .or(step["tool"].as_str())
            .unwrap_or_default()
    }

    /// Every input's tools, to tell whether the store was prepared for
    /// them.
    fn all_chains(&self) -> serde_json::Value {
        self.document
            .channels()
            .into_iter()
            .map(|channel| {
                let chain = self.chain(&channel);
                (channel, serde_json::Value::Array(chain))
            })
            .collect::<serde_json::Map<_, _>>()
            .into()
    }

    /// Records that the store the canvas draws was prepared for the
    /// document as it is.
    pub(super) fn tools_are_prepared(&mut self) {
        self.tools.prepared = Some(self.all_chains());
    }

    /// Writes an input's tools into the document. An input with no tool
    /// has no tools step.
    fn set_chain(&mut self, channel: &str, mut chain: Vec<serde_json::Value>) {
        // A chain runs its tools in the order of what they do.
        let place = |step: &serde_json::Value| {
            let provider = Self::provider_of(step);
            let category = self
                .tools
                .tools
                .iter()
                .find(|tool| tool.key == provider)
                .map_or("", |tool| tool.category.as_str());
            CATEGORIES
                .iter()
                .position(|(name, _)| *name == category)
                .unwrap_or(CATEGORIES.len())
        };
        chain.sort_by_key(place);
        let existing = self.tools_step(channel).map(|step| step.id);
        if chain.is_empty() {
            if let Some(id) = existing {
                self.document.remove_step(id);
                if self.selected_step() == Some(id) {
                    self.selected = Row::Input(channel.to_string());
                }
            }
            return;
        }
        let settings = serde_json::json!({ "steps": chain });
        match existing {
            Some(id) => {
                self.document.set_step_settings(id, settings);
            }
            None => {
                // Tools run before the background is fitted.
                let steps = &self.document.stage(StageKind::Linear).steps;
                let before = steps
                    .iter()
                    .position(|step| step.kind.name() == flow_operations::BACKGROUND)
                    .unwrap_or(steps.len());
                let mut step =
                    operation(flow_operations::RC_ASTRO_INPUT, Accepts::Linear).for_band(channel);
                if let StepKind::Operation { settings: own, .. } = &mut step.kind {
                    *own = settings;
                }
                let _ = self.document.insert_step(StageKind::Linear, before, step);
            }
        }
    }

    /// Chooses the provider of one kind of tool for an input, or none.
    fn set_tool(&mut self, channel: &str, category: &str, provider: Option<&str>) {
        let in_category = |key: &str| {
            self.tools
                .tools
                .iter()
                .any(|tool| tool.key == key && tool.category == category)
        };
        let mut chain: Vec<serde_json::Value> = self
            .chain(channel)
            .into_iter()
            .filter(|step| !in_category(Self::provider_of(step)))
            .collect();
        if let Some(provider) = provider {
            chain.push(serde_json::json!({ "provider": provider, "parameters": {} }));
        }
        self.set_chain(channel, chain);
    }

    /// Sets one setting of one of an input's tools.
    fn set_tool_value(
        &mut self,
        channel: &str,
        provider: &str,
        name: &str,
        value: serde_json::Value,
    ) {
        let mut chain = self.chain(channel);
        for step in &mut chain {
            if Self::provider_of(step) == provider {
                if !step["parameters"].is_object() {
                    step["parameters"] = serde_json::json!({});
                }
                step["parameters"][name] = value.clone();
            }
        }
        self.set_chain(channel, chain);
    }

    /// The settings of an input's tools that a person can set, in the
    /// order they are listed: the tool, the setting, and its value now.
    fn tool_settings(&self, channel: &str) -> Vec<(String, Setting, serde_json::Value)> {
        let mut listed = Vec::new();
        for step in self.chain(channel) {
            let provider = Self::provider_of(&step).to_string();
            let Some(schema) = self
                .tools
                .tools
                .iter()
                .find(|tool| tool.key == provider)
                .and_then(|tool| tool.schema.as_ref())
            else {
                continue;
            };
            for setting in &schema.parameters {
                if setting.flag.is_none() || matches!(setting.kind, SettingKind::Other) {
                    continue;
                }
                let value = step["parameters"]
                    .get(&setting.name)
                    .cloned()
                    .unwrap_or_else(|| match setting.kind {
                        SettingKind::Float { default, .. } => default.into(),
                        SettingKind::Int { default, .. } => default.into(),
                        SettingKind::Bool { default } => default.into(),
                        SettingKind::Other => serde_json::Value::Null,
                    });
                listed.push((provider.clone(), setting.clone(), value));
            }
        }
        listed.truncate(MAX_TOOL_SETTINGS);
        listed
    }

    /// The input the selected row belongs to.
    fn selected_input(&self) -> Option<String> {
        match &self.selected {
            Row::Input(channel) | Row::InputStep { channel, .. } => Some(channel.clone()),
            _ => None,
        }
    }

    /// Applies a tool setting's slider, from 0 to 1.
    pub(super) fn tool_slider_moved(&mut self, index: usize, position: f32) {
        let Some(channel) = self.selected_input() else {
            return;
        };
        let Some((provider, setting, _)) = self.tool_settings(&channel).into_iter().nth(index)
        else {
            return;
        };
        let value: serde_json::Value = match setting.kind {
            SettingKind::Float { min, max, .. } => {
                // Two decimals are as fine as the tools' own sliders.
                let value = min + (max - min) * f64::from(position);
                ((value * 100.0).round() / 100.0).into()
            }
            SettingKind::Int { min, max, .. } => {
                (min + ((max - min) as f64 * f64::from(position)).round() as i64).into()
            }
            SettingKind::Bool { .. } | SettingKind::Other => return,
        };
        self.set_tool_value(&channel, &provider, &setting.name, value);
    }

    // ---- running the tools ----------------------------------------------

    /// Whether the tools can be run from here, and why not when they
    /// cannot.
    fn cannot_run(&self) -> Option<&'static str> {
        if self.tools.binary.is_none() {
            Some("The psf-guard program was not found.")
        } else if self.tools.store.is_none() || self.tools.document.is_none() {
            Some("Tools run on a store of channel stacks. This document has none.")
        } else {
            None
        }
    }

    /// Saves the document and has `psf-guard` prepare the store again for
    /// it. The canvas takes the new store when the run ends.
    fn run_tools(&mut self) {
        let (Some(binary), Some(store), Some(document)) = (
            self.tools.binary.clone(),
            self.tools.store.clone(),
            self.tools.document.clone(),
        ) else {
            return;
        };
        if self.tools.run.is_some() {
            return;
        }
        if let Err(error) = std::fs::write(&document, self.document.to_json()) {
            self.tools.status = format!("The document could not be saved: {error}");
            return;
        }
        let mut command = Command::new(&binary);
        command
            .arg("studio-prepare")
            .arg("--sources")
            .arg(store.with_extension("sources"))
            .arg("--apply")
            .arg(&document)
            .arg("--output")
            .arg(&store);
        self.start_run(command, None);
    }

    /// Whether a run of `psf-guard` is in hand.
    pub(super) fn is_running(&self) -> bool {
        self.tools.run.is_some()
    }

    /// What the run in hand, or the last one, said.
    pub(super) fn run_status(&self) -> &str {
        &self.tools.status
    }

    /// The `psf-guard` program.
    pub(super) fn psf_guard(&self) -> Option<PathBuf> {
        self.tools.binary.clone()
    }

    /// Starts a run of `psf-guard` and follows what it prints. With
    /// `opens`, the run prepares a picture, and its store and document are
    /// opened when it ends.
    pub(super) fn start_run(&mut self, mut command: Command, opens: Option<(PathBuf, PathBuf)>) {
        if self.tools.run.is_some() {
            return;
        }
        let run = Arc::new(Mutex::new(Run {
            line: "Starting…".to_string(),
            done: None,
            opens,
        }));
        self.tools.run = Some(run.clone());
        std::thread::spawn(move || {
            let program = command.get_program().to_string_lossy().into_owned();
            let child = command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
            let outcome = match child {
                Err(error) => Err(format!("{program} did not start: {error}")),
                Ok(mut child) => {
                    if let Some(stdout) = child.stdout.take() {
                        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                            run.lock().expect("tool run").line = line.trim().to_string();
                        }
                    }
                    match child.wait_with_output() {
                        Ok(output) if output.status.success() => Ok(()),
                        Ok(output) => Err(String::from_utf8_lossy(&output.stderr)
                            .lines()
                            .last()
                            .unwrap_or("it failed")
                            .to_string()),
                        Err(error) => Err(error.to_string()),
                    }
                }
            };
            run.lock().expect("tool run").done = Some(outcome);
        });
    }

    pub(super) fn follow_tools(&mut self) {
        if let Some(listed) = self.tools.listing.lock().expect("tool listing").take() {
            match listed {
                Ok(tools) => self.tools.tools = tools,
                Err(error) => self.tools.status = format!("The tools were not listed: {error}"),
            }
        }
        let Some(run) = self.tools.run.clone() else {
            return;
        };
        let (line, done, opens) = {
            let mut run = run.lock().expect("tool run");
            (run.line.clone(), run.done.take(), run.opens.clone())
        };
        self.tools.status = line;
        match done {
            None => {}
            Some(Ok(())) => {
                self.tools.run = None;
                match opens {
                    Some((store, document)) => self.show_picture(store, document),
                    None => {
                        self.tools.status =
                            "The tools ran. The canvas shows their result.".to_string();
                        self.reload_store();
                    }
                }
            }
            Some(Err(error)) => {
                self.tools.run = None;
                self.tools.status = format!("Not finished: {error}");
            }
        }
    }

    /// Draws the store again from its file, after a run prepared it.
    /// Shows a picture a run prepared: its document, then its store.
    fn show_picture(&mut self, store: PathBuf, document: PathBuf) {
        match std::fs::read_to_string(&document)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                crate::studio_model::Document::from_json(&text).map_err(|error| error.to_string())
            }) {
            Ok(opened) => self.document = opened,
            Err(error) => {
                self.tools.status = format!("The document could not be opened: {error}");
                return;
            }
        }
        self.tools.store = Some(store);
        self.tools.document = Some(document);
        self.tools.status = String::new();
        self.selected = crate::studio_model::Row::Combined;
        self.reload_store();
        self.canvas.edit(|canvas| canvas.fit());
    }

    fn reload_store(&mut self) {
        let Some(path) = self.tools.store.clone() else {
            return;
        };
        let (source, bands) = match crate::store_tiles::source(Some(&path)) {
            Ok(opened) => opened,
            Err(error) => {
                self.tools.status = format!("The new store could not be opened: {error}");
                return;
            }
        };
        let pyramid = source.pyramid();
        self.band_info = Arc::new(bands);
        self.analysed.clear();
        self.levels = pyramid.levels;
        {
            let mut shared = self.canvas.0 .0.lock().expect("canvas state");
            shared.canvas.document_size = (pyramid.width, pyramid.height);
            shared.canvas.band_count = self.band_info.len();
            shared.new_source = Some(source);
        }
        self.take_in_new_inputs();
        self.tools_are_prepared();
        self.refresh_canvas();
    }

    /// Gives the document an input and a layer for each input the store
    /// has that the document lacks: the stars a star removal took out.
    /// The stars go on top with the screen blend, which puts stars back
    /// onto a starless picture, and are stretched as their input is.
    pub(super) fn take_in_new_inputs(&mut self) {
        let known = self.document.channels();
        let new: Vec<String> = self
            .band_info
            .names
            .iter()
            .filter(|name| !known.contains(name))
            .cloned()
            .collect();
        for name in new {
            let source = operation(flow_operations::CHANNEL_STACK, Accepts::Linear).for_band(&name);
            let _ = self
                .document
                .insert_step(StageKind::Sources, usize::MAX, source);
            let mut layer = LayerRecord::new(
                &channel_label(&name),
                LayerSource::Band { band: name.clone() },
            );
            layer.blend = BlendMode::Screen;
            layer.color = Some([1.0; 3]);
            let _ = self.document.insert_layer(usize::MAX, layer);
        }
    }

    // ---- the controls ---------------------------------------------------

    /// An input's tools: one choice per kind of tool, each chosen tool's
    /// settings, and the button that runs them.
    pub(super) fn tools_block(&self, channel: &str, cx: &mut Context<Self>) -> gpui::Div {
        let label = channel_label(channel);
        let chain = self.chain(channel);
        let chosen: Vec<&str> = chain.iter().map(Self::provider_of).collect();
        let mut block = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Self::heading("Tools"));
        let _ = label;
        if let Some(why) = self.cannot_run() {
            block = block.child(Self::note(why));
        } else if self.tools.tools.is_empty() {
            block = block.child(Self::note("Looking for the tools this computer has…"));
        }
        for (at, (category, title)) in CATEGORIES.iter().enumerate() {
            let offered: Vec<&ToolInfo> = self
                .tools
                .tools
                .iter()
                .filter(|tool| tool.category == *category)
                .collect();
            if offered.is_empty() {
                continue;
            }
            let none_chosen = !offered
                .iter()
                .any(|tool| chosen.contains(&tool.key.as_str()));
            let mut picks = vec![{
                let channel = channel.to_string();
                Self::pick("None", ("tool-none", at), none_chosen, cx, move |this| {
                    this.set_tool(&channel, category, None)
                })
            }];
            let mut problems = Vec::new();
            for (index, tool) in offered.iter().enumerate() {
                let on = chosen.contains(&tool.key.as_str());
                if let Some(why) = &tool.unavailable {
                    problems.push(format!("{}: {why}", tool.name));
                    continue;
                }
                let (channel, key) = (channel.to_string(), tool.key.clone());
                picks.push(Self::pick(
                    tool.name.clone(),
                    ("tool", at * 8 + index),
                    on,
                    cx,
                    move |this| this.set_tool(&channel, category, Some(&key)),
                ));
            }
            block = block
                .child(Self::field(
                    *title,
                    self.menu(format!("tool-{category}"), "None", picks, cx),
                ))
                .children(problems.into_iter().filter_map(Self::help));
        }
        // The chosen tools' settings.
        for (index, (provider, setting, value)) in
            self.tool_settings(channel).into_iter().enumerate()
        {
            let tool = self
                .tools
                .tools
                .iter()
                .find(|tool| tool.key == provider)
                .map_or(provider.clone(), |tool| tool.name.clone());
            // The tool's name goes before its first setting.
            let first = index == 0
                || self
                    .tool_settings(channel)
                    .get(index - 1)
                    .is_none_or(|(before, ..)| *before != provider);
            if first {
                block = block.child(Self::heading(tool));
            }
            block = match setting.kind {
                SettingKind::Float { min, max, .. } => {
                    let now = value.as_f64().unwrap_or(min);
                    block.child(self.labelled_slider(
                        format!("{} {now:.2}", setting.label),
                        SliderId::ToolParam(index),
                        ((now - min) / (max - min).max(f64::EPSILON)) as f32,
                        cx,
                    ))
                }
                SettingKind::Int { min, max, .. } => {
                    let now = value.as_i64().unwrap_or(min);
                    block.child(self.labelled_slider(
                        format!("{} {now}", setting.label),
                        SliderId::ToolParam(index),
                        (now - min) as f32 / ((max - min).max(1)) as f32,
                        cx,
                    ))
                }
                SettingKind::Bool { .. } => {
                    let now = value.as_bool().unwrap_or(false);
                    let (channel, name) = (channel.to_string(), setting.name.clone());
                    block.child(Self::act(
                        format!("{}: {}", setting.label, if now { "on" } else { "off" }),
                        ("tool-switch", index),
                        now,
                        cx,
                        move |this| this.set_tool_value(&channel, &provider, &name, (!now).into()),
                    ))
                }
                SettingKind::Other => block,
            };
        }
        // Running them.
        let running = self.tools.run.is_some();
        let changed = self
            .tools
            .prepared
            .as_ref()
            .is_some_and(|prepared| *prepared != self.all_chains());
        if self.cannot_run().is_none() {
            block = block
                .child(
                    Self::act(
                        if running { "Running…" } else { "Run tools" },
                        "tools-run",
                        running,
                        cx,
                        |this| this.run_tools(),
                    ),
                )
                .children(Self::help(
                    "Runs every input's tools and prepares the store again. A tool that ran with these settings before is not run again.",
                ));
            if changed && !running {
                block = block.child(div().text_xs().child("Changed. Run to update."));
            }
        }
        if !self.tools.status.is_empty() {
            block = block.child(div().text_xs().child(self.tools.status.clone()));
        }
        block
    }
}
