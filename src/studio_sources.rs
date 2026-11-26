//! A PSF-Studio document's sources: PSF Guard's channel stacks.
//!
//! PSF Guard stacks each channel. Studio starts from those stacks and does
//! the color processing itself. This module lists a target's channel stacks
//! on a server, downloads them, and registers them onto one grid with
//! Seiza, which is the first thing every later step needs.

use crate::color_steps::{
    correct_channel_backgrounds, place_on_reference, register_channels, run_tool_chain,
    BackgroundEvent, ToolChain, ToolEvent,
};
use crate::image_steps::{Domain, ImageShape, Region};
use crate::server::stack_preview::color::{
    StackBackgroundExtraction, StackColorRole, StackColorSource,
};
use crate::server::stack_preview::rc_astro::ProcessingChain;
use crate::server::workers::rc_astro::RcAstroRunner;
use crate::server::workers::Workers;
use crate::studio::{
    background_corrected_channel, stars_channel, tools_channel, RegionReader, StudioError,
};
use crate::studio_flow::role_name;
use anyhow::{anyhow, bail, Context, Result};
use seiza_background::BackgroundFit;
use seiza_stacking::{GpuOptions, LinearImage};
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::path::Path;

/// One database on a PSF Guard server.
pub struct Server {
    client: reqwest::Client,
    /// `http://host:port/api/db/<database>`.
    database_url: String,
    token: Option<String>,
}

impl Server {
    pub fn new(origin: &str, database: &str, token: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            database_url: format!("{}/api/db/{database}", origin.trim_end_matches('/')),
            token,
        }
    }

    fn get(&self, url: String) -> reqwest::RequestBuilder {
        let request = self.client.get(url);
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    /// The plate solve of one frame: the one the server has, or a new one
    /// when it has none. `None` when the frame cannot be solved; the reason
    /// is the server's.
    pub async fn frame_solve(
        &self,
        image_id: i32,
    ) -> Result<Option<crate::astrometry::AstrometrySolutionResponse>> {
        let url = format!("{}/images/{image_id}/astrometry", self.database_url);
        let known: serde_json::Value = self.data(url.clone()).await?;
        // Only a solve of the pixels shows where the stars are; a WCS from
        // the file's header may describe other pixels.
        let pixel_solve = !known["solution"].is_null() && known["mode"] != "embedded_wcs";
        let analysis = if !pixel_solve {
            let request = self.client.post(url);
            let request = match &self.token {
                Some(token) => request.bearer_auth(token),
                None => request,
            };
            let response = request.send().await?;
            let status = response.status();
            let body: serde_json::Value = response
                .json()
                .await
                .with_context(|| format!("reading the server's reply ({status})"))?;
            if !status.is_success() {
                let reason = body["error"].as_str().unwrap_or("no reason given");
                bail!("the server could not solve the frame ({status}): {reason}");
            }
            body["data"].clone()
        } else {
            known
        };
        if analysis["solution"].is_null() {
            return Ok(None);
        }
        serde_json::from_value(analysis["solution"].clone())
            .map(Some)
            .context("reading the frame's plate solve")
    }

    /// Reads the `data` of the server's reply to a GET.
    async fn data<T: DeserializeOwned>(&self, url: String) -> Result<T> {
        let response = self.get(url).send().await?;
        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .with_context(|| format!("reading the server's reply ({status})"))?;
        if !status.is_success() {
            let reason = body["error"].as_str().unwrap_or("no reason given");
            bail!("the server refused the request ({status}): {reason}");
        }
        serde_json::from_value(body["data"].clone()).context("reading the server's reply")
    }

    /// A target's name and its current channel stacks, one per channel, in
    /// the server's order. A channel with several current stacks is
    /// refused: the caller must say which stack it means.
    pub async fn channel_stacks(
        &self,
        project_id: i32,
        target_id: i32,
    ) -> Result<(String, Vec<StackColorSource>)> {
        let catalog: serde_json::Value = self
            .data(format!(
                "{}/projects/{project_id}/stack-previews/color",
                self.database_url
            ))
            .await?;
        let target = catalog["targets"]
            .as_array()
            .and_then(|targets| {
                targets
                    .iter()
                    .find(|target| target["target_id"] == target_id)
            })
            .ok_or_else(|| {
                anyhow!("project {project_id} has no channel stacks for target {target_id}")
            })?;
        let stacks: Vec<StackColorSource> =
            serde_json::from_value(target["source_candidates"].clone())
                .context("reading the target's channel stacks")?;
        let mut one_each: Vec<StackColorSource> = Vec::new();
        for stack in stacks {
            if one_each.iter().any(|kept| kept.role == stack.role) {
                bail!(
                    "target {target_id} has more than one current {} stack; choosing between them is not built yet",
                    stack.filter_name
                );
            }
            one_each.push(stack);
        }
        let name = target["target_name"]
            .as_str()
            .map_or_else(|| format!("Target {target_id}"), str::to_string);
        Ok((name, one_each))
    }

    /// Downloads a channel stack's FITS to `path`, a piece at a time.
    pub async fn download_stack(&self, stack: &StackColorSource, path: &Path) -> Result<u64> {
        self.download(stack, "fits", path)
            .await?
            .ok_or_else(|| anyhow!("the server has no {} stack", stack.filter_name))
    }

    /// Downloads the map of where a stack's frames clipped, which the
    /// stacker recorded. `None` when the stack has none.
    pub async fn download_clipped(
        &self,
        stack: &StackColorSource,
        path: &Path,
    ) -> Result<Option<u64>> {
        self.download(stack, "clipped", path).await
    }

    /// Downloads one file of a stack to `path` through a partial file.
    /// `None` when the server has no such file.
    async fn download(
        &self,
        stack: &StackColorSource,
        file: &str,
        path: &Path,
    ) -> Result<Option<u64>> {
        use std::io::Write;
        let url = format!(
            "{}/stack-previews/{}/{}/{file}?v={}",
            self.database_url, stack.job_id, stack.group_index, stack.artifact_revision
        );
        let response = self.get(url).send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND && file != "fits" {
            let _ = std::fs::remove_file(path);
            return Ok(None);
        }
        let mut response = response
            .error_for_status()
            .with_context(|| format!("downloading the {} stack", stack.filter_name))?;
        let partial = path.with_extension("fits.part");
        let mut output = std::fs::File::create(&partial)
            .with_context(|| format!("creating {}", partial.display()))?;
        let mut bytes = 0;
        while let Some(chunk) = response.chunk().await? {
            output.write_all(&chunk)?;
            bytes += chunk.len() as u64;
        }
        output.sync_all()?;
        std::fs::rename(&partial, path).with_context(|| format!("writing {}", path.display()))?;
        Ok(Some(bytes))
    }
}

/// How one channel was placed on the reference channel's grid.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelPlacement {
    pub role: StackColorRole,
    /// `None` for the reference channel, which is not moved.
    pub registration: Option<ChannelRegistration>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelRegistration {
    pub rms_error_pixels: f64,
    pub drift_pixels: f64,
    pub matched_stars: usize,
}

/// Channel stacks on one grid: a linear image with one channel per plane.
///
/// The planes are, in order: every stack as it came; for each channel with
/// processing tools, the tools' result ([`tools_channel`]) and the stars a
/// star removal took out ([`stars_channel`]); and, with background
/// correction, every channel corrected in full
/// ([`background_corrected_channel`]). The corrected plane is the tools'
/// result corrected, where a channel has tools. A strength
/// between is a blend of the two, because Seiza's correction changes each
/// sample in proportion to the strength and resampling keeps proportions.
/// So a person can move the strength and see the result at once, with no
/// refit.
///
/// The channels stay as separate planes, as Seiza gave them. A region is
/// interleaved when it is read, so the channels are never copied whole.
pub struct RegisteredChannels {
    shape: ImageShape,
    planes: Vec<Vec<f32>>,
    /// A name for each plane: the channel's role, and for a corrected
    /// plane [`background_corrected_channel`] of it.
    pub names: Vec<String>,
    pub placements: Vec<ChannelPlacement>,
    /// Each channel's fitted background, when the channels were corrected.
    pub backgrounds: Vec<(StackColorRole, BackgroundFit)>,
    /// The processing tools that ran on each channel that had any, with
    /// their settings as they were run.
    pub tools: Vec<(StackColorRole, ProcessingChain)>,
    /// What continuum subtraction did for each emission line it was asked
    /// for, in words.
    pub continuum: Vec<(StackColorRole, String)>,
}

/// Continuum subtraction: its settings, and where each channel's stack
/// clipped, in the channel's own frame, as its stacker recorded it.
pub struct ChannelContinuum {
    pub config: crate::server::stack_preview::color::EmissionProcessing,
    pub clipping: BTreeMap<StackColorRole, LinearImage>,
}

/// The name of the channel that holds an emission line with the continuum
/// under it subtracted, in a store that keeps both.
pub fn line_channel(channel: &str) -> String {
    format!("{channel}@line")
}

impl RegisteredChannels {
    /// Reads each channel stack, deconvolves it and runs its processing
    /// tools when `tools` has them for it, corrects its background when
    /// `background` is given, and places every result on the first stack's
    /// grid.
    ///
    /// The steps and their order are the ones PSF Guard's color previews
    /// run: the code in `color_steps`. Registration is measured on the
    /// stacks as they came. The tools run on a stack as it came. Each
    /// background is fitted and corrected on the tools' result, in the
    /// channel's own frame, before resampling, so the empty border a
    /// resample leaves is never fitted.
    ///
    /// Every stack is held in memory while this runs, twice when the
    /// background is corrected. A mosaic too large for that needs these
    /// steps by tile.
    pub fn prepare(
        stacks: &[(StackColorRole, &Path)],
        tools: Option<&ChannelTools<'_>>,
        continuum: Option<&ChannelContinuum>,
        background: Option<&StackBackgroundExtraction>,
        gpu: &GpuOptions,
    ) -> Result<Self> {
        if stacks.is_empty() {
            bail!("a document needs at least one channel stack");
        }
        let frames = stacks
            .iter()
            .map(|(_, path)| open_mono(path))
            .collect::<Result<Vec<_>>>()?;
        let roles: Vec<StackColorRole> = stacks.iter().map(|(role, _)| *role).collect();
        let reference = (roles[0], frames[0].image.width, frames[0].image.height);

        let others: Vec<(StackColorRole, &LinearImage)> = roles
            .iter()
            .copied()
            .zip(frames.iter().map(|frame| &frame.image))
            .skip(1)
            .collect();
        let registrations =
            register_channels((roles[0], &frames[0].image), &others, gpu, |_, _| Ok(()))
                .map_err(anyhow::Error::msg)?;
        let mut placements = vec![ChannelPlacement {
            role: roles[0],
            registration: None,
        }];
        placements.extend(
            registrations
                .iter()
                .map(|(role, registration)| ChannelPlacement {
                    role: *role,
                    registration: Some(ChannelRegistration {
                        rms_error_pixels: registration.rms_error_pixels,
                        drift_pixels: registration.drift_pixels,
                        matched_stars: registration.matched_stars,
                    }),
                }),
        );
        // Places one channel's image on the reference grid.
        let place = |channel: usize, image: LinearImage| -> Result<Vec<f32>> {
            match channel.checked_sub(1) {
                None => Ok(image.data),
                Some(other) => {
                    let (role, registration) = &registrations[other];
                    Ok(
                        place_on_reference(*role, &image, reference, registration, gpu)
                            .map_err(anyhow::Error::msg)?
                            .data,
                    )
                }
            }
        };

        let channel_names = roles
            .iter()
            .map(|role| role_name(*role))
            .collect::<Result<Vec<_>, _>>()?;
        let mut names = channel_names.clone();
        let mut planes = Vec::new();
        for (channel, frame) in frames.iter().enumerate() {
            planes.push(place(channel, frame.image.clone())?);
        }

        // The tools run on each stack as it came, in the channel's own
        // frame. What follows them, the background step included, works on
        // their result.
        let mut processed: Vec<LinearImage> =
            frames.iter().map(|frame| frame.image.clone()).collect();
        let mut tool_runs = Vec::new();
        if let Some(tools) = tools {
            for (channel, (role, frame)) in roles.iter().zip(&frames).enumerate() {
                let label = channel_names[channel].clone();
                // A deconvolution runs first, and the chain runs on its
                // result. The two together are the channel's tools state.
                let mut source = std::borrow::Cow::Borrowed(frame);
                if let Some(deconvolution) = tools.deconvolutions.get(role) {
                    (tools.progress)(&label, "Seiza deconvolution", 0.0);
                    let (restored, _) = deconvolution
                        .apply(tools.cache, frame)
                        .map_err(|error| anyhow!("the deconvolution of {label}: {error}"))?;
                    source = std::borrow::Cow::Owned(seiza_stacking::FitsFrame {
                        image: restored,
                        headers: frame.headers.clone(),
                        ..frame.clone()
                    });
                }
                let Some(chain) = tools.chains.get(role) else {
                    if let std::borrow::Cow::Owned(restored) = source {
                        names.push(tools_channel(&label));
                        planes.push(place(channel, restored.image.clone())?);
                        processed[channel] = restored.image;
                    }
                    continue;
                };
                let frame = source.as_ref();
                let outcome = run_tool_chain(
                    RcAstroRunner {
                        workers: tools.workers,
                        person: true,
                    },
                    tools.cache,
                    chain,
                    &frame.image,
                    &frame.headers,
                    None,
                    |ToolEvent::Running {
                         label: tool,
                         fraction,
                     }| (tools.progress)(&label, tool, fraction),
                    &mut || Ok(()),
                )
                .map_err(|error| anyhow!("the tools of {label}: {error}"))?;
                names.push(tools_channel(&label));
                planes.push(place(channel, outcome.image.clone())?);
                if let Some(stars) = outcome.stars {
                    names.push(stars_channel(&label));
                    planes.push(place(channel, stars)?);
                }
                processed[channel] = outcome.image;
                tool_runs.push((*role, chain.config.clone()));
            }
        }

        // Each group's tools, on its registered channels together. The
        // results replace each channel's tools state and stars, already on
        // the reference grid.
        let mut on_grid: Vec<Option<LinearImage>> = vec![None; roles.len()];
        if let Some(tools) = tools {
            let (width, height) = (reference.1, reference.2);
            for group in &tools.groups {
                let members: Vec<usize> = group
                    .roles
                    .iter()
                    .filter_map(|role| roles.iter().position(|other| other == role))
                    .collect();
                if members.len() < 2 {
                    continue;
                }
                // Each member in its state after its own tools, or as stacked.
                let latest = |channel: usize| -> Result<LinearImage> {
                    let wanted = tools_channel(&channel_names[channel]);
                    let at = names
                        .iter()
                        .position(|other| *other == wanted)
                        .unwrap_or(channel);
                    Ok(LinearImage::new(width, height, 1, planes[at].clone())?)
                };
                let mut images = members
                    .iter()
                    .map(|channel| latest(*channel))
                    .collect::<Result<Vec<_>>>()?;
                // A color image has three channels: a group of two repeats
                // its last.
                while images.len() < 3 {
                    images.push(images[images.len() - 1].clone());
                }
                let combined = crate::server::stack_preview::color_calibration::interleave(
                    &images[0], &images[1], &images[2],
                )
                .map_err(anyhow::Error::msg)?;
                let outcome = run_tool_chain(
                    RcAstroRunner {
                        workers: tools.workers,
                        person: true,
                    },
                    tools.cache,
                    &group.chain,
                    &combined,
                    &frames[0].headers,
                    None,
                    |ToolEvent::Running {
                         label: tool,
                         fraction,
                     }| (tools.progress)(&group.name, tool, fraction),
                    &mut || Ok(()),
                )
                .map_err(|error| anyhow!("the tools of {}: {error}", group.name))?;
                let split = |image: &LinearImage, at: usize| -> Vec<f32> {
                    image.data.iter().skip(at).step_by(3).copied().collect()
                };
                let mut set = |name: String, samples: Vec<f32>| match names
                    .iter()
                    .position(|other| *other == name)
                {
                    Some(at) => planes[at] = samples,
                    None => {
                        names.push(name);
                        planes.push(samples);
                    }
                };
                for (at, channel) in members.iter().enumerate() {
                    let label = &channel_names[*channel];
                    let starless = split(&outcome.image, at);
                    on_grid[*channel] = Some(LinearImage::new(width, height, 1, starless.clone())?);
                    set(tools_channel(label), starless);
                    if let Some(stars) = &outcome.stars {
                        set(stars_channel(label), split(stars, at));
                    }
                    tool_runs.push((roles[*channel], group.chain.config.clone()));
                }
            }
        }

        let mut backgrounds = Vec::new();
        if let Some(extraction) = background {
            // Corrected in full: the strength is applied when the two
            // planes are blended.
            let full = StackBackgroundExtraction {
                strength: 1.0,
                ..extraction.clone()
            };
            // A channel a group's tools ran on is corrected on the grid.
            for (channel, image) in on_grid.iter().enumerate() {
                if let Some(image) = image {
                    processed[channel] = image.clone();
                }
            }
            let mut channels: Vec<(StackColorRole, &mut LinearImage)> =
                roles.iter().copied().zip(processed.iter_mut()).collect();
            // Catalog protection needs a plate solve of the stack, which
            // Studio does not read yet, so no region is protected.
            correct_channel_backgrounds(
                &mut channels,
                &full,
                |_| Vec::new(),
                |role, event| {
                    if let BackgroundEvent::Corrected { fit, .. } = event {
                        backgrounds.push((role, fit.clone()));
                    }
                    Ok(())
                },
            )
            .map_err(anyhow::Error::msg)?;
            for (channel, corrected) in processed.into_iter().enumerate() {
                names.push(background_corrected_channel(&channel_names[channel]));
                if on_grid[channel].is_some() {
                    planes.push(corrected.data);
                } else {
                    planes.push(place(channel, corrected)?);
                }
            }
        }

        // Continuum subtraction, with the steps PSF Guard's color previews
        // run: the continuum under each line is fitted on the stacks as
        // they came, which still have the stars the fit measures, and then
        // subtracted from the line in its last linear state.
        let mut continuum_results = Vec::new();
        if let Some(continuum) = continuum.filter(|continuum| {
            continuum.config.mode != crate::server::stack_preview::color::ContinuumMode::Off
        }) {
            use seiza_stacking::{
                ContinuumFitOutcome, ContinuumSource, ContinuumSources, ContinuumTransfer,
            };
            let (width, height) = (reference.1, reference.2);
            let image = |samples: Vec<f32>| LinearImage::new(width, height, 1, samples);
            for (line, role) in roles.iter().enumerate() {
                let Some(settings) = continuum.config.lines.get(role) else {
                    continue;
                };
                let wanted = if *role == StackColorRole::Oiii {
                    vec![StackColorRole::Green, StackColorRole::Blue]
                } else {
                    vec![StackColorRole::Red]
                };
                let Some(references) = wanted
                    .iter()
                    .map(|wanted| roles.iter().position(|other| other == wanted))
                    .collect::<Option<Vec<usize>>>()
                else {
                    continuum_results.push((
                        *role,
                        "not subtracted: the broadband channel under it is missing".to_string(),
                    ));
                    continue;
                };
                let inputs: Vec<usize> = std::iter::once(line).chain(references.clone()).collect();
                let masks: Vec<Option<Vec<u8>>> = inputs
                    .iter()
                    .map(|input| {
                        let map = continuum.clipping.get(&roles[*input])?;
                        let placed = place(*input, map.clone()).ok()?;
                        // A map of another size is not this stack's.
                        if placed.len() != width * height {
                            return None;
                        }
                        Some(
                            placed
                                .iter()
                                .map(|value| u8::from(!value.is_finite() || *value > 0.0))
                                .collect(),
                        )
                    })
                    .collect();
                let source = |input: usize, mask: &Option<Vec<u8>>| ContinuumSource {
                    name: channel_names[input].clone(),
                    revision: format!("studio:{}", channel_names[input]),
                    transfer: ContinuumTransfer::Linear,
                    saturation: continuum.config.saturation.get(&roles[input]).copied(),
                    clipping_revision: mask
                        .as_ref()
                        .map(|_| format!("clipping:{}", channel_names[input])),
                };
                let sources = ContinuumSources {
                    width,
                    height,
                    narrowband: source(line, &masks[0]),
                    references: references
                        .iter()
                        .zip(&masks[1..])
                        .map(|(input, mask)| source(*input, mask))
                        .collect(),
                };
                let stacked: Vec<LinearImage> = inputs
                    .iter()
                    .map(|input| image(planes[*input].clone()))
                    .collect::<Result<_, _>>()?;
                let borrowed: Vec<&LinearImage> = stacked[1..].iter().collect();
                let mask_slices: Vec<Option<&[u8]>> =
                    masks.iter().map(|mask| mask.as_deref()).collect();
                let outcome = crate::color_steps::fit_continuum_line(
                    &stacked[0],
                    &borrowed,
                    &sources,
                    &mask_slices,
                    &continuum.config,
                    settings,
                    role.label(),
                )
                .map_err(|error| anyhow!("the continuum of {}: {error}", role.label()))?;
                let plan = match outcome {
                    ContinuumFitOutcome::Fitted { plan } => plan,
                    ContinuumFitOutcome::Skipped { detail, .. } => {
                        continuum_results.push((*role, format!("not subtracted: {detail}")));
                        continue;
                    }
                };
                // Each input in its last linear state: with its background
                // corrected, else after its tools, else as stacked. The
                // inputs must agree on their stars: all starless, or all
                // with their stars, or the line keeps or loses the stars
                // of its references.
                let last_state = |input: usize| -> Option<LinearImage> {
                    let name = &channel_names[input];
                    [background_corrected_channel(name), tools_channel(name)]
                        .iter()
                        .find_map(|wanted| names.iter().position(|other| other == wanted))
                        .and_then(|at| image(planes[at].clone()).ok())
                };
                let starless: Vec<bool> = inputs
                    .iter()
                    .map(|input| {
                        let wanted = stars_channel(&channel_names[*input]);
                        names.contains(&wanted)
                    })
                    .collect();
                if starless.iter().any(|&one| one) && !starless.iter().all(|&one| one) {
                    continuum_results.push((
                        *role,
                        "not subtracted: remove the stars of the line and of its references alike"
                            .to_string(),
                    ));
                    continue;
                }
                let latest: Vec<LinearImage> = inputs
                    .iter()
                    .zip(&stacked)
                    .map(|(input, stacked)| last_state(*input).unwrap_or_else(|| stacked.clone()))
                    .collect();
                let (from, state) = if starless[0] {
                    (&latest, "starless")
                } else {
                    (&latest, "with its stars")
                };
                let from_references: Vec<&LinearImage> = from[1..].iter().collect();
                let residual =
                    seiza_stacking::subtract_continuum(&from[0], &from_references, &sources, &plan)
                        .map_err(|error| anyhow!("the continuum of {}: {error}", role.label()))?;
                let residual = residual.data;
                names.push(line_channel(&channel_names[line]));
                planes.push(residual);
                continuum_results.push((
                    *role,
                    format!(
                        "subtracted {state}, weights {}",
                        plan.coefficients
                            .iter()
                            .map(|weight| format!("{weight:.3}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
        }

        Ok(Self {
            shape: ImageShape {
                width: reference.1,
                height: reference.2,
                channels: planes.len(),
            },
            planes,
            names,
            placements,
            backgrounds,
            tools: tool_runs,
            continuum: continuum_results,
        })
    }
}

impl RegisteredChannels {
    /// One channel as stacked, on the reference grid.
    pub fn stacked(&self, role: StackColorRole) -> Option<LinearImage> {
        let name = role_name(role).ok()?;
        let plane = self.names.iter().position(|other| *other == name)?;
        LinearImage::new(
            self.shape.width,
            self.shape.height,
            1,
            self.planes[plane].clone(),
        )
        .ok()
    }

    /// One plane by its name, such as `red@tools`, on the reference grid.
    pub fn plane(&self, name: &str) -> Option<LinearImage> {
        let plane = self.names.iter().position(|other| other == name)?;
        LinearImage::new(
            self.shape.width,
            self.shape.height,
            1,
            self.planes[plane].clone(),
        )
        .ok()
    }

    /// Adds a plane under a new name. Returns whether `samples` is the
    /// grid's size and the name is free.
    pub fn add_plane(&mut self, name: &str, samples: Vec<f32>) -> bool {
        if samples.len() != self.shape.width * self.shape.height
            || self.names.iter().any(|other| other == name)
        {
            return false;
        }
        self.names.push(name.to_string());
        self.planes.push(samples);
        self.shape.channels = self.planes.len();
        true
    }

    /// Replaces one plane's samples. Returns whether the plane exists and
    /// `samples` is its size.
    pub fn replace_plane(&mut self, name: &str, samples: Vec<f32>) -> bool {
        let Some(plane) = self.names.iter().position(|other| other == name) else {
            return false;
        };
        if self.planes[plane].len() != samples.len() {
            return false;
        }
        self.planes[plane] = samples;
        true
    }

    /// Multiplies every state of one channel by `gain`: the channel as
    /// stacked, after its tools, its stars, and with its background
    /// corrected. Color calibration's gains are applied this way, so every
    /// state of red, green and blue keeps the same balance.
    pub fn scale_channel(&mut self, role: StackColorRole, gain: f32) {
        let Ok(name) = role_name(role) else {
            return;
        };
        let state = format!("{name}@");
        for (plane, plane_name) in self.planes.iter_mut().zip(&self.names) {
            if *plane_name == name || plane_name.starts_with(&state) {
                for sample in plane.iter_mut() {
                    *sample *= gain;
                }
            }
        }
    }
}

impl RegisteredChannels {
    /// Gives the sky of `channels` the level it has in `anchor`, in every
    /// state that still has a sky: as stacked, after the tools, and with
    /// the background corrected. A channel's stars have no sky and are
    /// left alone.
    ///
    /// Color calibration's gains scale each channel's sky with it, so
    /// without this the sky takes the color of the largest gain. The sky's
    /// level is the median of a sample of the plane.
    pub fn level_sky(&mut self, anchor: StackColorRole, channels: &[StackColorRole]) {
        let median = |plane: &[f32]| -> Option<f32> {
            let step = (plane.len() / 500_000).max(1);
            let mut sample: Vec<f32> = plane
                .iter()
                .step_by(step)
                .copied()
                .filter(|value| value.is_finite())
                .collect();
            if sample.is_empty() {
                return None;
            }
            let middle = sample.len() / 2;
            sample.select_nth_unstable_by(middle, f32::total_cmp);
            Some(sample[middle])
        };
        let Ok(anchor) = role_name(anchor) else {
            return;
        };
        for state in ["", "@tools", "@background"] {
            let plane_of = |name: &str| {
                let wanted = match state {
                    "" => name.to_string(),
                    "@tools" => tools_channel(name),
                    _ => background_corrected_channel(name),
                };
                self.names.iter().position(|other| *other == wanted)
            };
            // An anchor without this state, such as green with no tools,
            // lends its level as stacked.
            let anchor_plane =
                plane_of(&anchor).or_else(|| self.names.iter().position(|other| *other == anchor));
            let Some(level) = anchor_plane.and_then(|plane| median(&self.planes[plane])) else {
                continue;
            };
            for role in channels {
                let Ok(name) = role_name(*role) else { continue };
                let Some(plane) = plane_of(&name) else {
                    continue;
                };
                let Some(own) = median(&self.planes[plane]) else {
                    continue;
                };
                for sample in self.planes[plane].iter_mut() {
                    *sample += level - own;
                }
            }
        }
    }
}

/// The processing tools to run on channels, and where their results are
/// kept.
pub struct ChannelTools<'a> {
    /// The workers that run the tools: this machine's own, and any
    /// connected to it.
    pub workers: &'a Workers,
    /// Where each step's result is kept, so a repeat run is fast.
    pub cache: &'a Path,
    /// The chain for each channel that has one.
    pub chains: BTreeMap<StackColorRole, ToolChain>,
    /// The deconvolution of each channel that has one. It runs before the
    /// channel's chain.
    pub deconvolutions: BTreeMap<StackColorRole, crate::color_steps::ChannelDeconvolution>,
    /// Chains that run on a group of channels together, after each
    /// channel's own: on the group's registered channels combined into one
    /// color image, as PSF Guard's color tools run.
    pub groups: Vec<GroupTools>,
    /// Hears the channel, the tool running on it, and how far the channel's
    /// chain has got, from 0 to 1.
    pub progress: &'a dyn Fn(&str, &str, f32),
}

/// A chain that runs on a group of channels together.
pub struct GroupTools {
    /// The group's name, for progress.
    pub name: String,
    /// The group's channels, in the order the color image holds them.
    pub roles: Vec<StackColorRole>,
    pub chain: ToolChain,
}

fn open_mono(path: &Path) -> Result<seiza_stacking::FitsFrame> {
    let frame = crate::image_io::open_linear_frame(path)
        .with_context(|| format!("reading {}", path.display()))?;
    if frame.image.channels != 1 {
        bail!(
            "{} has {} channels; a channel stack has one",
            path.display(),
            frame.image.channels
        );
    }
    Ok(frame)
}

impl RegionReader for RegisteredChannels {
    fn shape(&self) -> ImageShape {
        self.shape
    }

    fn domain(&self) -> Domain {
        Domain::Linear
    }

    fn peak_bytes(&self, region: Region) -> u64 {
        (region.width as u64)
            .saturating_mul(region.height as u64)
            .saturating_mul(self.shape.channels as u64)
            .saturating_mul(size_of::<f32>() as u64)
    }

    fn read(&self, region: Region) -> Result<Vec<f32>, StudioError> {
        region.check(self.shape)?;
        let channels = self.shape.channels;
        let mut samples = vec![0.0; region.width * region.height * channels];
        for (channel, plane) in self.planes.iter().enumerate() {
            for row in 0..region.height {
                let start = (region.y + row) * self.shape.width + region.x;
                let source = &plane[start..start + region.width];
                let out = &mut samples[row * region.width * channels..];
                for (column, &value) in source.iter().enumerate() {
                    out[column * channels + channel] = value;
                }
            }
        }
        Ok(samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channels(width: usize, height: usize, planes: Vec<Vec<f32>>) -> RegisteredChannels {
        RegisteredChannels {
            shape: ImageShape {
                width,
                height,
                channels: planes.len(),
            },
            planes,
            names: Vec::new(),
            placements: Vec::new(),
            backgrounds: Vec::new(),
            tools: Vec::new(),
            continuum: Vec::new(),
        }
    }

    #[test]
    fn a_region_is_the_channels_interleaved() {
        // Two 3 x 2 channels: one counts up, one counts up from 100.
        let first: Vec<f32> = (0..6).map(|v| v as f32).collect();
        let second: Vec<f32> = (0..6).map(|v| 100.0 + v as f32).collect();
        let image = channels(3, 2, vec![first, second]);
        let whole = image.read(image.shape().region()).unwrap();
        assert_eq!(
            whole,
            [0.0, 100.0, 1.0, 101.0, 2.0, 102.0, 3.0, 103.0, 4.0, 104.0, 5.0, 105.0]
        );
        let corner = image
            .read(Region {
                x: 1,
                y: 1,
                width: 2,
                height: 1,
            })
            .unwrap();
        assert_eq!(corner, [4.0, 104.0, 5.0, 105.0]);
    }

    #[test]
    fn a_region_outside_the_image_is_refused() {
        let image = channels(3, 2, vec![vec![0.0; 6]]);
        let outside = Region {
            x: 2,
            y: 0,
            width: 2,
            height: 1,
        };
        assert!(image.read(outside).is_err());
    }
}
