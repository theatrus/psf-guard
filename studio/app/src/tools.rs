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
    channel_label, color_group_of, flow_operations, BlendMode, LayerRecord, LayerSource, Row,
    StageKind, StepKind, StepRecord,
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
    (STARS, "Stars"),
];

/// The kind of tool that removes stars.
const STARS: &str = "star_removal";

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
struct Run {
    /// The last line the program printed.
    line: String,
    /// How it ended, once it has.
    done: Option<Result<(), String>>,
    /// For a run that prepares a picture to open: its store and document.
    opens: Option<(PathBuf, PathBuf)>,
    /// For a run of the tools: each input's tools as the run was given
    /// them, which the store holds once it ends.
    chains: Option<serde_json::Value>,
    /// The program's process, while it runs, so a person can stop it.
    process: Option<u32>,
    /// When the run started.
    started: std::time::Instant,
    /// Whether a person stopped the run.
    stopped: bool,
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
    /// The document as it was last saved, to save it again only when it
    /// changed.
    saved: String,
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
            saved: String::new(),
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
        let mut chains = self
            .document
            .channels()
            .into_iter()
            .map(|channel| {
                let chain = self.chain(&channel);
                (channel, serde_json::Value::Array(chain))
            })
            .collect::<serde_json::Map<_, _>>();
        // An input's deconvolution is prepared with the store as well.
        for step in &self.document.stage(StageKind::Linear).steps {
            if let (
                true,
                Some(band),
                StepKind::Operation {
                    operation,
                    settings,
                    ..
                },
            ) = (step.enabled, &step.band, &step.kind)
                && operation == flow_operations::DECONVOLUTION
            {
                chains.insert(format!("{band}:deconvolution"), settings.clone());
            }
        }
        // Tools that run on a color group are prepared with the store too.
        for (band, _) in crate::studio_model::COLOR_GROUPS {
            let chain = self.group_chain(band);
            if !chain.is_empty() {
                chains.insert(format!("{band}:together"), chain.into());
            }
        }
        // Continuum subtraction is prepared with the store as well.
        chains.insert(
            "continuum".to_string(),
            self.continuum_step()
                .map_or(serde_json::Value::Null, |(_, settings)| settings),
        );
        chains.into()
    }

    /// The document's continuum subtraction step and its settings, if it
    /// has one that is on.
    pub(super) fn continuum_step(&self) -> Option<(uuid::Uuid, serde_json::Value)> {
        self.document
            .stage(StageKind::Linear)
            .steps
            .iter()
            .filter(|step| step.enabled)
            .find_map(|step| match &step.kind {
                StepKind::Operation {
                    operation,
                    settings,
                    ..
                } if operation == flow_operations::EMISSION => Some((step.id, settings.clone())),
                _ => None,
            })
    }

    /// Turns continuum subtraction on, with Seiza's automatic fit, or off.
    /// It takes effect when the store is prepared again.
    pub(super) fn set_continuum(&mut self, on: bool) {
        match (on, self.continuum_step()) {
            (true, None) => {
                let step = StepRecord::new(StepKind::Operation {
                    operation: flow_operations::EMISSION.to_string(),
                    accepts: Accepts::Linear,
                    settings: serde_json::json!({}),
                });
                let _ = self
                    .document
                    .insert_step(StageKind::Linear, usize::MAX, step);
            }
            (false, Some((id, _))) => {
                self.document.remove_step(id);
            }
            _ => {}
        }
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

    /// Whether a tool is of one kind, such as star removal.
    fn is_of(&self, key: &str, category: &str) -> bool {
        self.tools
            .tools
            .iter()
            .any(|tool| tool.key == key && tool.category == category)
    }

    /// Chooses the provider of one kind of tool for an input, or none.
    /// When the input's stars are removed with its group, the choice is
    /// the group's.
    fn set_tool(&mut self, channel: &str, category: &str, provider: Option<&str>) {
        let together = (category == STARS)
            .then(|| self.stars_together(channel))
            .flatten();
        let chain = match together {
            Some(band) => self.group_chain(band),
            None => self.chain(channel),
        };
        let mut chain: Vec<serde_json::Value> = chain
            .into_iter()
            .filter(|step| !self.is_of(Self::provider_of(step), category))
            .collect();
        if let Some(provider) = provider {
            chain.push(serde_json::json!({ "provider": provider, "parameters": {} }));
        }
        match together {
            Some(band) => self.set_group_chain(band, chain),
            None => self.set_chain(channel, chain),
        }
    }

    // ---- star removal on a color group ----------------------------------

    /// The color group an input belongs to, when the document has at least
    /// two of the group's inputs: its band and those inputs.
    fn group_of(&self, channel: &str) -> Option<(&'static str, Vec<String>)> {
        let (band, inputs) = color_group_of(channel)?;
        let present: Vec<String> = self
            .document
            .channels()
            .into_iter()
            .filter(|other| inputs.contains(&other.as_str()))
            .collect();
        (present.len() >= 2).then_some((band, present))
    }

    /// The inputs a color group's tools step applies to, as a person reads
    /// them: "Ha, OIII, SII together". `None` for any other step.
    pub(super) fn group_applies_to(&self, step: &StepRecord) -> Option<String> {
        if step.kind.name() != flow_operations::RC_ASTRO_COLOR {
            return None;
        }
        let band = step.band.as_deref()?;
        let (_, inputs) = crate::studio_model::COLOR_GROUPS
            .iter()
            .find(|(group, _)| *group == band)?;
        let present: Vec<String> = self
            .document
            .channels()
            .into_iter()
            .filter(|channel| inputs.contains(&channel.as_str()))
            .map(|channel| channel_label(&channel))
            .collect();
        Some(format!("{} together", present.join(", ")))
    }

    /// The step that holds a color group's tools.
    fn group_step(&self, band: &str) -> Option<&StepRecord> {
        self.document
            .stage(StageKind::Linear)
            .steps
            .iter()
            .find(|step| {
                step.kind.name() == flow_operations::RC_ASTRO_COLOR
                    && step.band.as_deref() == Some(band)
            })
    }

    /// A color group's tools in order.
    fn group_chain(&self, band: &str) -> Vec<serde_json::Value> {
        match self.group_step(band).map(|step| &step.kind) {
            Some(StepKind::Operation { settings, .. }) => {
                settings["steps"].as_array().cloned().unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    /// Writes a color group's tools into the document. A group with no
    /// tool has no step.
    fn set_group_chain(&mut self, band: &str, chain: Vec<serde_json::Value>) {
        let existing = self.group_step(band).map(|step| step.id);
        if chain.is_empty() {
            if let Some(id) = existing {
                self.document.remove_step(id);
            }
            return;
        }
        let settings = serde_json::json!({ "steps": chain });
        match existing {
            Some(id) => {
                self.document.set_step_settings(id, settings);
            }
            None => {
                // A group's tools run after its inputs' own and before the
                // background is fitted.
                let steps = &self.document.stage(StageKind::Linear).steps;
                let before = steps
                    .iter()
                    .position(|step| step.kind.name() == flow_operations::BACKGROUND)
                    .unwrap_or(steps.len());
                let mut step =
                    operation(flow_operations::RC_ASTRO_COLOR, Accepts::Linear).for_band(band);
                if let StepKind::Operation { settings: own, .. } = &mut step.kind {
                    *own = settings;
                }
                let _ = self.document.insert_step(StageKind::Linear, before, step);
            }
        }
    }

    /// The band of the group an input's stars are removed with, when they
    /// are removed with its group rather than on the input alone.
    fn stars_together(&self, channel: &str) -> Option<&'static str> {
        let (band, _) = self.group_of(channel)?;
        self.group_chain(band)
            .iter()
            .any(|step| self.is_of(Self::provider_of(step), STARS))
            .then_some(band)
    }

    /// Moves star removal between each input of a group and the group
    /// together. The tool and its settings move with it.
    fn set_stars_together(&mut self, channel: &str, together: bool) {
        let Some((band, inputs)) = self.group_of(channel) else {
            return;
        };
        let is_stars =
            |this: &Self, step: &serde_json::Value| this.is_of(Self::provider_of(step), STARS);
        if together {
            // This input's star tool, or the first another input has.
            let tool = std::iter::once(channel.to_string())
                .chain(inputs.iter().cloned())
                .find_map(|input| {
                    self.chain(&input)
                        .into_iter()
                        .find(|step| is_stars(self, step))
                });
            let Some(tool) = tool else {
                return;
            };
            for input in &inputs {
                let chain: Vec<_> = self
                    .chain(input)
                    .into_iter()
                    .filter(|step| !is_stars(self, step))
                    .collect();
                self.set_chain(input, chain);
            }
            let mut chain: Vec<_> = self
                .group_chain(band)
                .into_iter()
                .filter(|step| !is_stars(self, step))
                .collect();
            chain.push(tool);
            self.set_group_chain(band, chain);
        } else {
            let (tools, rest): (Vec<_>, Vec<_>) = self
                .group_chain(band)
                .into_iter()
                .partition(|step| is_stars(self, step));
            let Some(tool) = tools.into_iter().next() else {
                return;
            };
            self.set_group_chain(band, rest);
            for input in &inputs {
                let mut chain: Vec<_> = self
                    .chain(input)
                    .into_iter()
                    .filter(|step| !is_stars(self, step))
                    .collect();
                chain.push(tool.clone());
                self.set_chain(input, chain);
            }
        }
    }

    /// The tools an input's Tools block shows: its own, and its group's
    /// star removal when its stars are removed with the group.
    fn shown_chain(&self, channel: &str) -> Vec<serde_json::Value> {
        let mut chain = self.chain(channel);
        if let Some(band) = self.stars_together(channel) {
            chain.extend(
                self.group_chain(band)
                    .into_iter()
                    .filter(|step| self.is_of(Self::provider_of(step), STARS)),
            );
        }
        chain
    }

    /// Sets one setting of one of an input's tools.
    fn set_tool_value(
        &mut self,
        channel: &str,
        provider: &str,
        name: &str,
        value: serde_json::Value,
    ) {
        let together = self
            .stars_together(channel)
            .filter(|_| self.is_of(provider, STARS));
        let mut chain = match together {
            Some(band) => self.group_chain(band),
            None => self.chain(channel),
        };
        for step in &mut chain {
            if Self::provider_of(step) == provider {
                if !step["parameters"].is_object() {
                    step["parameters"] = serde_json::json!({});
                }
                step["parameters"][name] = value.clone();
            }
        }
        match together {
            Some(band) => self.set_group_chain(band, chain),
            None => self.set_chain(channel, chain),
        }
    }

    /// The settings of an input's tools that a person can set, in the
    /// order they are listed: the tool, the setting, and its value now.
    fn tool_settings(&self, channel: &str) -> Vec<(String, Setting, serde_json::Value)> {
        let mut listed = Vec::new();
        for step in self.shown_chain(channel) {
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
                ((min as f64 + (max as f64 - min as f64) * f64::from(position)).round() as i64)
                    .into()
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
        let chains = self.all_chains();
        self.start_run(command, None);
        if let Some(run) = &self.tools.run {
            run.lock().expect("tool run").chains = Some(chains);
        }
    }

    /// Saves the document where the picture keeps it, when it changed
    /// since it was last saved. The file is written beside its place and
    /// renamed over it, so a crash never leaves half a document.
    pub(super) fn save_document(&mut self) {
        let Some(path) = self.tools.document.clone() else {
            return;
        };
        let text = self.document.to_json();
        if text == self.tools.saved {
            return;
        }
        let temporary = path.with_extension("json.saving");
        let saved =
            std::fs::write(&temporary, &text).and_then(|()| std::fs::rename(&temporary, &path));
        match saved {
            Ok(()) => self.tools.saved = text,
            Err(error) => {
                self.tools.status = format!("Not saved: {error}");
            }
        }
    }

    /// Stops saving the document, and says why: the file on disk could not
    /// be read, and is left as it is.
    pub(super) fn keep_document_unsaved(&mut self, why: String) {
        self.tools.document = None;
        self.tools.status = why;
    }

    /// Takes the document as saved on disk: what a picture opened with.
    pub(super) fn document_is_saved(&mut self) {
        self.tools.saved = self.document.to_json();
    }

    /// The open picture's store, if any.
    pub(super) fn store_path(&self) -> Option<&std::path::Path> {
        self.tools.store.as_deref()
    }

    /// Shows a message in the top bar.
    pub(super) fn say(&mut self, message: &str) {
        self.tools.status = message.to_string();
    }

    /// Stops the run in hand. The store and the picture stay as they were
    /// before it; what its tools finished is kept in the cache for the next
    /// run.
    pub(super) fn stop_run(&mut self) {
        let Some(run) = &self.tools.run else {
            return;
        };
        let mut run = run.lock().expect("tool run");
        run.stopped = true;
        if let Some(process) = run.process {
            // The program runs in a process group of its own, with the
            // tools it starts, so the whole group is stopped.
            let _ = Command::new("kill")
                .arg("-TERM")
                .arg(format!("-{process}"))
                .status();
        }
    }

    /// Whether a run of `psf-guard` is in hand.
    pub(super) fn is_running(&self) -> bool {
        self.tools.run.is_some()
    }

    /// How far the run in hand has got: what it is doing, the share of
    /// that done when the program reports one, and the time since it
    /// started. `psf-guard` reports a tool's progress as a line such as
    /// `ha: RC-Astro StarXTerminator (40%)`.
    pub(super) fn run_progress(&self) -> Option<(String, Option<f32>, std::time::Duration)> {
        let run = self.tools.run.as_ref()?;
        let run = run.lock().expect("tool run");
        let line = run.line.trim();
        let share = line
            .strip_suffix("%)")
            .and_then(|head| head.rsplit_once(" ("))
            .and_then(|(what, percent)| Some((what, percent.trim().parse::<f32>().ok()?)));
        let (what, share) = match share {
            Some((what, percent)) => (what.to_string(), Some((percent / 100.0).clamp(0.0, 1.0))),
            None => (line.to_string(), None),
        };
        Some((what, share, run.started.elapsed()))
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
            chains: None,
            process: None,
            stopped: false,
            started: std::time::Instant::now(),
        }));
        self.tools.run = Some(run.clone());
        std::thread::spawn(move || {
            let program = command.get_program().to_string_lossy().into_owned();
            // A group of its own, so a stop reaches the tools it starts.
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.process_group(0);
            }
            let child = command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
            let outcome = match child {
                Err(error) => Err(format!("{program} did not start: {error}")),
                Ok(mut child) => {
                    {
                        let mut state = run.lock().expect("tool run");
                        state.process = Some(child.id());
                        // A stop asked for before the program started.
                        if state.stopped {
                            let _ = Command::new("kill")
                                .arg("-TERM")
                                .arg(format!("-{}", child.id()))
                                .status();
                        }
                    }
                    // Both pipes are read at once: a program that fills one
                    // while the other is read would wait forever.
                    let errors = child.stderr.take().map(|stderr| {
                        std::thread::spawn(move || {
                            BufReader::new(stderr)
                                .lines()
                                .map_while(Result::ok)
                                .filter(|line| !line.trim().is_empty())
                                .last()
                        })
                    });
                    if let Some(stdout) = child.stdout.take() {
                        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                            run.lock().expect("tool run").line = line.trim().to_string();
                        }
                    }
                    let last_error = errors.and_then(|reader| reader.join().ok()).flatten();
                    let waited = child.wait();
                    // The process is gone, so a later stop must not reach a
                    // group that has taken its number.
                    let stopped = {
                        let mut state = run.lock().expect("tool run");
                        state.process = None;
                        state.stopped
                    };
                    match waited {
                        _ if stopped => Err("stopped".to_string()),
                        Ok(status) if status.success() => Ok(()),
                        Ok(_) => Err(last_error.unwrap_or_else(|| "it failed".to_string())),
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
        let (line, done, opens, chains) = {
            let mut run = run.lock().expect("tool run");
            (
                run.line.clone(),
                run.done.take(),
                run.opens.clone(),
                run.chains.clone(),
            )
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
                        // The store holds the tools the run was given, not
                        // any changed while it ran.
                        if chains.is_some() {
                            self.tools.prepared = chains;
                        }
                    }
                }
            }
            Some(Err(error)) => {
                self.tools.run = None;
                // A stopped run leaves the store it was writing behind.
                if error == "stopped" {
                    let store = opens
                        .as_ref()
                        .map(|(store, _)| store.clone())
                        .or_else(|| self.tools.store.clone());
                    if let Some(store) = store {
                        remove_partial_stores(&store);
                    }
                }
                self.tools.status = format!("Not finished: {error}");
            }
        }
    }

    /// Draws the store again from its file, after a run prepared it.
    /// Shows a picture a run prepared: its document, then its store.
    fn show_picture(&mut self, store: PathBuf, document: PathBuf) {
        // The picture on screen keeps its last edits.
        self.save_document();
        let opened = std::fs::read_to_string(&document)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                crate::studio_model::Document::from_json(&text).map_err(|error| error.to_string())
            });
        let opened = match opened {
            Ok(opened) => opened,
            Err(error) => {
                self.tools.status = format!("Not opened: the document could not be read ({error})");
                return;
            }
        };
        // The store is opened first, so a store that cannot be opened
        // leaves the picture on screen as it was.
        let (source, bands) = match crate::store_tiles::source(Some(&store)) {
            Ok(source) => source,
            Err(error) => {
                self.tools.status = format!("Not opened: the store could not be read ({error})");
                return;
            }
        };
        self.document = opened;
        self.document_is_saved();
        self.tools.store = Some(store);
        self.tools.document = Some(document);
        self.tools.status = String::new();
        self.selected = crate::studio_model::Row::Combined;
        self.pins.clear();
        self.open_menu = None;
        self.stroke = None;
        self.drag = None;
        self.alone = false;
        self.mask_shown = false;
        self.show_source(source, bands);
        self.canvas.edit(|canvas| canvas.fit());
    }

    fn reload_store(&mut self) {
        let Some(path) = self.tools.store.clone() else {
            return;
        };
        let (source, bands) = match crate::store_tiles::source(Some(&path)) {
            Ok(opened) => opened,
            Err(error) => {
                self.tools.status = format!("Not opened: the store could not be read ({error})");
                return;
            }
        };
        self.show_source(source, bands);
    }

    /// Draws a store's tiles from now on.
    fn show_source(
        &mut self,
        source: Arc<dyn psf_studio_render::TileSource>,
        bands: crate::store_tiles::Bands,
    ) {
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
            // A line with its continuum subtracted is an input to add to a
            // part; stars go back on top at once.
            if name.ends_with("@line") {
                continue;
            }
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
        let chain = self.shown_chain(channel);
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
                    ("tool", at * 1000 + index),
                    on,
                    cx,
                    move |this| this.set_tool(&channel, category, Some(&key)),
                ));
            }
            block = block
                .child(Self::field(
                    *title,
                    self.menu(format!("tool-{channel}-{category}"), "None", picks, cx),
                ))
                .children(problems.into_iter().filter_map(Self::help));
            // Star removal runs on this input alone or on its color group.
            if *category == STARS
                && !none_chosen
                && let Some((_, inputs)) = self.group_of(channel)
            {
                let together = self.stars_together(channel).is_some();
                let group = inputs
                    .iter()
                    .map(|input| channel_label(input))
                    .collect::<Vec<_>>()
                    .join(", ");
                let picks = vec![
                    {
                        let channel = channel.to_string();
                        Self::pick("This input", "stars-alone", !together, cx, move |this| {
                            this.set_stars_together(&channel, false)
                        })
                    },
                    {
                        let channel = channel.to_string();
                        Self::pick(
                            format!("{group} together"),
                            "stars-together",
                            together,
                            cx,
                            move |this| this.set_stars_together(&channel, true),
                        )
                    },
                ];
                block = block
                    .child(Self::field(
                        "Run on",
                        self.menu(format!("stars-on-{channel}"), "This input", picks, cx),
                    ))
                    .children(Self::help(
                        "On this input alone, after its own tools, or on the group's registered inputs combined into one color image, as PSF Guard's color tools run.",
                    ));
            }
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
                        (now as f64 - min as f64) as f32
                            / (max as f64 - min as f64).max(1.0) as f32,
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

/// Removes the partial files a stopped run left beside a store: a store
/// `picture.pgtiles` is written to `picture.partial-<id>` and renamed when
/// it is complete.
fn remove_partial_stores(store: &std::path::Path) {
    let (Some(folder), Some(name)) = (store.parent(), store.file_stem()) else {
        return;
    };
    let prefix = format!("{}.partial-", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
