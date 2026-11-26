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
    let (title, mut stacks) = server
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
        // Where the stack's frames clipped, for continuum subtraction.
        let clipped = server
            .download_clipped(stack, &folder.join(clipped_file(stack)))
            .await
            .unwrap_or_else(|error| {
                println!(
                    "  No clipping map of the {} stack: {error}",
                    stack.filter_name
                );
                None
            });
        if clipped.is_none() {
            println!("  The {} stack has no clipping map", stack.filter_name);
        }
        println!(
            "Downloaded the {} stack ({} frames, {:.1} MB)",
            stack.filter_name,
            stack.accepted_frames,
            bytes as f64 / (1 << 20) as f64
        );
    }
    // The plate solve of the first stack's reference frame, which color
    // calibration needs to find Gaia's stars on the stacks' shared grid.
    // A solve kept from an earlier run is not this stack's.
    let _ = std::fs::remove_file(folder.join(SOLVE_FILE));
    match stacks[0].reference_image_id {
        Some(image_id) => match server.frame_solve(image_id).await {
            Ok(Some(solution)) => {
                write_solve(&folder, &stacks[0], &solution)?;
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
    write_atomically(&listing, serde_json::to_string_pretty(&stacks)?)?;
    prepare_store(
        &title,
        &stacks,
        &folder,
        options.apply.as_deref(),
        options.document.as_deref(),
        &options.output,
    )
}

/// Writes a file through a temporary file beside it, renamed over it, so a
/// crash or a kill never leaves half a file.
fn write_atomically(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".writing");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, contents)
        .with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("writing {}", path.display()))
}

/// The names documents had when they were named after the setup they
/// started from.
const SETUP_NAMES: [&str; 2] = ["PSF Guard default color", "Default"];

/// A picture's name from its sources folder: the folder's name without
/// `.sources`.
fn title_of_sources(folder: &Path) -> String {
    folder.file_stem().map_or_else(
        || "Picture".to_string(),
        |stem| stem.to_string_lossy().into_owned(),
    )
}

/// The file in a sources folder that holds the plate solve of the first
/// stack's reference frame, with the stack it solves.
const SOLVE_FILE: &str = "reference-solve.json";

/// The identity of a stack as a solve records it: a solve is used only for
/// the stack it was made for.
fn stack_identity(stack: &StackColorSource) -> String {
    format!(
        "{}:{}:{}",
        stack.job_id, stack.group_index, stack.artifact_revision
    )
}

/// Keeps the plate solve of `stack`, the first stack, for color
/// calibration.
fn write_solve(
    folder: &Path,
    stack: &StackColorSource,
    solution: &crate::astrometry::AstrometrySolutionResponse,
) -> Result<()> {
    let record = serde_json::json!({ "stack": stack_identity(stack), "solution": solution });
    write_atomically(&folder.join(SOLVE_FILE), serde_json::to_string(&record)?)
}

/// The plate solve kept for `stack`, if one was made for it.
fn read_solve(
    folder: &Path,
    stack: &StackColorSource,
) -> Option<crate::astrometry::AstrometrySolutionResponse> {
    let text = std::fs::read_to_string(folder.join(SOLVE_FILE)).ok()?;
    let record: serde_json::Value = serde_json::from_str(&text).ok()?;
    if record["stack"].as_str() != Some(stack_identity(stack).as_str()) {
        return None;
    }
    serde_json::from_value(record["solution"].clone()).ok()
}

/// The file in a sources folder that lists the downloaded channel stacks.
const SOURCES_FILE: &str = "sources.json";

/// The file in a sources folder that says where each stack is, for stacks
/// that were not downloaded but are read where a person keeps them.
const PATHS_FILE: &str = "paths.json";

/// The name of a downloaded channel stack in its sources folder. One stack
/// job can hold several channels, one per group.
fn stack_file(stack: &StackColorSource) -> String {
    format!("{}-{}.fits", stack.job_id, stack.group_index)
}

/// The name of a stack's clipping map in its sources folder.
fn clipped_file(stack: &StackColorSource) -> String {
    format!("{}-{}-clipped.fits", stack.job_id, stack.group_index)
}

/// Where one channel's stack is: the file a person named for it, or the
/// one downloaded to the sources folder.
fn stack_path(folder: &Path, stack: &StackColorSource) -> PathBuf {
    std::fs::read_to_string(folder.join(PATHS_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<BTreeMap<String, PathBuf>>(&text).ok())
        .and_then(|paths| paths.get(&stack.job_id).cloned())
        .unwrap_or_else(|| {
            let path = folder.join(stack_file(stack));
            // A folder downloaded before the group was in the name.
            let before = folder.join(format!("{}.fits", stack.job_id));
            if !path.exists() && before.exists() {
                before
            } else {
                path
            }
        })
}

pub struct StudioFilesOptions {
    /// One `channel=file` per channel, reference first.
    pub channels: Vec<String>,
    /// A document to prepare the store for, in place of the default flow.
    pub apply: Option<PathBuf>,
    /// Where a new default document goes.
    pub document: Option<PathBuf>,
    /// Where the first file points, in degrees, and its pixel scale in
    /// arcseconds per pixel: the hint its plate solve starts from.
    pub pointing: Option<(f64, f64, f64)>,
    pub output: PathBuf,
}

/// Prepares a PSF-Studio tile store from channel stacks a person has as
/// files: FITS or XISF, one per channel. No server takes part. The files
/// are read where they are; the sources folder beside the store holds only
/// a list of them, the plate solve and the tools' results.
pub fn studio_files(options: StudioFilesOptions) -> Result<()> {
    let mut stacks = Vec::new();
    let mut paths = BTreeMap::new();
    for entry in &options.channels {
        let Some((name, file)) = entry.split_once('=') else {
            bail!("{entry} is not CHANNEL=FILE, such as ha=Ha.xisf");
        };
        let role: StackColorRole = serde_json::from_value(serde_json::Value::String(name.into()))
            .with_context(|| format!("{name} is not a channel"))?;
        if stacks
            .iter()
            .any(|stack: &StackColorSource| stack.role == role)
        {
            bail!("{name} is given twice");
        }
        let path = std::fs::canonicalize(file).with_context(|| format!("finding {file}"))?;
        if !crate::image_io::is_image_path(&path) {
            bail!("{} is not a FITS or XISF file", path.display());
        }
        let stamp = std::fs::metadata(&path).with_context(|| format!("reading {file}"))?;
        let modified = stamp
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_nanos());
        // The file's identity: the tools' results are kept under it, and a
        // changed file gets a new one.
        let identity = {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(format!("{}|{}|{modified}", path.display(), stamp.len()));
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        paths.insert(identity.clone(), path.clone());
        stacks.push(StackColorSource {
            role,
            filter_name: name.to_string(),
            label: path.file_name().map_or_else(
                || name.to_string(),
                |file| file.to_string_lossy().into_owned(),
            ),
            exposure_group: None,
            job_id: identity,
            group_index: 0,
            artifact_revision: format!("{}-{modified}", stamp.len()),
            accepted_frames: 0,
            wbpp: false,
            total_exposure_seconds: None,
            reference_image_id: None,
            sky_orientation: None,
            registration_transform: None,
        });
    }
    let folder = options.output.with_extension("sources");
    std::fs::create_dir_all(&folder).with_context(|| format!("creating {}", folder.display()))?;

    // The first file is the grid every channel is registered to. Its own
    // pixels are solved, so color calibration can find Gaia's stars on it.
    let first = paths[&stacks[0].job_id].clone();
    let frame = crate::image_io::open_linear_frame(&first)
        .with_context(|| format!("reading {}", first.display()))?;
    // The picture is named after what the first file shows, or the file.
    let title = crate::image_io::header_text(&frame.headers, "OBJECT")
        .map(|object| object.trim().to_string())
        .filter(|object| !object.is_empty())
        .unwrap_or_else(|| {
            first.file_stem().map_or_else(
                || "Picture".to_string(),
                |stem| stem.to_string_lossy().into_owned(),
            )
        });
    stacks[0].sky_orientation = Some(crate::server::stack_preview::StackSkyOrientation {
        convention: crate::server::stack_preview::SOURCE_ORIENTATION_NAME.into(),
        version: seiza_stacking::SKY_ORIENTATION_VERSION,
        source: "file".into(),
        output_width: frame.image.width,
        output_height: frame.image.height,
        source_to_output: seiza_stacking::AffineTransform::IDENTITY,
    });
    drop(frame);
    let solver = crate::astrometry::AstrometryContext::new(Default::default());
    let hint = options
        .pointing
        .map(|(ra, dec, scale)| crate::astrometry::PixelSolveHint {
            center: (ra, dec),
            scale_arcsec_per_pixel: scale,
            source: "command_line",
        });
    let _ = std::fs::remove_file(folder.join(SOLVE_FILE));
    match solver.solve_stack(0, &first, None, hint) {
        Ok(analysis) => match analysis.solution {
            Some(solution) => {
                write_solve(&folder, &stacks[0], &solution)?;
                println!(
                    "Plate solve of {}: {} stars, {:.2} arcsec RMS",
                    stacks[0].label, solution.matched_stars, solution.rms_arcsec
                );
            }
            None => println!(
                "{} could not be plate solved{}; colors stay uncalibrated. \
                 Give --pointing when the file records no pointing",
                stacks[0].label,
                analysis
                    .error
                    .map_or_else(String::new, |error| format!(" ({error})"))
            ),
        },
        Err(error) => println!("No plate solve of {}: {error}", stacks[0].label),
    }

    write_atomically(
        &folder.join(PATHS_FILE),
        serde_json::to_string_pretty(&paths)?,
    )?;
    let listing = folder.join(SOURCES_FILE);
    write_atomically(&listing, serde_json::to_string_pretty(&stacks)?)?;
    prepare_store(
        &title,
        &stacks,
        &folder,
        options.apply.as_deref(),
        options.document.as_deref(),
        &options.output,
    )
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
        &title_of_sources(&options.sources),
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
    title: &str,
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
        .map(|stack| (stack.role, stack_path(folder, stack)))
        .collect();

    // The document decides what runs: a new one is PSF Guard's default flow.
    let roles: Vec<StackColorRole> = stacks.iter().map(|stack| stack.role).collect();
    let document = match apply {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            let mut document = Document::from_json(&text)?;
            // A document named after the setup it started from, as they
            // were before, takes its picture's name, and is saved with it.
            if SETUP_NAMES.contains(&document.name.as_str()) && title != document.name {
                document.name = title.to_string();
                write_atomically(path, document.to_json())?;
            }
            document
        }
        None => {
            let processing = serde_json::from_str(DEFAULT_COLOR_SETTINGS)?;
            let mut document = document_from_processing(title, &processing, &roles)?;
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
    let mut deconvolutions = BTreeMap::new();
    for stack in stacks {
        let source = format!("mono:{}:{}", stack.job_id, stack.group_index);
        let revision = stack.artifact_revision.to_string();
        let deconvolution = processing
            .input_deconvolutions
            .get(&stack.role)
            .map(|config| {
                crate::color_steps::ChannelDeconvolution::resolve(
                    *config, "studio", &source, &revision,
                )
            })
            .transpose()
            .map_err(|error| anyhow!("the deconvolution of {}: {error}", stack.filter_name))?;
        if let Some(config) = processing
            .input_rc_astro
            .get(&stack.role)
            .filter(|config| !config.steps.is_empty())
        {
            let chain = ToolChain::resolve(
                &workers,
                config,
                "studio",
                &source,
                &revision,
                deconvolution
                    .as_ref()
                    .map(|deconvolution| deconvolution.id.as_str()),
            )
            .map_err(|error| anyhow!("the tools of {}: {error}", stack.filter_name))?;
            chains.insert(stack.role, chain);
        }
        if let Some(deconvolution) = deconvolution {
            deconvolutions.insert(stack.role, deconvolution);
        }
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
        deconvolutions,
        progress: &report,
    };

    let started = Instant::now();
    let inputs: Vec<_> = files
        .iter()
        .map(|(role, path)| (*role, path.as_path()))
        .collect();
    // Continuum subtraction, when the document asks for it, with each
    // stack's clipping map from the sources folder.
    let continuum =
        processing
            .emission
            .clone()
            .map(|config| crate::studio_sources::ChannelContinuum {
                config,
                clipping: stacks
                    .iter()
                    .filter_map(|stack| {
                        let map = crate::server::stack_preview::read_clipped(
                            &folder.join(clipped_file(stack)),
                        )?;
                        Some((stack.role, map))
                    })
                    .collect(),
            });
    let mut prepared = RegisteredChannels::prepare(
        &inputs,
        (!tools.chains.is_empty() || !tools.deconvolutions.is_empty()).then_some(&tools),
        continuum.as_ref(),
        background.as_ref(),
        &seiza_stacking::GpuOptions::cpu(),
    )?;
    let calibration = calibrate_colors(&mut prepared, stacks, folder, &document);
    match &calibration {
        Calibration::NotAsked => {}
        Calibration::Skipped { why } => println!("  Colors not calibrated: {why}"),
        Calibration::Done { gains, stars, .. } => println!(
            "  Colors calibrated against Gaia from {stars} stars: red x{:.3}, blue x{:.3}",
            gains[0], gains[2]
        ),
    }
    // The stars are stretched before the sky is levelled: the star stretch
    // needs a channel with and without its stars at one sky level.
    // The largest rectangle every input covers: what the picture keeps
    // unless a person crops it otherwise. Registration leaves empty edges
    // where the inputs do not overlap.
    let crop = automatic_crop(&prepared);
    let (mut display, star_stretch) = stretch_stars(&mut prepared, &document);
    if let Calibration::Done {
        neutral_sky: true, ..
    } = calibration
    {
        prepared.level_sky(
            StackColorRole::Green,
            &[StackColorRole::Red, StackColorRole::Blue],
        );
    }
    let calibrated = matches!(calibration, Calibration::Done { .. });
    for name in stretch_whole_images(
        &mut prepared,
        &processing.input_stretches,
        calibrated,
        &display,
    ) {
        println!("  {name}: stretched for display");
        display.push(name);
    }
    if let Some(amount) = star_stretch {
        println!("  Stars of red, green and blue stretched as one layer, amount {amount:.2}");
    }
    for (role, chain) in &prepared.tools {
        let ran: Vec<&str> = chain
            .steps
            .iter()
            .map(|step| step.provider.as_str())
            .collect();
        println!("  {role:?} tools: {}", ran.join(", "));
    }
    for (role, words) in &prepared.continuum {
        println!("  {} continuum: {words}", role.label());
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
    write_atomically(&backgrounds, serde_json::to_string(&prepared.backgrounds)?)?;
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
        .deconvolutions
        .values()
        .map(|deconvolution| deconvolution.id.as_str())
        .chain(
            tools
                .chains
                .values()
                .filter_map(|chain| chain.ids.last().map(String::as_str)),
        )
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

    // Beside the store, so whatever opens it knows whether red, green and
    // blue are balanced and may share one stretch, and which planes are
    // already stretched for display.
    // Written after the store, so it never describes another store.
    let record = output.with_extension("prepared.json");
    let prepared_record = serde_json::json!({
        "calibration": calibration,
        "display": display,
        "star_stretch": star_stretch,
        "crop": crop,
    });
    write_atomically(&record, serde_json::to_string(&prepared_record)?)?;
    if let Some(path) = document_out {
        write_atomically(path, document.to_json())?;
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
        /// Whether the sky of red and blue is to be levelled to green's.
        #[serde(skip)]
        neutral_sky: bool,
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
    let settings: crate::server::stack_preview::color_calibration::StackColorCalibration =
        match step.map(|step| &step.kind) {
            Some(crate::studio::StepKind::Operation { settings, .. }) if !settings.is_null() => {
                match serde_json::from_value(settings.clone()) {
                    Ok(settings) => settings,
                    Err(error) => {
                        return Calibration::Skipped {
                            why: format!("the Color calibration step's settings: {error}"),
                        }
                    }
                }
            }
            _ => Default::default(),
        };
    if !settings.enabled {
        return Calibration::NotAsked;
    }
    let Some(orientation) = stacks[0].sky_orientation.as_ref() else {
        return Calibration::Skipped {
            why: "the first stack records no sky layout".into(),
        };
    };
    // A mosaic's grid is the canvas, which its reference frame's solve
    // does not cover; PSF Guard calibrates a mosaic with the canvas's own
    // solve, which Studio does not make yet.
    if crate::server::stack_preview::canvas_astrometry::is_mosaic(Some(orientation)) {
        return Calibration::Skipped {
            why: "a mosaic needs a solve of its canvas, which Studio does not make yet".into(),
        };
    }
    let Some(solution) = read_solve(folder, &stacks[0]) else {
        return Calibration::Skipped {
            why: "the sources have no plate solve of the first stack".into(),
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
            Calibration::Done {
                neutral_sky: settings.neutralize_background,
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
    // A star removal that unscreens its stars gives stars that do not add
    // back to the image, so there is no image with stars to stretch from.
    let unscreened = prepared.tools.iter().any(|(_, chain)| {
        chain.steps.iter().any(|step| {
            step.parameters
                .get("unscreen")
                .and_then(|value| serde_json::to_value(value).ok())
                == Some(serde_json::Value::Bool(true))
        })
    });
    if unscreened {
        println!("  The stars were not stretched: turn Unscreen Stars off for red, green and blue");
        return (Vec::new(), None);
    }
    let [Some(red_less), Some(green_less), Some(blue_less)] =
        names.map(|name| prepared.plane(&tools_channel(name)))
    else {
        return (Vec::new(), None);
    };
    // The image with its stars is the starless image plus the stars the
    // tool took out: the image as it entered star removal, after any
    // sharpening or noise reduction before it. The stack as it came would
    // put those tools' changes into the stars.
    let with_stars = |starless: &seiza_stacking::LinearImage, name: &str| {
        let mut image = starless.clone();
        let stars = prepared.plane(&stars_channel(name))?;
        for (sample, star) in image.data.iter_mut().zip(&stars.data) {
            *sample += *star;
        }
        Some(image)
    };
    let (Some(red), Some(green), Some(blue)) = (
        with_stars(&red_less, "red"),
        with_stars(&green_less, "green"),
        with_stars(&blue_less, "blue"),
    ) else {
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
    already_display: &[String],
) -> Vec<String> {
    use crate::studio::{
        background_corrected_channel, stars_channel, stretched_channel, tools_channel,
    };
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
    for (role, name) in layered.clone() {
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

    // The stars of a channel stretched this way follow it: the channel is
    // stretched again with its stars put back, and the stars are taken out
    // of that with Seiza's unscreen, so screening them over the starless
    // channel gives the channel with its stars. Stars already stretched for
    // display, by the star stretch, are left as they are.
    let mut groups: Vec<Vec<(StackColorRole, String)>> = Vec::new();
    if together {
        groups.push(
            layered
                .iter()
                .filter(|(role, _)| rgb.contains(role))
                .cloned()
                .collect(),
        );
    }
    groups.extend(
        layered
            .iter()
            .filter(|(role, _)| !(together && rgb.contains(role)))
            .map(|entry| vec![entry.clone()]),
    );
    for group in groups {
        let wanted = group.iter().all(|(_, name)| {
            let stars = stars_channel(name);
            prepared.names.contains(&stars) && !already_display.contains(&stars)
        });
        if !wanted || group.is_empty() {
            continue;
        }
        let with_stars: Option<Vec<seiza_stacking::LinearImage>> = group
            .iter()
            .map(|(_, name)| {
                let mut image = linear(prepared, name)?;
                let stars = prepared.plane(&stars_channel(name))?;
                for (sample, star) in image.data.iter_mut().zip(&stars.data) {
                    *sample += *star;
                }
                Some(image)
            })
            .collect();
        let Some(with_stars) = with_stars else {
            continue;
        };
        let role = group[0].0;
        let stretched: Result<Vec<Vec<f32>>, String> = if with_stars.len() == 3 {
            crate::server::stack_preview::color_calibration::interleave(
                &with_stars[0],
                &with_stars[1],
                &with_stars[2],
            )
            .and_then(|image| crate::color_steps::stretch_for_display(&image, &stretches[&role]))
            .map(|image| {
                (0..3)
                    .map(|channel| {
                        image
                            .data
                            .iter()
                            .skip(channel)
                            .step_by(3)
                            .copied()
                            .collect()
                    })
                    .collect()
            })
        } else {
            crate::color_steps::stretch_for_display(&with_stars[0], &stretches[&role])
                .map(|image| vec![image.data])
        };
        let stretched = match stretched {
            Ok(stretched) => stretched,
            Err(error) => {
                println!("  The stars of {} were not stretched: {error}", group[0].1);
                continue;
            }
        };
        for ((_, name), original) in group.iter().zip(stretched) {
            let Some(starless) = prepared.plane(&stretched_channel(name)) else {
                continue;
            };
            let stars: Vec<f32> = original
                .iter()
                .zip(&starless.data)
                .map(|(original, base)| {
                    if !original.is_finite() || !base.is_finite() {
                        f32::NAN
                    } else if *base >= 1.0 {
                        0.0
                    } else {
                        ((original - base) / (1.0 - base)).clamp(0.0, 1.0)
                    }
                })
                .collect();
            let plane = stars_channel(name);
            if prepared.replace_plane(&plane, stars) {
                added.push(plane);
            }
        }
    }
    added
}

/// Where a picture's channel stacks come from.
pub enum StudioFrom {
    /// A target's current stacks on a PSF Guard server.
    Server {
        server: String,
        database: String,
        token: Option<String>,
        project_id: i32,
        target_id: i32,
        channels: Vec<String>,
    },
    /// Stack files on this computer, as `channel=file`.
    Files {
        channels: Vec<String>,
        pointing: Option<(f64, f64, f64)>,
    },
}

pub struct StudioOptions {
    pub from: StudioFrom,
    /// The folder that holds the picture's store and document.
    pub folder: PathBuf,
    /// The picture's name in that folder.
    pub name: String,
    /// The PSF-Studio application, or its executable.
    pub app: Option<PathBuf>,
    /// Whether to open the picture once it is prepared.
    pub open: bool,
}

/// Prepares a picture from its channel stacks and opens it in PSF-Studio:
/// one command from a PSF Guard target, or from stack files, to the
/// editor.
///
/// The picture's store is `<name>.pgtiles` and its document `<name>.json`
/// in the folder. A document that is there already is the picture's: the
/// store is prepared for it, so its tools and stretches are kept. Without
/// one, a new document is written from Studio's default flow.
pub fn studio(mut options: StudioOptions) -> Result<()> {
    std::fs::create_dir_all(&options.folder)
        .with_context(|| format!("creating {}", options.folder.display()))?;
    // An application opened through the system starts in another folder,
    // so it is given whole paths.
    options.folder = std::path::absolute(&options.folder)
        .with_context(|| format!("finding {}", options.folder.display()))?;
    let store = options.folder.join(format!("{}.pgtiles", options.name));
    let document = options.folder.join(format!("{}.json", options.name));
    let known = document.is_file();
    let (apply, new_document) = if known {
        (Some(document.clone()), None)
    } else {
        (None, Some(document.clone()))
    };
    match options.from {
        StudioFrom::Server {
            server,
            database,
            token,
            project_id,
            target_id,
            channels,
        } => tokio::runtime::Runtime::new()?.block_on(studio_sources(StudioSourcesOptions {
            server,
            database,
            token,
            project_id,
            target_id,
            channels,
            document: new_document,
            apply,
            output: store.clone(),
        }))?,
        StudioFrom::Files { channels, pointing } => studio_files(StudioFilesOptions {
            channels,
            apply,
            document: new_document,
            pointing,
            output: store.clone(),
        })?,
    }
    if !options.open {
        return Ok(());
    }
    let app = options
        .app
        .or_else(|| std::env::var_os("PSF_STUDIO_APP").map(PathBuf::from))
        .ok_or_else(|| {
            anyhow!(
                "the picture is prepared at {}. To open it, give --app or set PSF_STUDIO_APP \
                 to the PSF-Studio application",
                store.display()
            )
        })?;
    // Studio runs the tools through this same program.
    let this_program = std::env::current_exe().context("finding this program")?;
    let arguments = [
        std::ffi::OsString::from("--open"),
        store.clone().into(),
        "--document".into(),
        document.into(),
        "--psf-guard".into(),
        this_program.into(),
    ];
    // A macOS application bundle is opened through the system, so it gets
    // a window of its own; an executable is started as it is.
    let bundle = app.extension().is_some_and(|extension| extension == "app");
    let mut command = if bundle && cfg!(target_os = "macos") {
        let mut command = std::process::Command::new("open");
        command.arg("-n").arg(&app).arg("--args");
        command
    } else {
        std::process::Command::new(&app)
    };
    command
        .args(arguments)
        .spawn()
        .with_context(|| format!("starting {}", app.display()))?;
    println!("Opened {} in PSF-Studio", store.display());
    Ok(())
}

/// A stack file as `channel=file`, with the channel read from the file:
/// its FILTER header, classified as PSF Guard classifies a filter's name,
/// or failing that the parts of the file's name.
pub fn channel_and_file(file: &str) -> Result<String> {
    use crate::server::stack_preview::color::classify_filter;

    let path = Path::new(file);
    let frame = crate::image_io::open_linear_frame(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let from_header = crate::image_io::header_text(&frame.headers, "FILTER")
        .and_then(|filter| classify_filter(filter.trim()));
    let from_name = || {
        let name = path.file_stem()?.to_string_lossy().into_owned();
        // The last part that is a filter's name: a target's name comes
        // first in most file names.
        name.split(|character: char| !character.is_alphanumeric())
            .rev()
            .find_map(classify_filter)
    };
    let role = from_header.or_else(from_name).ok_or_else(|| {
        anyhow!(
            "{} does not say which channel it is; give it as --channel CHANNEL={file}",
            path.display()
        )
    })?;
    Ok(format!("{}={file}", crate::studio_flow::role_name(role)?))
}

/// The largest rectangle in which every input, as stacked, covers every
/// pixel, with Seiza's crop: what PSF Guard's color previews keep with the
/// crop "Inscribed". As x, y, width and height, or `None` when the inputs
/// share no such rectangle.
fn automatic_crop(prepared: &RegisteredChannels) -> Option<[usize; 4]> {
    let inputs: Vec<(String, seiza_stacking::LinearImage)> = prepared
        .names
        .iter()
        .filter(|name| !name.contains('@'))
        .filter_map(|name| Some((name.clone(), prepared.plane(name)?)))
        .collect();
    let samples: Vec<seiza_stacking::ChannelSamples<'_>> = inputs
        .iter()
        .map(|(name, image)| seiza_stacking::ChannelSamples::new(name, image))
        .collect();
    let report =
        seiza_stacking::crop_report(&samples, seiza_stacking::ColorCrop::Inscribed).ok()?;
    let region = report.region;
    Some([region.x, region.y, region.width, region.height])
}

#[cfg(test)]
mod tests {
    use crate::server::stack_preview::color::{classify_filter, StackColorRole};

    #[test]
    fn a_file_name_gives_its_channel_from_its_last_filter_part() {
        let of = |name: &str| {
            name.split(|character: char| !character.is_alphanumeric())
                .rev()
                .find_map(classify_filter)
        };
        assert_eq!(of("Sh2-157_Ha_300s_master"), Some(StackColorRole::Ha));
        assert_eq!(of("masterLight_OIII"), Some(StackColorRole::Oiii));
        assert_eq!(of("M31-L"), Some(StackColorRole::Luminance));
        assert_eq!(of("integration"), None);
    }
}
