use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::server::stack_preview::color::{StackColorRole, StackColorSource};
use crate::server::workers::Workers;
use crate::studio::Document;
use crate::studio_sources::{RegisteredChannels, Server};

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
                // A map kept from an earlier revision is not this stack's.
                let _ = std::fs::remove_file(folder.join(clipped_file(stack)));
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

use psf_studio_core::prepare::{write_atomically, Calibration, Prepare, PrepareStack};

/// A picture's name from its sources folder: the folder's name without
/// `.sources`.
fn title_of_sources(folder: &Path) -> String {
    folder
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !crate::studio::is_generated_name(stem))
        .unwrap_or_else(|| crate::studio::UNTITLED_PICTURE.to_string())
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
        let stack = psf_studio_core::prepare::StackFile::new(role, Path::new(file))?;
        paths.insert(stack.identity.clone(), stack.path.clone());
        stacks.push(StackColorSource {
            role,
            filter_name: name.to_string(),
            label: stack.label,
            exposure_group: None,
            job_id: stack.identity,
            group_index: 0,
            artifact_revision: stack.revision,
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
    let title = psf_studio_core::prepare::picture_title(&frame.headers, &first);
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
    let document = psf_studio_core::prepare::picture_document(title, &roles, apply)?;
    // A tool's result is kept beside the downloaded stacks.
    let workers = Workers::default();
    let tools = crate::studio_sources::WorkerTools {
        workers: &workers,
        cache: folder,
    };
    let prepare_stacks: Vec<PrepareStack> = stacks
        .iter()
        .zip(files)
        .map(|(stack, (role, path))| PrepareStack {
            role,
            path,
            source: format!("mono:{}:{}", stack.job_id, stack.group_index),
            revision: stack.artifact_revision.clone(),
            key: format!(
                "{}:{}:{}",
                stack.job_id, stack.group_index, stack.artifact_revision
            ),
            clipping: crate::server::stack_preview::read_clipped(&folder.join(clipped_file(stack))),
        })
        .collect();
    // One line per tool and per hundredth of its chain, which PSF Studio
    // shows as a progress bar.
    let reported = std::cell::RefCell::new((String::new(), u32::MAX));
    let progress = |channel: &str, tool: &str, fraction: f32| {
        let percent = (fraction.clamp(0.0, 1.0) * 100.0) as u32;
        let line = format!("{channel}: {tool}");
        let mut last = reported.borrow_mut();
        if *last != (line.clone(), percent) {
            println!("  {line} ({percent}%)");
            *last = (line, percent);
        }
    };
    psf_studio_core::prepare::prepare_store(Prepare {
        stacks: &prepare_stacks,
        document: &document,
        tools: Some(&tools),
        folder,
        output,
        calibrate: &|prepared| calibrate_colors(prepared, stacks, folder, &document),
        report: &|line| println!("{line}"),
        progress: &progress,
    })?;
    if let Some(path) = document_out {
        write_atomically(path, document.to_json())?;
        println!("Wrote the document to {}", path.display());
    }
    Ok(())
}

/// What `psf-guard studio-tool` runs: one processing tool on one image.
pub struct StudioToolOptions {
    /// The image, a FITS file of one channel or three.
    pub input: PathBuf,
    /// The tool, as one step of a tool chain: `{"provider": ..., "parameters": {...}}`.
    pub tool: String,
    /// Where the tool's result is written, as a 32-bit FITS file.
    pub output: PathBuf,
    /// Where the stars a star removal took out are written, if it kept any.
    pub stars: Option<PathBuf>,
    /// Where each run's result is kept, so a repeat run is fast.
    pub cache: PathBuf,
    /// Whether the tool runs a second time on the image shifted by half a
    /// tile, and the two results are averaged.
    pub shifted: bool,
}

/// Runs one processing tool on an image PSF Studio rendered, such as one
/// layer or the picture at a Finish step, and writes its result. The result
/// is kept by the image's content and the tool's settings, so the same
/// image and tool run once.
pub fn studio_tool(options: StudioToolOptions) -> Result<()> {
    use crate::server::stack_preview::rc_astro::{ProcessingChain, ProcessingStep};
    use crate::server::workers::rc_astro::RcAstroRunner;
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(&options.input)
        .with_context(|| format!("reading {}", options.input.display()))?;
    let digest: String = Sha256::digest(&bytes)
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    drop(bytes);
    let mut frame = crate::image_io::open_linear_frame(&options.input)
        .with_context(|| format!("reading {}", options.input.display()))?;
    // The tools clip at 1. An image of an HDR picture reaches above it, so
    // it goes to them scaled into 0 to 1 by its brightest sample, and comes
    // back at its own scale.
    let brightest = frame
        .image
        .data
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .fold(0.0_f32, f32::max);
    let scale = brightest.max(1.0);
    if scale > 1.0 {
        for sample in &mut frame.image.data {
            *sample /= scale;
        }
    }
    let digest = format!("{digest}:unit");
    let tool: ProcessingStep =
        serde_json::from_str(&options.tool).context("the tool is not a step of a tool chain")?;
    let workers = Workers::default();
    let chain = ProcessingChain { steps: vec![tool] };
    std::fs::create_dir_all(&options.cache)?;
    let reported = std::cell::RefCell::new((String::new(), u32::MAX));
    let report = |tool: &str, fraction: f32| {
        let percent = (fraction.clamp(0.0, 1.0) * 100.0) as u32;
        let line = format!("picture: {tool}");
        let mut last = reported.borrow_mut();
        if *last != (line.clone(), percent) {
            println!("  {line} ({percent}%)");
            *last = (line, percent);
        }
    };
    let run = |image: &seiza_stacking::LinearImage, revision: &str, label: &str| {
        let resolved = crate::color_steps::ToolChain::resolve(
            &workers, &chain, "studio", "picture", revision, None,
        )
        .map_err(anyhow::Error::msg)?;
        crate::color_steps::run_tool_chain(
            RcAstroRunner {
                workers: &workers,
                person: true,
            },
            &options.cache,
            &resolved,
            image,
            &frame.headers,
            None,
            |crate::color_steps::ToolEvent::Running {
                 label: tool,
                 fraction,
             }| { report(&format!("{tool}{label}"), fraction) },
            &mut || Ok(()),
        )
        .map_err(anyhow::Error::msg)
    };
    let started = Instant::now();
    let mut outcome = run(&frame.image, &digest, "")?;
    if options.shifted {
        use crate::studio_sources::{shifted_passes, Pass};
        let first = Pass {
            image: outcome.image,
            stars: outcome.stars,
        };
        let both = shifted_passes(&frame.image, first, &mut |image, suffix| {
            let moved = run(image, &format!("{digest}:{suffix}"), ", shifted")?;
            Ok(Pass {
                image: moved.image,
                stars: moved.stars,
            })
        })?;
        outcome.image = both.image;
        outcome.stars = both.stars;
    }
    if scale > 1.0 {
        for image in std::iter::once(&mut outcome.image).chain(outcome.stars.as_mut()) {
            for sample in &mut image.data {
                *sample *= scale;
            }
        }
    }
    let write = |path: &Path, image: &seiza_stacking::LinearImage| -> Result<()> {
        let data = match image.channels {
            1 => seiza_fits::F32ImageData::Mono(&image.data),
            3 => seiza_fits::F32ImageData::RgbInterleaved(&image.data),
            other => bail!("a tool gave {other} channels"),
        };
        let partial = path.with_extension("partial.fits");
        seiza_fits::write_f32_image(&partial, image.width, image.height, data, &[])
            .map_err(|error| anyhow!("writing {}: {error}", path.display()))?;
        std::fs::rename(&partial, path)?;
        Ok(())
    };
    // The stars first: the result's file is how a reader knows the run is
    // complete, stars and all.
    if let (Some(path), Some(stars)) = (&options.stars, &outcome.stars) {
        write(path, stars)?;
    }
    write(&options.output, &outcome.image)?;
    println!(
        "Ran the tool in {:.1} s{}",
        started.elapsed().as_secs_f64(),
        if outcome.stars.is_some() {
            ", with stars"
        } else {
            ""
        }
    );
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

/// The PSF Studio that ships inside this PSF Guard app, at
/// `PSF Guard.app/Contents/Helpers/PSF Studio.app`, when this program is
/// that app's.
pub fn bundled_studio() -> Option<PathBuf> {
    studio_beside(&std::env::current_exe().ok()?)
}

/// The Studio in the app whose program is `executable`.
fn studio_beside(executable: &Path) -> Option<PathBuf> {
    let contents = executable.parent()?.parent()?;
    let studio = contents.join("Helpers").join("PSF Studio.app");
    studio.is_dir().then_some(studio)
}

/// Tells macOS that the bundled PSF Studio opens `psf-studio://` links, so
/// **Open in PSF Studio** works once PSF Guard has run. macOS finds an app
/// in Applications on its own, but not one inside another app. Nothing to
/// do elsewhere, or without a bundled Studio.
pub fn register_bundled_studio() {
    #[cfg(target_os = "macos")]
    if let Some(studio) = bundled_studio() {
        const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/\
LaunchServices.framework/Support/lsregister";
        match std::process::Command::new(LSREGISTER)
            .arg("-f")
            .arg(&studio)
            .status()
        {
            Ok(status) if status.success() => {
                tracing::info!("Registered {} for psf-studio links", studio.display())
            }
            Ok(status) => tracing::warn!("Registering PSF Studio for links ended with {status}"),
            Err(error) => tracing::warn!("PSF Studio was not registered for links: {error}"),
        }
    }
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
        .or_else(bundled_studio)
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
    let role = psf_studio_core::prepare::channel_of_file(Path::new(file))
        .map_err(|error| anyhow!("{error:#}; give it as --channel CHANNEL={file}"))?;
    Ok(format!("{}={file}", crate::studio_flow::role_name(role)?))
}

#[cfg(test)]
mod tests {
    use crate::server::stack_preview::color::{classify_filter, StackColorRole};

    #[test]
    fn every_provider_has_the_tool_step_studio_gives_it() {
        use crate::remote_work::steps::{Category, PROVIDERS};
        use crate::studio::flow_operations as op;
        assert_eq!(crate::studio::TOOL_PROVIDERS.len(), PROVIDERS.len());
        for provider in PROVIDERS {
            let wanted = match provider.category {
                Category::Sharpen => op::SHARPENING,
                Category::Denoise => op::NOISE_REDUCTION,
                Category::StarRemoval => op::STAR_REMOVAL,
            };
            assert_eq!(provider.category.label(), wanted);
            assert_eq!(
                crate::studio::tool_operation(provider.key),
                wanted,
                "{}",
                provider.key
            );
        }
    }

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

    #[test]
    fn the_bundled_studio_is_inside_the_app() {
        let folder = tempfile::tempdir().unwrap();
        let contents = folder.path().join("PSF Guard.app/Contents");
        std::fs::create_dir_all(contents.join("Helpers/PSF Studio.app")).unwrap();
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        assert_eq!(
            super::studio_beside(&contents.join("MacOS/psf-guard")),
            Some(contents.join("Helpers/PSF Studio.app"))
        );
        // A program outside an app has none.
        assert_eq!(
            super::studio_beside(&folder.path().join("bin/psf-guard")),
            None
        );
    }
}
