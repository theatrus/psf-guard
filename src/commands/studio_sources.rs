use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::color_steps::ToolChain;
use crate::processing_setups::DEFAULT_COLOR_SETTINGS;
use crate::server::stack_preview::color::{StackColorRole, StackColorSource};
use crate::server::workers::Workers;
use crate::studio::{Document, RegionReader, StoreOptions, TileStore};
use crate::studio_flow::{document_from_processing, processing_from_document};
use crate::studio_sources::{ChannelTools, RegisteredChannels, Server};

pub struct StudioSourcesOptions {
    pub server: String,
    pub database: String,
    pub token: Option<String>,
    pub project_id: i32,
    pub target_id: i32,
    /// The channels to take, by PSF Guard's role names, reference first.
    /// Empty takes every channel the target has.
    pub channels: Vec<String>,
    /// Where the document goes. It starts as PSF Guard's default color flow.
    pub document: Option<PathBuf>,
    /// A document to prepare the store for, in place of the default flow.
    pub apply: Option<PathBuf>,
    pub output: PathBuf,
}

/// Fetches a target's channel stacks from a PSF Guard server, registers
/// them onto one grid and writes them as a PSF-Studio tile store, with a
/// document that starts as PSF Guard's default color flow.
pub async fn studio_sources(options: StudioSourcesOptions) -> Result<()> {
    let server = Server::new(&options.server, &options.database, options.token);
    let mut stacks = server
        .channel_stacks(options.project_id, options.target_id)
        .await?;
    if !options.channels.is_empty() {
        let mut chosen = Vec::new();
        for name in &options.channels {
            let role: StackColorRole =
                serde_json::from_value(serde_json::Value::String(name.clone()))
                    .with_context(|| format!("{name} is not a channel"))?;
            let Some(stack) = stacks.iter().find(|stack| stack.role == role) else {
                bail!("target {} has no {name} channel stack", options.target_id);
            };
            chosen.push(stack.clone());
        }
        stacks = chosen;
    }
    if stacks.is_empty() {
        bail!("target {} has no channel stacks", options.target_id);
    }

    let folder = options.output.with_extension("sources");
    std::fs::create_dir_all(&folder).with_context(|| format!("creating {}", folder.display()))?;
    for stack in &stacks {
        let path = folder.join(stack_file(stack));
        let bytes = server.download_stack(stack, &path).await?;
        println!(
            "Downloaded the {} stack ({} frames, {:.1} MB)",
            stack.filter_name,
            stack.accepted_frames,
            bytes as f64 / (1 << 20) as f64
        );
    }
    // What was downloaded, so the store can be prepared again without the
    // server: for other tools, or other settings.
    let listing = folder.join(SOURCES_FILE);
    std::fs::write(&listing, serde_json::to_string_pretty(&stacks)?)
        .with_context(|| format!("writing {}", listing.display()))?;
    prepare_store(
        &stacks,
        &folder,
        options.apply.as_deref(),
        options.document.as_deref(),
        &options.output,
    )
}

/// The file in a sources folder that lists the downloaded channel stacks.
const SOURCES_FILE: &str = "sources.json";

/// The name of a downloaded channel stack in its sources folder.
fn stack_file(stack: &StackColorSource) -> String {
    format!("{}.fits", stack.job_id)
}

pub struct StudioPrepareOptions {
    /// The folder `studio-sources` downloaded the channel stacks to.
    pub sources: PathBuf,
    /// A document to prepare the store for, in place of the default flow.
    pub apply: Option<PathBuf>,
    /// Where a new default document goes.
    pub document: Option<PathBuf>,
    pub output: PathBuf,
}

/// Prepares a PSF-Studio tile store from channel stacks that were
/// downloaded before, without a server: registers them, runs the document's
/// processing tools, fits the backgrounds and writes the store.
pub fn studio_prepare(options: StudioPrepareOptions) -> Result<()> {
    let listing = options.sources.join(SOURCES_FILE);
    let text = std::fs::read_to_string(&listing).with_context(|| {
        format!(
            "reading {}; fetch the stacks with studio-sources first",
            listing.display()
        )
    })?;
    let stacks: Vec<StackColorSource> =
        serde_json::from_str(&text).with_context(|| format!("reading {}", listing.display()))?;
    prepare_store(
        &stacks,
        &options.sources,
        options.apply.as_deref(),
        options.document.as_deref(),
        &options.output,
    )
}

/// Registers downloaded channel stacks, runs the document's processing
/// tools on them, fits the backgrounds and writes the store.
fn prepare_store(
    stacks: &[StackColorSource],
    folder: &Path,
    apply: Option<&Path>,
    document_out: Option<&Path>,
    output: &Path,
) -> Result<()> {
    if stacks.is_empty() {
        bail!("there are no channel stacks to prepare");
    }
    let files: Vec<(StackColorRole, PathBuf)> = stacks
        .iter()
        .map(|stack| (stack.role, folder.join(stack_file(stack))))
        .collect();

    // The document decides what runs: a new one is PSF Guard's default flow.
    let roles: Vec<StackColorRole> = stacks.iter().map(|stack| stack.role).collect();
    let document = match apply {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            Document::from_json(&text)?
        }
        None => {
            let processing = serde_json::from_str(DEFAULT_COLOR_SETTINGS)?;
            document_from_processing("PSF Guard default color", &processing, &roles)?
        }
    };
    let processing = processing_from_document(&document)?;
    let background = processing.background_extraction;

    // Each channel's processing tools, resolved against the tools this
    // machine has. A tool's result is kept beside the downloaded stacks.
    let workers = Workers::default();
    let mut chains = BTreeMap::new();
    for stack in stacks {
        let Some(config) = processing.input_rc_astro.get(&stack.role) else {
            continue;
        };
        if config.steps.is_empty() {
            continue;
        }
        let chain = ToolChain::resolve(
            &workers,
            config,
            "studio",
            &format!("mono:{}:{}", stack.job_id, stack.group_index),
            &stack.artifact_revision.to_string(),
        )
        .map_err(|error| anyhow!("the tools of {}: {error}", stack.filter_name))?;
        chains.insert(stack.role, chain);
    }
    // One line per tool and per tenth of its channel's chain.
    let reported = std::cell::RefCell::new((String::new(), u32::MAX));
    let report = |channel: &str, tool: &str, fraction: f32| {
        let tenth = (fraction.clamp(0.0, 1.0) * 10.0) as u32;
        let line = format!("{channel}: {tool}");
        let mut last = reported.borrow_mut();
        if *last != (line.clone(), tenth) {
            println!("  {line} ({}%)", tenth * 10);
            *last = (line, tenth);
        }
    };
    let tools = ChannelTools {
        workers: &workers,
        cache: folder,
        chains,
        progress: &report,
    };

    let started = Instant::now();
    let inputs: Vec<_> = files
        .iter()
        .map(|(role, path)| (*role, path.as_path()))
        .collect();
    let prepared = RegisteredChannels::prepare(
        &inputs,
        (!tools.chains.is_empty()).then_some(&tools),
        background.as_ref(),
        &seiza_stacking::GpuOptions::cpu(),
    )?;
    for (role, chain) in &prepared.tools {
        let ran: Vec<&str> = chain
            .steps
            .iter()
            .map(|step| step.provider.as_str())
            .collect();
        println!("  {role:?} tools: {}", ran.join(", "));
    }
    for placement in &prepared.placements {
        match &placement.registration {
            None => println!("  {:?} is the reference", placement.role),
            Some(registration) => println!(
                "  {:?} registered: {:.3} px RMS, {:.1} px drift, {} stars",
                placement.role,
                registration.rms_error_pixels,
                registration.drift_pixels,
                registration.matched_stars
            ),
        }
    }
    for (role, fit) in &prepared.backgrounds {
        let model = match &fit.model {
            seiza_background::FittedModel::Polynomial { degree, .. } => {
                format!("polynomial of degree {degree}")
            }
            seiza_background::FittedModel::RadialBasis { .. } => "radial basis".to_string(),
            _ => "another model".to_string(),
        };
        println!("  {role:?} background: {model}");
    }
    let backgrounds = folder.join("backgrounds.json");
    std::fs::write(&backgrounds, serde_json::to_string(&prepared.backgrounds)?)
        .with_context(|| format!("writing {}", backgrounds.display()))?;
    println!("Prepared in {:.1} s", started.elapsed().as_secs_f64());
    let registered = prepared;

    // The key changes when any channel's stack is rebuilt.
    let key = stacks
        .iter()
        .map(|stack| {
            format!(
                "{}:{}:{}",
                stack.job_id, stack.group_index, stack.artifact_revision
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    // It also changes with the tools that ran and their settings.
    let tool_key = tools
        .chains
        .values()
        .filter_map(|chain| chain.ids.last().map(String::as_str))
        .collect::<Vec<_>>()
        .join("|");
    let key = if tool_key.is_empty() {
        key
    } else {
        format!("{key}|tools:{tool_key}")
    };
    let started = Instant::now();
    let store = TileStore::create(
        output,
        &registered,
        &StoreOptions {
            channels: registered.names.clone(),
            ..StoreOptions::new(&format!("channels:{key}"))
        },
    )?;
    let shape = registered.shape();
    println!(
        "Wrote {}: {} x {} pixels, {} channel(s), {} levels, in {:.1} s",
        output.display(),
        shape.width,
        shape.height,
        shape.channels,
        store.levels(),
        started.elapsed().as_secs_f64()
    );

    if let Some(path) = document_out {
        std::fs::write(path, document.to_json())
            .with_context(|| format!("writing {}", path.display()))?;
        println!("Wrote the document to {}", path.display());
    }
    Ok(())
}

/// Prints the processing tools this computer can run, with each tool's
/// settings, as JSON: what PSF-Studio offers a person to choose from.
pub fn studio_tools() -> Result<()> {
    let workers = Workers::default();
    let tools: Vec<serde_json::Value> = crate::remote_work::steps::PROVIDERS
        .iter()
        .map(|provider| {
            let (schema, unavailable) = match workers.step_schema(provider.key) {
                Ok(schema) => (Some(schema), None),
                Err(why) => (None, Some(why)),
            };
            serde_json::json!({
                "key": provider.key,
                "category": provider.category,
                "name": provider.name,
                "schema": schema,
                "unavailable": unavailable,
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&tools)?);
    Ok(())
}
