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
    // The plate solve of the first stack's reference frame, which color
    // calibration needs to find Gaia's stars on the stacks' shared grid.
    let solve_file = folder.join(SOLVE_FILE);
    match stacks[0].reference_image_id {
        Some(image_id) => match server.frame_solve(image_id).await {
            Ok(Some(solution)) => {
                std::fs::write(&solve_file, serde_json::to_string(&solution)?)
                    .with_context(|| format!("writing {}", solve_file.display()))?;
                println!(
                    "Plate solve of the reference frame: {} stars, {:.2} arcsec RMS",
                    solution.matched_stars, solution.rms_arcsec
                );
            }
            Ok(None) => {
                println!("The reference frame has no plate solve; colors stay uncalibrated")
            }
            Err(error) => println!("No plate solve of the reference frame: {error}"),
        },
        None => println!("The first stack records no reference frame; colors stay uncalibrated"),
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

/// The file in a sources folder that holds the plate solve of the first
/// stack's reference frame.
const SOLVE_FILE: &str = "reference-solve.json";

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
            let mut document =
                document_from_processing("PSF Guard default color", &processing, &roles)?;
            // Studio starts from the layered stretch.
            document.use_layered_stretch();
            document
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
    let mut prepared = RegisteredChannels::prepare(
        &inputs,
        (!tools.chains.is_empty()).then_some(&tools),
        background.as_ref(),
        &seiza_stacking::GpuOptions::cpu(),
    )?;
    let calibration = calibrate_colors(&mut prepared, stacks, folder, &document);
    match &calibration {
        Calibration::NotAsked => {}
        Calibration::Skipped { why } => println!("  Colors not calibrated: {why}"),
        Calibration::Done { gains, stars } => println!(
            "  Colors calibrated against Gaia from {stars} stars: red x{:.3}, blue x{:.3}",
            gains[0], gains[2]
        ),
    }
    let (mut display, star_stretch) = stretch_stars(&mut prepared, &document);
    let calibrated = matches!(calibration, Calibration::Done { .. });
    for name in stretch_whole_images(&mut prepared, &processing.input_stretches, calibrated) {
        println!("  {name}: stretched for display");
        display.push(name);
    }
    if let Some(amount) = star_stretch {
        println!("  Stars of red, green and blue stretched as one layer, amount {amount:.2}");
    }
    // Beside the store, so whatever opens it knows whether red, green and
    // blue are balanced and may share one stretch, and which planes are
    // already stretched for display.
    let record = output.with_extension("prepared.json");
    let prepared_record = serde_json::json!({
        "calibration": calibration,
        "display": display,
        "star_stretch": star_stretch,
    });
    std::fs::write(&record, serde_json::to_string(&prepared_record)?)
        .with_context(|| format!("writing {}", record.display()))?;
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

/// What color calibration did while a store was prepared.
#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Calibration {
    /// The document has no red, green and blue, or its Color calibration
    /// step is off.
    NotAsked,
    Skipped {
        why: String,
    },
    Done {
        /// The multipliers applied to every state of red, green and blue.
        gains: [f32; 3],
        /// The Gaia stars behind the weaker of the two color fits.
        stars: usize,
    },
}

/// Runs the document's Color calibration step: measures the gains of red,
/// green and blue against Gaia on the stacks as registered, and applies
/// them to every state of the three channels.
fn calibrate_colors(
    prepared: &mut RegisteredChannels,
    stacks: &[StackColorSource],
    folder: &Path,
    document: &Document,
) -> Calibration {
    use crate::server::stack_preview::color_calibration::StackColorCalibrationOutcome as Outcome;
    use crate::studio::{flow_operations, StageKind};

    const RGB: [StackColorRole; 3] = [
        StackColorRole::Red,
        StackColorRole::Green,
        StackColorRole::Blue,
    ];
    let step = document
        .stage(StageKind::Linear)
        .steps
        .iter()
        .find(|step| step.kind.name() == flow_operations::COLOR_CALIBRATION);
    if step.is_some_and(|step| !step.enabled) {
        return Calibration::NotAsked;
    }
    let [Some(red), Some(green), Some(blue)] = RGB.map(|role| prepared.stacked(role)) else {
        return Calibration::NotAsked;
    };
    let settings = step
        .and_then(|step| match &step.kind {
            crate::studio::StepKind::Operation { settings, .. } => {
                serde_json::from_value(settings.clone()).ok()
            }
            _ => None,
        })
        .unwrap_or_default();
    let settings: crate::server::stack_preview::color_calibration::StackColorCalibration = settings;
    let neutral_sky = settings.neutralize_background;
    let Some(orientation) = stacks[0].sky_orientation.as_ref() else {
        return Calibration::Skipped {
            why: "the first stack records no sky layout".into(),
        };
    };
    let solve = folder.join(SOLVE_FILE);
    let Some(solution) = std::fs::read_to_string(&solve)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
    else {
        return Calibration::Skipped {
            why: "the sources have no plate solve; fetch them again with studio-sources".into(),
        };
    };
    let service = crate::astrometry::AstrometryContext::new(Default::default());
    let catalog = match service.gaia_photometry_catalog() {
        Ok((catalog, _)) => catalog,
        Err(error) => {
            return Calibration::Skipped {
                why: format!("Gaia colors: {error}"),
            }
        }
    };
    let measured = crate::color_steps::measure_color_gains(
        [&red, &green, &blue],
        &solution,
        orientation,
        &catalog,
        &settings,
    );
    match (measured.outcome, measured.gains) {
        (Outcome::Calibrated(report), Some(gains)) => {
            for (role, gain) in RGB.into_iter().zip(gains) {
                prepared.scale_channel(role, gain);
            }
            if neutral_sky {
                prepared.level_sky(
                    StackColorRole::Green,
                    &[StackColorRole::Red, StackColorRole::Blue],
                );
            }
            Calibration::Done {
                gains,
                stars: report.stars_used,
            }
        }
        (Outcome::Skipped(skip), _) => Calibration::Skipped { why: skip.detail },
        (Outcome::Calibrated(_), None) => Calibration::Skipped {
            why: "no gains were measured".into(),
        },
    }
}

/// Runs the document's Star stretch step: replaces the stars of red, green
/// and blue with one linked display layer. Returns the planes that are
/// stretched for display from then on, and the amount used.
///
/// The step needs each of the three channels as stacked and after a star
/// removal. Without those, or without the step, nothing changes.
fn stretch_stars(
    prepared: &mut RegisteredChannels,
    document: &Document,
) -> (Vec<String>, Option<f64>) {
    use crate::studio::{flow_operations, stars_channel, tools_channel, StageKind, StepKind};

    let Some(step) = document
        .stage(StageKind::Stretch)
        .steps
        .iter()
        .find(|step| step.enabled && step.kind.name() == flow_operations::STAR_STRETCH)
    else {
        return (Vec::new(), None);
    };
    let amount = match &step.kind {
        StepKind::Operation { settings, .. } => settings["amount"].as_f64().unwrap_or(5.0),
        _ => 5.0,
    }
    .clamp(0.0, 8.0);
    let names = ["red", "green", "blue"];
    if names
        .iter()
        .any(|name| !prepared.names.contains(&stars_channel(name)))
    {
        return (Vec::new(), None);
    }
    let [Some(red), Some(green), Some(blue)] = names.map(|name| prepared.plane(name)) else {
        return (Vec::new(), None);
    };
    let [Some(red_less), Some(green_less), Some(blue_less)] =
        names.map(|name| prepared.plane(&tools_channel(name)))
    else {
        return (Vec::new(), None);
    };
    match crate::color_steps::stretch_star_layer(
        [&red, &green, &blue],
        [&red_less, &green_less, &blue_less],
        amount,
    ) {
        Ok(layer) => {
            let mut display = Vec::new();
            for (name, samples) in names.into_iter().zip(layer) {
                let plane = stars_channel(name);
                if prepared.replace_plane(&plane, samples) {
                    display.push(plane);
                }
            }
            (display, Some(amount))
        }
        Err(error) => {
            println!("  The stars were not stretched: {error}");
            (Vec::new(), None)
        }
    }
}

/// Stretches each channel whose stretches include the layered stretch over
/// the whole image, and adds the result as the channel's stretched plane.
/// Returns the planes added.
///
/// The channel is stretched in its last linear state: with its background
/// corrected when the store has that, else after its tools, else as
/// stacked. Calibrated red, green and blue with the same stretches are
/// stretched together as one color image, so their balance is kept.
fn stretch_whole_images(
    prepared: &mut RegisteredChannels,
    stretches: &BTreeMap<
        StackColorRole,
        Vec<crate::server::stack_preview::stretch::StackStretchRequest>,
    >,
    calibrated: bool,
) -> Vec<String> {
    use crate::studio::{background_corrected_channel, stretched_channel, tools_channel};
    use seiza_stretch::StretchModel;

    let linear = |prepared: &RegisteredChannels, name: &str| {
        prepared
            .plane(&background_corrected_channel(name))
            .or_else(|| prepared.plane(&tools_channel(name)))
            .or_else(|| prepared.plane(name))
    };
    let layered: Vec<(StackColorRole, String)> = stretches
        .iter()
        .filter(|(_, stages)| {
            stages
                .iter()
                .any(|stage| matches!(stage.model, StretchModel::Layered(_)))
        })
        .filter_map(|(role, _)| Some((*role, crate::studio_flow::role_name(*role).ok()?)))
        .collect();
    let mut added = Vec::new();
    let rgb = [
        StackColorRole::Red,
        StackColorRole::Green,
        StackColorRole::Blue,
    ];
    let same = |a: StackColorRole, b: StackColorRole| {
        serde_json::to_value(&stretches[&a]).ok() == serde_json::to_value(&stretches[&b]).ok()
    };
    let together = calibrated
        && rgb
            .iter()
            .all(|role| layered.iter().any(|(other, _)| other == role))
        && same(rgb[0], rgb[1])
        && same(rgb[1], rgb[2]);
    if together {
        let names = ["red", "green", "blue"];
        if let [Some(red), Some(green), Some(blue)] = names.map(|name| linear(prepared, name)) {
            let stretched =
                crate::server::stack_preview::color_calibration::interleave(&red, &green, &blue)
                    .and_then(|image| {
                        crate::color_steps::stretch_for_display(&image, &stretches[&rgb[0]])
                    });
            match stretched {
                Ok(image) => {
                    for (channel, name) in names.into_iter().enumerate() {
                        let samples = image
                            .data
                            .iter()
                            .skip(channel)
                            .step_by(3)
                            .copied()
                            .collect();
                        let plane = stretched_channel(name);
                        if prepared.add_plane(&plane, samples) {
                            added.push(plane);
                        }
                    }
                }
                Err(error) => println!("  Red, green and blue were not stretched: {error}"),
            }
        }
    }
    for (role, name) in layered {
        if together && rgb.contains(&role) {
            continue;
        }
        let Some(image) = linear(prepared, &name) else {
            continue;
        };
        match crate::color_steps::stretch_for_display(&image, &stretches[&role]) {
            Ok(image) => {
                let plane = stretched_channel(&name);
                if prepared.add_plane(&plane, image.data) {
                    added.push(plane);
                }
            }
            Err(error) => println!("  {name} was not stretched: {error}"),
        }
    }
    added
}
