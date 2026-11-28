//! Preparing a PSF Studio picture: channel stacks registered onto one grid,
//! the document's Linear steps run on them, the layered stretches resolved
//! over the whole image, and the result written as a tile store with a
//! record beside it.
//!
//! PSF Guard prepares a picture with [`prepare_store`] from a server's
//! stacks or from files, and PSF Studio does when it opens stack files
//! itself. The caller chooses what runs the processing tools, if anything,
//! and whether colors are calibrated.

use crate::channels::{
    line_channel, ChannelContinuum, LinearProgram, LinearStep, LinearTools, RegisteredChannels,
};
use crate::color::{
    interleave, role_name, stretch_for_display, unscreen, EmissionProcessing, StackColorRole,
    StackStretchRequest,
};
use crate::studio::{
    color_setup_from_document, flow_operations as op, input_program, is_tool_step, stars_channel,
    stretched_channel, Document, StageKind, StepKind, StoreOptions, TileStore,
};
use anyhow::{bail, Context, Result};
use seiza_stacking::LinearImage;
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// One channel stack to prepare a picture from.
pub struct PrepareStack {
    pub role: StackColorRole,
    /// The stack's file, FITS or XISF.
    pub path: PathBuf,
    /// Where the stack comes from and its revision: the tools' results are
    /// kept by them, and a new revision runs the tools again.
    pub source: String,
    pub revision: String,
    /// What the store is keyed by for this stack: it changes when the
    /// stack is rebuilt.
    pub key: String,
    /// Where the stack clipped, as its stacker recorded it: what continuum
    /// subtraction keeps out of its fit.
    pub clipping: Option<LinearImage>,
}

/// What color calibration did, as the record beside a store keeps it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Calibration {
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

/// A picture to prepare.
pub struct Prepare<'a, R: LinearTools> {
    pub stacks: &'a [PrepareStack],
    pub document: &'a Document,
    /// What runs the document's processing tools and deconvolutions. With
    /// none, those steps do not run, and the record beside the store leaves
    /// them out, so PSF Studio shows them as still to run.
    pub tools: Option<&'a R>,
    /// Where the fitted backgrounds are written.
    pub folder: &'a Path,
    /// The tile store to write. Its record goes beside it, as
    /// `<store>.prepared.json`.
    pub output: &'a Path,
    /// Calibrates red, green and blue, as registered: PSF Guard measures
    /// them against Gaia. A caller that cannot returns
    /// [`Calibration::NotAsked`] or [`Calibration::Skipped`].
    pub calibrate: &'a dyn Fn(&mut RegisteredChannels) -> Calibration,
    /// Hears a line about what was done, for a person to read.
    pub report: &'a dyn Fn(&str),
    /// Hears the channel, the tool running on it, and how far the tool's
    /// chain has got, from 0 to 1.
    pub progress: &'a dyn Fn(&str, &str, f32),
}

/// Registers the stacks, runs the document's Linear steps on them, fits the
/// backgrounds and writes the store and its record.
pub fn prepare_store<R: LinearTools>(prepare: Prepare<'_, R>) -> Result<()>
where
    R::Tool: DeserializeOwned,
{
    let Prepare {
        stacks,
        document,
        tools,
        folder,
        output,
        calibrate,
        report,
        progress,
    } = prepare;
    if stacks.is_empty() {
        bail!("there are no channel stacks to prepare");
    }
    let roles: Vec<StackColorRole> = stacks.iter().map(|stack| stack.role).collect();
    let settings = color_setup_from_document(document)?;
    let emission: Option<EmissionProcessing> = match settings.get("emission") {
        Some(value) if !value.is_null() => Some(
            serde_json::from_value(value.clone())
                .context("the document's continuum subtraction")?,
        ),
        _ => None,
    };
    let input_stretches: BTreeMap<StackColorRole, Vec<StackStretchRequest>> =
        match settings.get("input_stretches") {
            Some(value) if !value.is_null() => {
                serde_json::from_value(value.clone()).context("the document's input stretches")?
            }
            _ => BTreeMap::new(),
        };

    // The Linear steps that change the channels, in the document's order.
    // Without tools, the tool and deconvolution steps are left to run later.
    let (steps, skipped) = linear_steps::<R::Tool>(document, &roles, tools.is_some())?;
    let inputs: Vec<_> = stacks
        .iter()
        .map(|stack| (stack.role, stack.path.as_path()))
        .collect();
    let continuum = emission.clone().map(|config| ChannelContinuum {
        config,
        clipping: stacks
            .iter()
            .filter_map(|stack| Some((stack.role, stack.clipping.clone()?)))
            .collect(),
    });
    let started = Instant::now();
    let program = tools.map(|tools| LinearProgram {
        tools,
        sources: stacks
            .iter()
            .map(|stack| (stack.role, (stack.source.clone(), stack.revision.clone())))
            .collect(),
        steps,
        progress,
    });
    let mut prepared = RegisteredChannels::prepare(
        &inputs,
        program.as_ref(),
        continuum.as_ref(),
        &seiza_stacking::GpuOptions::cpu(),
    )?;
    let calibration = calibrate(&mut prepared);
    match &calibration {
        Calibration::NotAsked => {}
        Calibration::Skipped { why } => report(&format!("  Colors not calibrated: {why}")),
        Calibration::Done { gains, stars, .. } => report(&format!(
            "  Colors calibrated against Gaia from {stars} stars: red x{:.3}, blue x{:.3}",
            gains[0], gains[2]
        )),
    }
    // The largest rectangle every input covers: what the picture keeps
    // unless a person crops it otherwise. Registration leaves empty edges
    // where the inputs do not overlap.
    let crop = automatic_crop(&prepared);
    // The stars stay linear: PSF Studio stretches them on the canvas, so
    // the star stretch's amount redraws without preparing again.
    let mut display: Vec<String> = Vec::new();
    let star_stretch: Option<f64> = None;
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
        &input_stretches,
        calibrated,
        &display,
        report,
    ) {
        report(&format!("  {name}: stretched for display"));
        display.push(name);
    }
    for (role, tool) in &prepared.tools {
        report(&format!("  {role:?} tools: {tool}"));
    }
    for (role, words) in &prepared.continuum {
        report(&format!("  {} continuum: {words}", role.label()));
    }
    for placement in &prepared.placements {
        match &placement.registration {
            None => report(&format!("  {:?} is the reference", placement.role)),
            Some(registration) => report(&format!(
                "  {:?} registered: {:.3} px RMS, {:.1} px drift, {} stars",
                placement.role,
                registration.rms_error_pixels,
                registration.drift_pixels,
                registration.matched_stars
            )),
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
        report(&format!("  {role:?} background: {model}"));
    }
    std::fs::create_dir_all(folder).with_context(|| format!("creating {}", folder.display()))?;
    write_atomically(
        &folder.join("backgrounds.json"),
        serde_json::to_string(&prepared.backgrounds)?,
    )?;
    report(&format!(
        "Prepared in {:.1} s",
        started.elapsed().as_secs_f64()
    ));
    let registered = prepared;

    // The steps that ran for each channel, in order, with their settings:
    // the record PSF Studio compares with the document to tell which steps
    // are still to run.
    let programs: serde_json::Map<String, serde_json::Value> = document
        .channels()
        .into_iter()
        .map(|channel| {
            let program = input_program(document, &channel)
                .into_iter()
                .filter(|step| {
                    !step["step"]
                        .as_str()
                        .and_then(|id| id.parse::<uuid::Uuid>().ok())
                        .is_some_and(|id| skipped.contains(&id))
                })
                .collect();
            (channel, serde_json::Value::Array(program))
        })
        .collect();
    // The store's key changes when any channel's stack is rebuilt, with the
    // Linear steps that ran and their settings, and with continuum
    // subtraction: its settings, and which stacks had a clipping map.
    let key = stacks
        .iter()
        .map(|stack| stack.key.as_str())
        .collect::<Vec<_>>()
        .join("|");
    let key = {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(serde_json::to_string(&programs)?);
        let digest: String = digest
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("{key}|steps:{digest}")
    };
    let key = match &continuum {
        Some(continuum) => format!(
            "{key}|continuum:{}:{}",
            serde_json::to_string(&continuum.config).unwrap_or_default(),
            continuum
                .clipping
                .keys()
                .map(|role| role.label())
                .collect::<Vec<_>>()
                .join(",")
        ),
        None => key,
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
    let shape = crate::studio::RegionReader::shape(&registered);
    report(&format!(
        "Wrote {}: {} x {} pixels, {} channel(s), {} levels, in {:.1} s",
        output.display(),
        shape.width,
        shape.height,
        shape.channels,
        store.levels(),
        started.elapsed().as_secs_f64()
    ));

    // Beside the store, so whatever opens it knows whether red, green and
    // blue are balanced and may share one stretch, and which planes are
    // already stretched for display. Written after the store, so it never
    // describes another store.
    let record = output.with_extension("prepared.json");
    let prepared_record = serde_json::json!({
        "calibration": calibration,
        "display": display,
        "star_stretch": star_stretch,
        "crop": crop,
        // Each channel's states, in the order the steps ran, and the steps
        // the store was prepared with, so a canvas can draw any step and
        // tell when the document has moved on.
        "states": registered
            .states
            .iter()
            .map(|(channel, states)| {
                let states: Vec<serde_json::Value> = states
                    .iter()
                    .map(|state| {
                        serde_json::json!({
                            "step": state.step.to_string(),
                            "plane": state.plane,
                            "correction": state.correction,
                        })
                    })
                    .collect();
                (channel.clone(), serde_json::Value::Array(states))
            })
            .collect::<serde_json::Map<_, _>>(),
        "program": programs,
    });
    write_atomically(&record, serde_json::to_string(&prepared_record)?)?;
    Ok(())
}

/// The document's Linear steps that change the stacks' samples, in its
/// order, as preparing a store runs them: each tool step, deconvolution and
/// background extraction that is on. A color group's tool runs on the
/// group's stacks, when the document has two of them or more.
///
/// With `tools` false, tool steps and deconvolutions are left out, and
/// their ids are returned as the steps not run.
pub fn linear_steps<T: DeserializeOwned>(
    document: &Document,
    roles: &[StackColorRole],
    tools: bool,
) -> Result<(Vec<LinearStep<T>>, BTreeSet<uuid::Uuid>)> {
    let role_of = |band: &str| {
        roles
            .iter()
            .find(|role| role_name(**role).ok().as_deref() == Some(band))
            .copied()
    };
    let mut steps = Vec::new();
    let mut skipped = BTreeSet::new();
    for step in &document.stage(StageKind::Linear).steps {
        let StepKind::Operation { settings, .. } = &step.kind else {
            continue;
        };
        if !step.enabled {
            continue;
        }
        let name = step.kind.name();
        let what = || match &step.band {
            Some(band) => format!("{name} ({band})"),
            None => name.to_string(),
        };
        if name == op::BACKGROUND {
            let roles: Vec<StackColorRole> = match &step.band {
                None => roles.to_vec(),
                Some(band) => role_of(band).into_iter().collect(),
            };
            let extraction = serde_json::from_value(settings.clone())
                .with_context(|| format!("the settings of {}", what()))?;
            steps.push(LinearStep::Background {
                step: step.id,
                roles,
                extraction,
            });
        } else if name == op::DECONVOLUTION {
            let Some(role) = step.band.as_deref().and_then(role_of) else {
                continue;
            };
            if !tools {
                skipped.insert(step.id);
                continue;
            }
            let config = serde_json::from_value(settings.clone())
                .with_context(|| format!("the settings of {}", what()))?;
            steps.push(LinearStep::Deconvolution {
                step: step.id,
                role,
                config,
            });
        } else if is_tool_step(step) {
            let Some(band) = step.band.as_deref() else {
                continue;
            };
            let roles: Vec<StackColorRole> = match crate::studio::COLOR_GROUPS
                .iter()
                .find(|(group, _)| *group == band)
            {
                Some((_, members)) => members
                    .iter()
                    .filter_map(|member| role_of(member))
                    .collect(),
                None => role_of(band).into_iter().collect(),
            };
            if roles.is_empty() {
                continue;
            }
            if !tools {
                skipped.insert(step.id);
                continue;
            }
            let tool = serde_json::from_value(settings.clone())
                .with_context(|| format!("the settings of {}", what()))?;
            steps.push(LinearStep::Tool {
                step: step.id,
                roles,
                label: band.to_string(),
                tool,
                shifted: settings[crate::studio::SHIFTED_PASS_KEY]
                    .as_bool()
                    .unwrap_or(false),
            });
        }
    }
    Ok((steps, skipped))
}

/// Stretches each channel whose stretches include the layered stretch over
/// the whole image, and adds the result as the channel's stretched plane.
/// Returns the planes added or replaced, which are stretched for display.
///
/// The channel is stretched in its last linear state: after the last
/// Linear step that changed it, a background correction there in full, or
/// as stacked. Calibrated red, green and blue with the same stretches are
/// stretched together, as one color image, so they keep their balance.
///
/// The stars of such a channel follow it: the channel with its stars put
/// back is stretched with the very stretch the starless channel got, and
/// the stars are unscreened from that, so they screen back onto the
/// starless channel as they were. Stars the star stretch already made are
/// left as they are. A line with its continuum subtracted is stretched as
/// its channel's stretch chain resolves for it.
fn stretch_whole_images(
    prepared: &mut RegisteredChannels,
    stretches: &BTreeMap<StackColorRole, Vec<StackStretchRequest>>,
    calibrated: bool,
    already_display: &[String],
    report: &dyn Fn(&str),
) -> Vec<String> {
    use seiza_stretch::StretchModel;

    let linear = |prepared: &RegisteredChannels, name: &str| prepared.last_state(name);
    // One image of one channel, or three interleaved, and back.
    let combine = |images: &[LinearImage]| -> Result<LinearImage, String> {
        match images {
            [one] => Ok(one.clone()),
            [red, green, blue] => interleave(red, green, blue),
            _ => Err("a group is one channel or three".to_string()),
        }
    };
    let split = |image: &LinearImage| -> Vec<Vec<f32>> {
        (0..image.channels)
            .map(|channel| {
                image
                    .data
                    .iter()
                    .skip(channel)
                    .step_by(image.channels)
                    .copied()
                    .collect()
            })
            .collect()
    };
    let layered: Vec<(StackColorRole, String)> = stretches
        .iter()
        .filter(|(_, stages)| {
            stages.iter().any(|stage| {
                matches!(
                    stage.model,
                    StretchModel::Layered(_) | StretchModel::LayeredHdr(_)
                )
            })
        })
        .filter_map(|(role, _)| Some((*role, role_name(*role).ok()?)))
        .collect();
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

    let mut added = Vec::new();
    for group in groups {
        let stages = &stretches[&group[0].0];
        let label = group
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let Some(images) = group
            .iter()
            .map(|(_, name)| linear(prepared, name))
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let stretched = combine(&images).and_then(|image| stretch_for_display(&image, stages));
        let (stretched, stretch) = match stretched {
            Ok(stretched) => stretched,
            Err(error) => {
                report(&format!("  {label} was not stretched: {error}"));
                continue;
            }
        };
        let starless = split(&stretched);
        for ((_, name), samples) in group.iter().zip(&starless) {
            let plane = stretched_channel(name);
            if prepared.add_plane(&plane, samples.clone()) {
                added.push(plane);
            }
        }

        // The stars, with the starless image's own stretch.
        let wanted = group.iter().all(|(_, name)| {
            let stars = stars_channel(name);
            prepared.names.contains(&stars) && !already_display.contains(&stars)
        });
        if wanted {
            let with_stars: Option<Vec<LinearImage>> = group
                .iter()
                .zip(&images)
                .map(|((_, name), image)| {
                    let mut image = image.clone();
                    let stars = prepared.plane(&stars_channel(name))?;
                    for (sample, star) in image.data.iter_mut().zip(&stars.data) {
                        *sample += *star;
                    }
                    Some(image)
                })
                .collect();
            let original = with_stars
                .ok_or_else(|| "a stars plane is missing".to_string())
                .and_then(|images| combine(&images))
                .and_then(|image| stretch.apply(&image));
            match original {
                Ok(original) => {
                    for (((_, name), original), base) in
                        group.iter().zip(split(&original)).zip(&starless)
                    {
                        let stars = original
                            .iter()
                            .zip(base)
                            .map(|(original, base)| unscreen(*original, *base))
                            .collect();
                        let plane = stars_channel(name);
                        if prepared.replace_plane(&plane, stars) {
                            added.push(plane);
                        }
                    }
                }
                Err(error) => report(&format!(
                    "  The stars of {label} were not stretched: {error}"
                )),
            }
        }

        // A line with its continuum subtracted, stretched as its channel's
        // chain resolves for it.
        for (_, name) in &group {
            let line = line_channel(name);
            let Some(image) = prepared.plane(&line) else {
                continue;
            };
            match stretch_for_display(&image, stages) {
                Ok((stretched, _)) => {
                    let plane = stretched_channel(&line);
                    if prepared.add_plane(&plane, stretched.data) {
                        added.push(plane);
                    }
                }
                Err(error) => report(&format!("  {line} was not stretched: {error}")),
            }
        }
    }
    added
}

/// The largest rectangle in which every input, as stacked, covers every
/// pixel, with Seiza's crop: what PSF Guard's color previews keep with the
/// crop "Inscribed". As x, y, width and height, or `None` when the inputs
/// share no such rectangle.
fn automatic_crop(prepared: &RegisteredChannels) -> Option<[usize; 4]> {
    let inputs: Vec<(String, LinearImage)> = prepared
        .names
        .iter()
        .filter(|name| !name.contains('@'))
        .filter_map(|name| Some((name.clone(), prepared.plane(name)?)))
        .collect();
    let samples: Vec<seiza_compose::ChannelSamples<'_>> = inputs
        .iter()
        .map(|(name, image)| seiza_compose::ChannelSamples::new(name, image))
        .collect();
    let report = seiza_compose::crop_report(&samples, seiza_compose::ColorCrop::Inscribed).ok()?;
    let region = report.region;
    Some([region.x, region.y, region.width, region.height])
}

/// Writes a file through a temporary file beside it, renamed over it, so a
/// crash or a kill never leaves half a file.
pub fn write_atomically(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".writing");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, contents)
        .with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("writing {}", path.display()))
}
