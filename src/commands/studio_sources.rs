use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use crate::color_steps::ToolChain;
use crate::processing_setups::DEFAULT_COLOR_SETTINGS;
use crate::server::stack_preview::color::StackColorRole;
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
    let mut files = Vec::new();
    for stack in &stacks {
        let path = folder.join(format!("{}.fits", stack.job_id));
        let bytes = server.download_stack(stack, &path).await?;
        println!(
            "Downloaded the {} stack ({} frames, {:.1} MB)",
            stack.filter_name,
            stack.accepted_frames,
            bytes as f64 / (1 << 20) as f64
        );
        files.push((stack.role, path));
    }

    // The document decides what runs: a new one is PSF Guard's default flow.
    let roles: Vec<StackColorRole> = stacks.iter().map(|stack| stack.role).collect();
    let document = match &options.apply {
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
    for stack in &stacks {
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
        cache: &folder,
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
        &options.output,
        &registered,
        &StoreOptions {
            channels: registered.names.clone(),
            ..StoreOptions::new(&format!("channels:{key}"))
        },
    )?;
    let shape = registered.shape();
    println!(
        "Wrote {}: {} x {} pixels, {} channel(s), {} levels, in {:.1} s",
        options.output.display(),
        shape.width,
        shape.height,
        shape.channels,
        store.levels(),
        started.elapsed().as_secs_f64()
    );

    if let Some(path) = &options.document {
        std::fs::write(path, document.to_json())
            .with_context(|| format!("writing {}", path.display()))?;
        println!("Wrote the document to {}", path.display());
    }
    Ok(())
}
