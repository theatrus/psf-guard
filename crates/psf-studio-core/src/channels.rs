//! Channel stacks on one grid: registered onto the first, with the
//! document's Linear steps run on them in order, ready for a tile store.
//!
//! PSF Guard prepares a Studio picture with this, and PSF Studio does too
//! when it opens stack files itself. What runs the processing tools is a
//! [`LinearTools`]: PSF Guard's workers, or none.

use crate::color::{
    correct_channel_backgrounds, fit_continuum_line, interleave, place_on_reference,
    register_channels, role_name, BackgroundEvent, ContinuumMode, EmissionProcessing,
    StackBackgroundExtraction, StackColorRole,
};
use crate::image_steps::{Domain, ImageShape, Region};
use crate::studio::{stars_channel, step_channel, RegionReader, StudioError};
use anyhow::{anyhow, bail, Context, Result};
use seiza_background::BackgroundFit;
use seiza_stacking::{GpuOptions, LinearImage};
use std::collections::BTreeMap;
use std::path::Path;

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

/// One state a store keeps of a channel: the channel after one Linear step
/// that changed it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelState {
    /// The step, by its id in the document.
    pub step: uuid::Uuid,
    /// The plane that holds the channel after the step ([`step_channel`]).
    pub plane: String,
    /// Whether the plane is a background correction in full, which the
    /// canvas blends into the state before it by the step's strength.
    pub correction: bool,
}

/// Channel stacks on one grid: a linear image with one channel per plane.
///
/// The planes are, in order: every stack as it came; then, for each Linear
/// step that changed a channel, in the order the steps ran, the channel
/// after it ([`step_channel`]), with the stars a star removal took out
/// ([`stars_channel`]) and a line with its continuum subtracted. A
/// background extraction's plane is the channel corrected in full, and a
/// strength between is a blend with the state before it, because Seiza's
/// correction changes each sample in proportion to the strength and
/// resampling keeps proportions. So a person can move the strength and see
/// the result at once, with no refit. A step after it took the channel at
/// the step's strength.
///
/// The channels stay as separate planes, as Seiza gave them. A region is
/// interleaved when it is read, so the channels are never copied whole.
pub struct RegisteredChannels {
    shape: ImageShape,
    planes: Vec<Vec<f32>>,
    /// A name for each plane: the channel's role, and for a channel's
    /// other planes, the channel's name with what the plane holds.
    pub names: Vec<String>,
    pub placements: Vec<ChannelPlacement>,
    /// Each channel's fitted background, for each background extraction.
    pub backgrounds: Vec<(StackColorRole, BackgroundFit)>,
    /// The processing tools that ran on each channel, one per tool step,
    /// by name.
    pub tools: Vec<(StackColorRole, String)>,
    /// What continuum subtraction did for each emission line it was asked
    /// for, in words.
    pub continuum: Vec<(StackColorRole, String)>,
    /// Each channel's states after the Linear steps that changed it, in
    /// the order they ran, by the channel's name.
    pub states: BTreeMap<String, Vec<ChannelState>>,
    /// For each channel whose stars a tool took out, the plane of the
    /// channel just after that tool: the image the stars add back to.
    pub starless: BTreeMap<String, String>,
}

/// Continuum subtraction: its settings, and where each channel's stack
/// clipped, in the channel's own frame, as its stacker recorded it.
pub struct ChannelContinuum {
    pub config: EmissionProcessing,
    pub clipping: BTreeMap<StackColorRole, LinearImage>,
}

/// The name of the channel that holds an emission line with the continuum
/// under it subtracted, in a store that keeps both.
pub fn line_channel(channel: &str) -> String {
    format!("{channel}@line")
}

/// One Linear step that changes the samples of the channels it is for, as
/// preparing a store runs them: in the document's order. `T` is one
/// processing tool with its settings, as the [`LinearTools`] that run it
/// read it.
pub enum LinearStep<T> {
    /// One processing tool: on one channel, or on a color group's channels
    /// combined into one color image.
    Tool {
        step: uuid::Uuid,
        roles: Vec<StackColorRole>,
        /// What progress calls the channels: an input's name or a group's.
        label: String,
        tool: T,
        /// Whether the tool runs a second time on the channel shifted by
        /// [`SHIFTED_PASS`] pixels, and the two results are averaged, so
        /// the seams between the tiles BlurXTerminator works in fall in
        /// different places and blur out.
        shifted: bool,
    },
    /// Seiza's deconvolution of one channel.
    Deconvolution {
        step: uuid::Uuid,
        role: StackColorRole,
        config: seiza_deconvolution::DeconvolutionConfig,
    },
    /// A background extraction of the channels given, fitted with one model.
    Background {
        step: uuid::Uuid,
        roles: Vec<StackColorRole>,
        extraction: StackBackgroundExtraction,
    },
}

impl<T> LinearStep<T> {
    fn roles(&self) -> &[StackColorRole] {
        match self {
            LinearStep::Tool { roles, .. } | LinearStep::Background { roles, .. } => roles,
            LinearStep::Deconvolution { role, .. } => std::slice::from_ref(role),
        }
    }
}

/// Runs a document's processing tools and deconvolutions for
/// [`RegisteredChannels::prepare`], and keeps their results so a repeat run
/// is fast. PSF Guard runs them on its workers.
pub trait LinearTools {
    /// One processing tool with its settings.
    type Tool: Clone;

    /// `chain` with `tool` after it, when one chain can run them all: each
    /// tool does something later in a chain's order than the one before
    /// it, as a chain runs its tools in that order.
    fn extend_chain(&self, chain: &[Self::Tool], tool: &Self::Tool) -> Option<Vec<Self::Tool>>;

    /// The tool's name, as [`RegisteredChannels::tools`] records it.
    fn name(&self, tool: &Self::Tool) -> String;

    /// Runs `run`'s chain of tools on its image. `progress` hears the tool
    /// running and how far the chain has got, from 0 to 1.
    fn run_chain(
        &self,
        run: &ChainRun<'_, Self::Tool>,
        progress: &dyn Fn(&str, f32),
    ) -> Result<ChainResult>;

    /// Deconvolves one channel's frame with Seiza. Returns the image and
    /// the identity its result is kept under.
    fn deconvolve(
        &self,
        config: seiza_deconvolution::DeconvolutionConfig,
        source: &str,
        revision: &str,
        frame: &seiza_stacking::FitsFrame,
    ) -> Result<(LinearImage, String)>;
}

/// One chain of tools to run on one image.
pub struct ChainRun<'a, T> {
    /// The tools, in the order they run.
    pub chain: &'a [T],
    /// The stack the image comes from, and its revision: what the result
    /// is kept by.
    pub source: &'a str,
    pub revision: &'a str,
    /// The deconvolution that made the image, when one did.
    pub deconvolution: Option<&'a str>,
    pub image: &'a LinearImage,
    /// The stack's FITS headers.
    pub headers: &'a [(String, seiza_fits::HeaderValue)],
}

/// What a chain of tools made.
pub struct ChainResult {
    pub image: LinearImage,
    /// The stars a star removal took out.
    pub stars: Option<LinearImage>,
    /// The identity each tool's result is kept under, in the chain's order.
    pub ids: Vec<String>,
}

/// The Linear steps to run on the channels, and what runs their tools.
pub struct LinearProgram<'a, R: LinearTools> {
    /// What runs the tools and deconvolutions. With none, tool steps and
    /// deconvolutions are passed over, and the other steps run.
    pub tools: Option<&'a R>,
    /// Each stack's source and revision, which the results are kept by.
    pub sources: BTreeMap<StackColorRole, (String, String)>,
    /// The steps, in order.
    pub steps: Vec<LinearStep<R::Tool>>,
    /// Hears the channel, the tool running on it, and how far the tool's
    /// chain has got, from 0 to 1.
    pub progress: &'a dyn Fn(&str, &str, f32),
}

/// How far one channel has got through a [`LinearProgram`], and what its
/// samples are, for the cache.
///
/// Tool steps that follow one another in a chain's order on one channel
/// are run as one chain, as PSF Guard runs a channel's chain: each tool's
/// result is kept by the chain so far, so the results of a document saved
/// before each tool was a step are found again.
struct Track<T> {
    /// The channel's samples now, in its own frame or on the reference
    /// grid.
    image: LinearImage,
    on_grid: bool,
    /// What `start` is, for the cache: the stack's revision, or the id of
    /// the step that made it.
    revision: String,
    /// A deconvolution that ran on the image `revision` names and gave
    /// `start`.
    deconvolution: Option<String>,
    /// The samples the chain in hand starts from.
    start: LinearImage,
    /// The chain in hand: the tools run on `start` so far.
    chain: Vec<T>,
    /// The id of the last step that ran since `revision`.
    last: Option<String>,
}

impl<T> Track<T> {
    fn new(image: LinearImage, revision: &str) -> Self {
        Self {
            start: image.clone(),
            image,
            on_grid: false,
            revision: revision.to_string(),
            deconvolution: None,
            chain: Vec::new(),
            last: None,
        }
    }

    /// Starts again from the samples as they are now: what ran since
    /// `revision` becomes the new revision.
    fn settle(&mut self) {
        if let Some(last) = self.last.take() {
            self.revision = last;
        }
        self.deconvolution = None;
        self.chain.clear();
        self.start = self.image.clone();
    }

    /// The samples are now `image`, made by the step `id` from the samples
    /// as they were.
    fn replace(&mut self, image: LinearImage, id: String) {
        self.image = image;
        self.settle();
        self.revision = id;
    }

    /// How a color group's revision names this channel: its revision and
    /// the last step since, as PSF Guard's group tools named it.
    fn group_part(&self) -> String {
        format!("{}:{}", self.revision, self.last.as_deref().unwrap_or(""))
    }
}

/// How far a tool's shifted second pass first moves the channel, down and
/// right, in pixels: about half the spacing of the seams BlurXTerminator
/// leaves between its tiles. [`shifted_passes`] measures the spacing from
/// the two passes and moves by half of it when this is far from that.
pub const SHIFTED_PASS: usize = 192;

/// `image` moved `by` pixels right and down on a canvas that much larger,
/// with the edges it uncovers filled by mirroring the image.
pub fn shift_mirrored(image: &LinearImage, by: [usize; 2]) -> Result<LinearImage> {
    let (width, height, channels) = (image.width, image.height, image.channels);
    let mirror = |at: isize, size: usize| -> usize {
        let size = size as isize;
        let mut at = at;
        if at < 0 {
            at = -at - 1;
        }
        if at >= size {
            at = 2 * size - at - 1;
        }
        at.clamp(0, size - 1) as usize
    };
    let [across, down] = by;
    let (out_width, out_height) = (width + across, height + down);
    let mut data = Vec::with_capacity(out_width * out_height * channels);
    for y in 0..out_height {
        let row = mirror(y as isize - down as isize, height);
        for x in 0..out_width {
            let at = (row * width + mirror(x as isize - across as isize, width)) * channels;
            data.extend_from_slice(&image.data[at..at + channels]);
        }
    }
    Ok(LinearImage::new(out_width, out_height, channels, data)?)
}

/// The part of `image` that [`shift_mirrored`] moved: `width` by `height`
/// from `by` pixels in, right and down, in each channel.
pub fn unshift(
    image: &LinearImage,
    by: [usize; 2],
    width: usize,
    height: usize,
) -> Result<LinearImage> {
    let channels = image.channels;
    let [across, down] = by;
    if image.width < width + across || image.height < height + down {
        bail!("a shifted pass came back smaller than it went");
    }
    let mut data = Vec::with_capacity(width * height * channels);
    for y in 0..height {
        let start = ((y + down) * image.width + across) * channels;
        data.extend_from_slice(&image.data[start..start + width * channels]);
    }
    Ok(LinearImage::new(width, height, channels, data)?)
}

/// What one pass of a tool made: its image, and the stars a star removal
/// took out.
pub struct Pass {
    pub image: LinearImage,
    pub stars: Option<LinearImage>,
}

/// The seams' spacing BlurXTerminator leaves between its tiles, across and
/// down, measured from two passes of it on one image shifted apart: the
/// difference holds the seams of both, and the seams of one pass repeat at
/// the tiles' spacing. `None` along an axis where no spacing stands out.
pub fn seam_spacing(a: &LinearImage, b: &LinearImage) -> [Option<usize>; 2] {
    if a.data.len() != b.data.len() || a.width < 64 || a.height < 64 {
        return [None, None];
    }
    let (width, height, channels) = (a.width, a.height, a.channels);
    // The difference's size at each column, and at each row.
    let mut across = vec![0.0_f64; width];
    let mut down = vec![0.0_f64; height];
    for (y, row) in down.iter_mut().enumerate() {
        for (x, column) in across.iter_mut().enumerate() {
            let at = (y * width + x) * channels;
            let difference = (f64::from(a.data[at]) - f64::from(b.data[at])).abs();
            if difference.is_finite() {
                *column += difference;
                *row += difference;
            }
        }
    }
    let spacing = |profile: &[f64]| -> Option<usize> {
        // Each value less the mean around it, so the seams stand out of the
        // picture's slow changes.
        let reach = 24;
        let length = profile.len();
        let detail: Vec<f64> = (0..length)
            .map(|at| {
                let low = at.saturating_sub(reach);
                let high = (at + reach + 1).min(length);
                let mean = profile[low..high].iter().sum::<f64>() / (high - low) as f64;
                profile[at] - mean
            })
            .collect();
        // Tiles are a few hundred pixels apart.
        let (shortest, longest) = (128, 1024.min(length / 2));
        if longest <= shortest {
            return None;
        }
        let correlation = |lag: usize| -> f64 {
            detail[..length - lag]
                .iter()
                .zip(&detail[lag..])
                .map(|(a, b)| a * b)
                .sum::<f64>()
                / (length - lag) as f64
        };
        let scores: Vec<(usize, f64)> = (shortest..=longest)
            .map(|lag| (lag, correlation(lag)))
            .collect();
        let (_, best) = scores.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1))?;
        // A multiple of the spacing correlates as well as the spacing: the
        // shortest lag that scores nearly as high is the spacing, at the
        // top of its own peak.
        let first = scores.iter().position(|(_, score)| *score >= 0.8 * best)?;
        let (lag, _) = scores[first..]
            .iter()
            .copied()
            .take_while(|(_, score)| *score >= 0.5 * best)
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        // It must stand well above the lags around it.
        let mut others: Vec<f64> = scores.iter().map(|(_, score)| score.abs()).collect();
        others.sort_unstable_by(f64::total_cmp);
        let typical = others[others.len() / 2];
        (best > 4.0 * typical && best > 0.0).then_some(lag)
    };
    [spacing(&across), spacing(&down)]
}

/// A tool's result averaged with a second pass on the image shifted, so the
/// seams between BlurXTerminator's tiles fall in different places and
/// average out. `run` runs the tool on an image, named with a suffix for
/// its cache. The second pass is shifted by [`SHIFTED_PASS`]; the seams are
/// measured from the two, and when half their spacing is more than 24
/// pixels from that shift, a third pass shifted by half their spacing on
/// each axis is averaged with the first in its place.
pub fn shifted_passes(
    start: &LinearImage,
    first: Pass,
    run: &mut dyn FnMut(&LinearImage, &str) -> Result<Pass>,
) -> Result<Pass> {
    let (width, height) = (start.width, start.height);
    let pass =
        |run: &mut dyn FnMut(&LinearImage, &str) -> Result<Pass>, by: [usize; 2]| -> Result<Pass> {
            let suffix = if by[0] == by[1] {
                format!("shifted:{}", by[0])
            } else {
                format!("shifted:{}x{}", by[0], by[1])
            };
            let moved = run(&shift_mirrored(start, by)?, &suffix)?;
            Ok(Pass {
                image: unshift(&moved.image, by, width, height)?,
                stars: moved
                    .stars
                    .map(|stars| unshift(&stars, by, width, height))
                    .transpose()?,
            })
        };
    let second = pass(run, [SHIFTED_PASS; 2])?;
    let spacing = seam_spacing(&first.image, &second.image);
    let matched = spacing.map(|spacing| spacing.map(|spacing| spacing / 2).unwrap_or(SHIFTED_PASS));
    let off = matched.iter().any(|half| half.abs_diff(SHIFTED_PASS) > 24);
    let second = if off { pass(run, matched)? } else { second };
    Ok(Pass {
        image: average(&first.image, &second.image)?,
        stars: match (first.stars, second.stars) {
            (Some(first), Some(second)) => Some(average(&first, &second)?),
            (first, _) => first,
        },
    })
}

/// The mean of two images of one size.
pub fn average(a: &LinearImage, b: &LinearImage) -> Result<LinearImage> {
    if a.data.len() != b.data.len() {
        bail!("the two passes are of different sizes");
    }
    let data = a
        .data
        .iter()
        .zip(&b.data)
        .map(|(a, b)| (a + b) * 0.5)
        .collect();
    Ok(LinearImage::new(a.width, a.height, a.channels, data)?)
}

/// The chain of tools a color group runs together, while no other step
/// changes the group's channels.
struct GroupTrack<T> {
    roles: Vec<StackColorRole>,
    source: String,
    revision: String,
    start: LinearImage,
    chain: Vec<T>,
}

fn short_digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl RegisteredChannels {
    /// Reads each channel stack, runs the program's steps on the channels
    /// in order, and places every result on the first stack's grid.
    ///
    /// The steps are the ones PSF Guard's color previews run: the code in
    /// `color_steps`. Registration is measured on the stacks as they came.
    /// A step runs on a channel in its own frame, before resampling, so the
    /// empty border a resample leaves is never fitted; a color group's tool
    /// runs on its channels registered, and what follows it for those
    /// channels runs on the grid.
    ///
    /// Every stack is held in memory while this runs, with each state it
    /// passes through. A mosaic too large for that needs these steps by
    /// tile.
    pub fn prepare<R: LinearTools>(
        stacks: &[(StackColorRole, &Path)],
        program: Option<&LinearProgram<'_, R>>,
        continuum: Option<&ChannelContinuum>,
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
        let (width, height) = (reference.1, reference.2);

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
        // Adds a plane, or replaces one of the same name.
        let set = |names: &mut Vec<String>,
                   planes: &mut Vec<Vec<f32>>,
                   name: String,
                   samples: Vec<f32>| {
            match names.iter().position(|other| *other == name) {
                Some(at) => planes[at] = samples,
                None => {
                    names.push(name);
                    planes.push(samples);
                }
            }
        };

        let mut tracks: Vec<Track<R::Tool>> = frames
            .iter()
            .zip(&roles)
            .map(|(frame, role)| {
                let revision = program
                    .and_then(|program| program.sources.get(role))
                    .map_or("", |(_, revision)| revision.as_str());
                Track::new(frame.image.clone(), revision)
            })
            .collect();
        let mut states: BTreeMap<String, Vec<ChannelState>> = BTreeMap::new();
        let mut starless = BTreeMap::new();
        let mut backgrounds = Vec::new();
        let mut tool_runs = Vec::new();
        let mut group: Option<GroupTrack<R::Tool>> = None;
        let position = |role: &StackColorRole| roles.iter().position(|other| other == role);

        for step in program.map_or(&[][..], |program| program.steps.as_slice()) {
            let program = program.expect("steps come with their program");
            let members: Vec<usize> = step.roles().iter().filter_map(position).collect();
            if members.is_empty() {
                continue;
            }
            // A step on a group's channels ends the group's chain, unless
            // it is that group's next tool.
            let continues_group = matches!(
                (step, &group),
                (LinearStep::Tool { roles: wanted, .. }, Some(track))
                    if wanted.len() > 1 && *wanted == track.roles
            );
            if !continues_group
                && group
                    .as_ref()
                    .is_some_and(|track| track.roles.iter().any(|role| step.roles().contains(role)))
            {
                group = None;
            }
            match step {
                LinearStep::Tool {
                    step: id,
                    roles: wanted,
                    label,
                    tool,
                    shifted,
                } if wanted.len() == 1 => {
                    let Some(tools) = program.tools else {
                        continue;
                    };
                    let channel = members[0];
                    let track = &mut tracks[channel];
                    // A tool with a shifted second pass runs alone: its
                    // result is the average of two runs.
                    let chain = match tools.extend_chain(&track.chain, tool).filter(|_| !*shifted) {
                        Some(chain) => chain,
                        None => {
                            track.settle();
                            vec![tool.clone()]
                        }
                    };
                    let source = &program.sources[&roles[channel]].0;
                    let mut outcome = tools
                        .run_chain(
                            &ChainRun {
                                chain: &chain,
                                source,
                                revision: &track.revision,
                                deconvolution: track.deconvolution.as_deref(),
                                image: &track.start,
                                headers: &frames[channel].headers,
                            },
                            &|running, fraction| (program.progress)(label, running, fraction),
                        )
                        .map_err(|error| anyhow!("the tools of {label}: {error}"))?;
                    if *shifted {
                        // More passes, on the channel shifted down and right,
                        // mirrored at its edges, each kept by its own id.
                        let first = Pass {
                            image: outcome.image,
                            stars: outcome.stars,
                        };
                        let both = shifted_passes(&track.start, first, &mut |image, suffix| {
                            let revision = format!("{}:{suffix}", track.revision);
                            let moved = tools
                                .run_chain(
                                    &ChainRun {
                                        chain: &chain,
                                        source,
                                        revision: &revision,
                                        deconvolution: track.deconvolution.as_deref(),
                                        image,
                                        headers: &frames[channel].headers,
                                    },
                                    &|running, fraction| {
                                        (program.progress)(
                                            label,
                                            &format!("{running}, shifted"),
                                            fraction,
                                        )
                                    },
                                )
                                .map_err(|error| anyhow!("the tools of {label}: {error}"))?;
                            Ok(Pass {
                                image: moved.image,
                                stars: moved.stars,
                            })
                        })?;
                        outcome.image = both.image;
                        outcome.stars = both.stars;
                    }
                    if *shifted {
                        // The average is no chain's result: what follows
                        // starts from it.
                        let id = outcome.ids.last().cloned().unwrap_or_default();
                        track.replace(outcome.image, format!("{id}:shifted"));
                    } else {
                        track.chain = chain;
                        track.last = outcome.ids.last().cloned();
                        track.image = outcome.image;
                    }
                    let name = &channel_names[channel];
                    let plane = step_channel(name, *id);
                    let samples = if track.on_grid {
                        track.image.data.clone()
                    } else {
                        place(channel, track.image.clone())?
                    };
                    set(&mut names, &mut planes, plane.clone(), samples);
                    if let Some(stars) = outcome.stars {
                        let stars = if track.on_grid {
                            stars.data
                        } else {
                            place(channel, stars)?
                        };
                        set(&mut names, &mut planes, stars_channel(name), stars);
                        starless.insert(name.clone(), plane.clone());
                    }
                    states.entry(name.clone()).or_default().push(ChannelState {
                        step: *id,
                        plane,
                        correction: false,
                    });
                    tool_runs.push((roles[channel], tools.name(tool)));
                }
                LinearStep::Tool {
                    step: id,
                    roles: wanted,
                    label,
                    tool,
                    ..
                } => {
                    let Some(tools) = program.tools else {
                        continue;
                    };
                    if members.len() < 2 {
                        continue;
                    }
                    // The group's chain in hand, or a new one on the group's
                    // channels as they are now, registered.
                    let extended = group
                        .as_ref()
                        .filter(|track| track.roles == *wanted)
                        .and_then(|track| tools.extend_chain(&track.chain, tool));
                    let track = match (extended, group.take()) {
                        (Some(chain), Some(mut track)) => {
                            track.chain = chain;
                            track
                        }
                        _ => {
                            let mut images = Vec::new();
                            for channel in &members {
                                let track = &tracks[*channel];
                                images.push(if track.on_grid {
                                    track.image.clone()
                                } else {
                                    LinearImage::new(
                                        width,
                                        height,
                                        1,
                                        place(*channel, track.image.clone())?,
                                    )?
                                });
                            }
                            // A color image has three channels: a group of
                            // two repeats its last.
                            while images.len() < 3 {
                                images.push(images[images.len() - 1].clone());
                            }
                            let start = interleave(&images[0], &images[1], &images[2])
                                .map_err(anyhow::Error::msg)?;
                            let source = format!(
                                "{label}:{}",
                                members
                                    .iter()
                                    .map(|channel| program.sources[&roles[*channel]].0.as_str())
                                    .map(|source| source.trim_start_matches("mono:"))
                                    .collect::<Vec<_>>()
                                    .join("+")
                            );
                            let revision = members
                                .iter()
                                .map(|channel| tracks[*channel].group_part())
                                .collect::<Vec<_>>()
                                .join("+");
                            GroupTrack {
                                roles: wanted.clone(),
                                source,
                                revision,
                                start,
                                chain: vec![tool.clone()],
                            }
                        }
                    };
                    let outcome = tools
                        .run_chain(
                            &ChainRun {
                                chain: &track.chain,
                                source: &track.source,
                                revision: &track.revision,
                                deconvolution: None,
                                image: &track.start,
                                headers: &frames[members[0]].headers,
                            },
                            &|running, fraction| (program.progress)(label, running, fraction),
                        )
                        .map_err(|error| anyhow!("the tools of {label}: {error}"))?;
                    let split = |image: &LinearImage, at: usize| -> Vec<f32> {
                        image.data.iter().skip(at).step_by(3).copied().collect()
                    };
                    let id_now = outcome.ids.last().cloned().unwrap_or_default();
                    for (at, channel) in members.iter().enumerate() {
                        let name = &channel_names[*channel];
                        let samples = split(&outcome.image, at);
                        let plane = step_channel(name, *id);
                        set(&mut names, &mut planes, plane.clone(), samples.clone());
                        if let Some(stars) = &outcome.stars {
                            set(
                                &mut names,
                                &mut planes,
                                stars_channel(name),
                                split(stars, at),
                            );
                            starless.insert(name.clone(), plane.clone());
                        }
                        let member = &mut tracks[*channel];
                        member
                            .replace(LinearImage::new(width, height, 1, samples)?, id_now.clone());
                        member.on_grid = true;
                        states.entry(name.clone()).or_default().push(ChannelState {
                            step: *id,
                            plane,
                            correction: false,
                        });
                        tool_runs.push((roles[*channel], tools.name(tool)));
                    }
                    group = Some(track);
                }
                LinearStep::Deconvolution {
                    step: id,
                    role,
                    config,
                } => {
                    let Some(tools) = program.tools else {
                        continue;
                    };
                    let channel = members[0];
                    let label = channel_names[channel].clone();
                    let track = &mut tracks[channel];
                    if !track.chain.is_empty() || track.deconvolution.is_some() {
                        track.settle();
                    }
                    let source = &program.sources[role].0;
                    (program.progress)(&label, "Seiza deconvolution", 0.0);
                    let frame = seiza_stacking::FitsFrame {
                        image: track.image.clone(),
                        ..frames[channel].clone()
                    };
                    let (restored, kept) = tools
                        .deconvolve(*config, source, &track.revision, &frame)
                        .map_err(|error| anyhow!("the deconvolution of {label}: {error}"))?;
                    track.image = restored.clone();
                    track.start = restored;
                    track.chain.clear();
                    track.deconvolution = Some(kept.clone());
                    track.last = Some(kept);
                    let plane = step_channel(&label, *id);
                    let samples = if track.on_grid {
                        track.image.data.clone()
                    } else {
                        place(channel, track.image.clone())?
                    };
                    set(&mut names, &mut planes, plane.clone(), samples);
                    states.entry(label).or_default().push(ChannelState {
                        step: *id,
                        plane,
                        correction: false,
                    });
                }
                LinearStep::Background {
                    step: id,
                    extraction,
                    ..
                } => {
                    // Corrected in full: the strength is applied when the
                    // states are blended.
                    let full = StackBackgroundExtraction {
                        strength: 1.0,
                        ..extraction.clone()
                    };
                    let strength = extraction.strength as f32;
                    // What the correction depends on: its settings and
                    // every channel it fits, as they are now.
                    let key = short_digest(&format!(
                        "{}|{}",
                        serde_json::to_string(extraction)?,
                        members
                            .iter()
                            .map(|channel| tracks[*channel].group_part())
                            .collect::<Vec<_>>()
                            .join("+")
                    ));
                    let mut corrected: Vec<LinearImage> = members
                        .iter()
                        .map(|channel| tracks[*channel].image.clone())
                        .collect();
                    let member_roles: Vec<StackColorRole> =
                        members.iter().map(|channel| roles[*channel]).collect();
                    let mut channels: Vec<(StackColorRole, &mut LinearImage)> = member_roles
                        .iter()
                        .copied()
                        .zip(corrected.iter_mut())
                        .collect();
                    // Catalog protection needs a plate solve of the stack,
                    // which Studio does not read yet, so no region is
                    // protected.
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
                    for (channel, corrected) in members.iter().zip(corrected) {
                        let name = channel_names[*channel].clone();
                        let track = &mut tracks[*channel];
                        let plane = step_channel(&name, *id);
                        let samples = if track.on_grid {
                            corrected.data.clone()
                        } else {
                            place(*channel, corrected.clone())?
                        };
                        set(&mut names, &mut planes, plane.clone(), samples);
                        let blended = blend(&track.image, &corrected, strength)?;
                        track.replace(blended, format!("background:{key}"));
                        states.entry(name).or_default().push(ChannelState {
                            step: *id,
                            plane,
                            correction: true,
                        });
                    }
                }
            }
        }

        let mut prepared = Self {
            shape: ImageShape {
                width,
                height,
                channels: planes.len(),
            },
            planes,
            names,
            placements,
            backgrounds,
            tools: tool_runs,
            continuum: Vec::new(),
            states,
            starless,
        };
        if let Some(continuum) = continuum {
            prepared.subtract_continuum(continuum, &roles, &channel_names, &|channel, image| {
                place(channel, image)
            })?;
        }
        prepared.shape.channels = prepared.planes.len();
        Ok(prepared)
    }

    /// Continuum subtraction, with the steps PSF Guard's color previews
    /// run: the continuum under each line is fitted on the stacks as they
    /// came, which still have the stars the fit measures, and then
    /// subtracted from the line in its last linear state.
    fn subtract_continuum(
        &mut self,
        continuum: &ChannelContinuum,
        roles: &[StackColorRole],
        channel_names: &[String],
        place: &dyn Fn(usize, LinearImage) -> Result<Vec<f32>>,
    ) -> Result<()> {
        use seiza_stacking::{
            ContinuumFitOutcome, ContinuumSource, ContinuumSources, ContinuumTransfer,
        };
        if continuum.config.mode == ContinuumMode::Off {
            return Ok(());
        }
        let (width, height) = (self.shape.width, self.shape.height);
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
                self.continuum.push((
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
                .map(|input| image(self.planes[*input].clone()))
                .collect::<Result<_, _>>()?;
            let borrowed: Vec<&LinearImage> = stacked[1..].iter().collect();
            let mask_slices: Vec<Option<&[u8]>> =
                masks.iter().map(|mask| mask.as_deref()).collect();
            // The inputs must agree on their stars: all starless, or all
            // with their stars, or the line keeps or loses the stars of its
            // references.
            let starless: Vec<bool> = inputs
                .iter()
                .map(|input| self.names.contains(&stars_channel(&channel_names[*input])))
                .collect();
            if starless.iter().any(|&one| one) && !starless.iter().all(|&one| one) {
                self.continuum.push((
                    *role,
                    "not subtracted: remove the stars of the line and of its references alike"
                        .to_string(),
                ));
                continue;
            }
            // The fit measures the stars of the stacks either way. When the
            // line is subtracted starless, no star can keep a core or halo,
            // so Seiza's check of the stars' profiles is reported and does
            // not refuse the fit.
            let mut config = continuum.config.clone();
            if starless[0] {
                config.photometry.target = seiza_stacking::ContinuumTarget::Starless;
            }
            let outcome = fit_continuum_line(
                &stacked[0],
                &borrowed,
                &sources,
                &mask_slices,
                &config,
                settings,
                role.label(),
            )
            .map_err(|error| anyhow!("the continuum of {}: {error}", role.label()))?;
            let plan = match outcome {
                ContinuumFitOutcome::Fitted { plan } => plan,
                ContinuumFitOutcome::Skipped { detail, .. } => {
                    self.continuum
                        .push((*role, format!("not subtracted: {detail}")));
                    continue;
                }
            };
            // Each input in its last linear state.
            let latest: Vec<LinearImage> = inputs
                .iter()
                .zip(&stacked)
                .map(|(input, stacked)| {
                    self.last_state(&channel_names[*input])
                        .unwrap_or_else(|| stacked.clone())
                })
                .collect();
            let state = if starless[0] {
                "starless"
            } else {
                "with its stars"
            };
            let from_references: Vec<&LinearImage> = latest[1..].iter().collect();
            let residual =
                seiza_stacking::subtract_continuum(&latest[0], &from_references, &sources, &plan)
                    .map_err(|error| anyhow!("the continuum of {}: {error}", role.label()))?;
            self.names.push(line_channel(&channel_names[line]));
            self.planes.push(residual.data);
            self.continuum.push((
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
        Ok(())
    }

    /// One channel in its last linear state, on the reference grid: after
    /// the last Linear step that changed it, with a background correction
    /// there blended in by `strength`; as stacked when no step changed it.
    /// `None` for a channel the store does not hold.
    pub fn last_state_at(&self, channel: &str, strength: f32) -> Option<LinearImage> {
        let states = self.states.get(channel);
        let Some(last) = states.and_then(|states| states.last()) else {
            return self.plane(channel);
        };
        let after = self.plane(&last.plane)?;
        if !last.correction || strength >= 1.0 {
            return Some(after);
        }
        let states = states.expect("a last state has its list");
        let before = match states.len().checked_sub(2) {
            Some(at) => self.plane(&states[at].plane)?,
            None => self.plane(channel)?,
        };
        blend(&before, &after, strength).ok()
    }

    /// One channel in its last linear state, with a last background
    /// correction in full.
    pub fn last_state(&self, channel: &str) -> Option<LinearImage> {
        self.last_state_at(channel, 1.0)
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
    /// state that still has a sky: as stacked, and after each Linear step.
    /// A channel's stars have no sky and are left alone.
    ///
    /// Color calibration's gains scale each channel's sky with it, so
    /// without this the sky takes the color of the largest gain. The sky's
    /// level is the median of a sample of the plane. A state is levelled
    /// with the anchor after the same step; an anchor that the step did not
    /// change lends its level as stacked.
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
        let plane_of = |this: &Self, name: &str| this.names.iter().position(|other| other == name);
        let anchor_after = |this: &Self, step: Option<uuid::Uuid>| -> String {
            step.and_then(|step| {
                this.states
                    .get(&anchor)?
                    .iter()
                    .find(|state| state.step == step)
                    .map(|state| state.plane.clone())
            })
            .unwrap_or_else(|| anchor.clone())
        };
        for role in channels {
            let Ok(name) = role_name(*role) else { continue };
            let mut wanted: Vec<(String, Option<uuid::Uuid>)> = vec![(name.clone(), None)];
            wanted.extend(
                self.states
                    .get(&name)
                    .into_iter()
                    .flatten()
                    .map(|state| (state.plane.clone(), Some(state.step))),
            );
            for (plane_name, step) in wanted {
                let Some(level) = plane_of(self, &anchor_after(self, step))
                    .and_then(|plane| median(&self.planes[plane]))
                else {
                    continue;
                };
                let Some(plane) = plane_of(self, &plane_name) else {
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

/// `stacked` blended toward `corrected` by `strength`: Seiza's background
/// correction at that strength, which changes each sample in proportion.
fn blend(stacked: &LinearImage, corrected: &LinearImage, strength: f32) -> Result<LinearImage> {
    if strength >= 1.0 {
        return Ok(corrected.clone());
    }
    let data = stacked
        .data
        .iter()
        .zip(&corrected.data)
        .map(|(a, b)| a + (b - a) * strength)
        .collect();
    Ok(LinearImage::new(
        stacked.width,
        stacked.height,
        stacked.channels,
        data,
    )?)
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
            states: BTreeMap::new(),
            starless: BTreeMap::new(),
        }
    }

    #[test]
    fn a_shifted_pass_comes_back_to_where_it_was() {
        let image = LinearImage::new(5, 4, 1, (0..20).map(|value| value as f32).collect()).unwrap();
        let moved = shift_mirrored(&image, [2, 2]).unwrap();
        assert_eq!((moved.width, moved.height), (7, 6));
        // The image sits 2 pixels down and right; the edges uncovered
        // mirror it.
        assert_eq!(moved.data[2 * 7 + 2], 0.0);
        assert_eq!(moved.data[2 * 7], 1.0);
        assert_eq!(moved.data[0], 6.0);
        assert_eq!(unshift(&moved, [2, 2], 5, 4).unwrap().data, image.data);
        // Shifted by different amounts across and down.
        let apart = shift_mirrored(&image, [3, 1]).unwrap();
        assert_eq!((apart.width, apart.height), (8, 5));
        assert_eq!(unshift(&apart, [3, 1], 5, 4).unwrap().data, image.data);
        let doubled =
            LinearImage::new(5, 4, 1, image.data.iter().map(|v| v * 3.0).collect()).unwrap();
        assert_eq!(average(&image, &doubled).unwrap().data[3], 6.0);
    }

    /// Seams 446 pixels apart across and 348 down, as BlurXTerminator left
    /// on the Cat's Eye, are found in the difference of two passes.
    #[test]
    fn the_seams_spacing_is_measured_from_two_passes() {
        let (width, height) = (2400, 1800);
        let seams = |offset: usize| -> LinearImage {
            let data = (0..width * height)
                .map(|at| {
                    let (x, y) = (at % width, at / width);
                    let smooth = (x as f32 / width as f32) * 0.3 + (y as f32 / height as f32) * 0.2;
                    let seam = |position: usize, spacing: usize| {
                        let phase = (position + offset) % spacing;
                        if phase < 3 {
                            0.002
                        } else {
                            0.0
                        }
                    };
                    smooth + seam(x, 446) + seam(y, 348)
                })
                .collect();
            LinearImage::new(width, height, 1, data).unwrap()
        };
        let [across, down] = seam_spacing(&seams(0), &seams(192));
        assert_eq!(across, Some(446));
        assert_eq!(down, Some(348));
        // Two passes alike have no seams to tell apart.
        assert_eq!(seam_spacing(&seams(0), &seams(0)), [None, None]);
    }

    #[test]
    fn a_track_is_kept_by_what_ran_on_it() {
        // What a chain started after a deconvolution, or after another
        // chain, is kept by.
        let image = LinearImage::new(1, 1, 1, vec![0.5]).unwrap();
        let mut track: Track<&str> = Track::new(image.clone(), "stack-1");
        track.deconvolution = Some("deconvolved".to_string());
        track.last = Some("deconvolved".to_string());
        assert_eq!(track.group_part(), "stack-1:deconvolved");
        track.chain = vec!["nxt"];
        track.last = Some("after-nxt".to_string());
        track.settle();
        assert_eq!(track.revision, "after-nxt");
        assert!(track.deconvolution.is_none() && track.chain.is_empty());
        assert_eq!(track.group_part(), "after-nxt:");
        track.replace(image, "background:1".to_string());
        assert_eq!(track.revision, "background:1");
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
