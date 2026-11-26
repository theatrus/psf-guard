//! The steps of color processing, each as one function that PSF Guard's
//! color previews and PSF-Studio both call.
//!
//! A step here holds the order of the Seiza calls and the rules around
//! them. It knows nothing of jobs, caches or progress bars: a caller passes
//! a function that hears what the step is doing, and may stop it.
//!
//! Registration, the processing tools of one channel, the background step
//! and the measuring of color calibration gains are here. The rest of the color pipeline still lives in
//! `server::stack_preview::color` and moves here step by step.

use crate::astrometry::AstrometrySolutionResponse;
use crate::server::stack_preview::color::{
    ContinuumMode, EmissionLineSettings, EmissionProcessing, StackBackgroundExtraction,
    StackColorRole,
};
use crate::server::stack_preview::color_calibration::{
    calibrate_channels, interleave, StackColorCalibration, StackColorCalibrationOutcome,
    StackColorCalibrationSkip, StackColorCalibrationSkipReason, StarPlacement,
};
use crate::server::stack_preview::rc_astro::{self, ProcessingChain, RcAstroOutcome};
use crate::server::stack_preview::stretch::{StackDeconvolutionResult, StackStretchRequest};
use crate::server::stack_preview::StackSkyOrientation;
use crate::server::workers::rc_astro::RcAstroRunner;
use crate::server::workers::Workers;
use seiza_background::{BackgroundFit, FittedModel, ModelConfig, ProtectedRegion};
use seiza_stacking::{
    resample_to_reference_with_gpu, CancelSignal, ExternalToolSchema, GpuOptions, LinearImage,
    Registrar, RegistrationOptions, RegistrationResult,
};
use std::path::Path;

/// What a chain of processing tools is doing.
pub enum ToolEvent<'a> {
    /// A tool is running: its name as a person reads it, and how far the
    /// whole chain has got, from 0 to 1.
    Running { label: &'a str, fraction: f32 },
}

/// A chain of processing tools, ready to run on one image: the tools in
/// their order with their settings filled in, each tool's schema, and one
/// cache id per step.
pub struct ToolChain {
    pub config: ProcessingChain,
    pub schemas: Vec<(String, ExternalToolSchema)>,
    pub ids: Vec<String>,
}

impl ToolChain {
    /// Resolves a chain against the tools the workers offer. `scope`,
    /// `source` and `revision` say which image the chain runs on, so the
    /// cache keeps one result per image and per setting. `deconvolution` is
    /// the identity of a deconvolution that runs before the chain, if one
    /// does, so the chain's results follow its settings.
    ///
    /// Refuses a chain with a tool no worker offers.
    pub fn resolve(
        workers: &Workers,
        config: &ProcessingChain,
        scope: &str,
        source: &str,
        revision: &str,
        deconvolution: Option<&str>,
    ) -> Result<Self, String> {
        let schemas = rc_astro::schemas_for(workers, config)?;
        let config = rc_astro::normalize_against_schemas(config, &schemas)?;
        let ids =
            rc_astro::rc_astro_chain_ids(scope, source, revision, &config, &schemas, deconvolution)
                .map_err(|error| format!("{error:?}"))?;
        Ok(Self {
            config,
            schemas,
            ids,
        })
    }
}

/// Runs a chain of processing tools on one linear image: sharpening, noise
/// reduction and star removal, each by the provider the chain names
/// (RC-Astro's BlurXTerminator, NoiseXTerminator and StarXTerminator, or
/// Seiza's noise model).
///
/// Each step's result is kept under `cache`, so a chain that ran before
/// returns at once, and a chain that grew by one tool runs only that tool.
/// `before_step` is read before each tool starts; an error ends the chain
/// there. `cancel` stops the tool in hand.
///
/// Returns the processed image, and the stars when star removal kept them.
#[allow(clippy::too_many_arguments)]
pub fn run_tool_chain(
    runner: RcAstroRunner<'_>,
    cache: &Path,
    chain: &ToolChain,
    image: &LinearImage,
    headers: &[(String, seiza_fits::HeaderValue)],
    cancel: Option<&CancelSignal>,
    mut observe: impl FnMut(ToolEvent<'_>),
    before_step: &mut dyn FnMut() -> Result<(), String>,
) -> Result<RcAstroOutcome, String> {
    rc_astro::apply_rc_astro(
        runner,
        cache,
        &chain.ids,
        &chain.config,
        &chain.schemas,
        image,
        headers,
        &mut |label, fraction| observe(ToolEvent::Running { label, fraction }),
        cancel,
        before_step,
    )
}

/// The largest registration residual a channel may have, in pixels.
pub const MAX_REGISTRATION_RMS_PIXELS: f64 = 2.0;

/// What the registration step is doing to one channel.
pub enum RegistrationEvent<'a> {
    /// About to find the reference channel's stars.
    PreparingReference,
    /// The reference channel is ready.
    ReferenceReady,
    /// About to register the channel to the reference.
    Registering,
    /// The channel is registered, within the residual limit.
    Registered(&'a RegistrationResult),
}

/// Measures how each channel maps onto the reference channel's grid.
///
/// Registration is measured on the channels as they are given. A caller
/// registers the stacks as they came, before any tool changes them, so the
/// result does not depend on a tool's settings. A channel whose residual
/// passes [`MAX_REGISTRATION_RMS_PIXELS`] is refused.
///
/// `observe` hears each event; an error from it stops the step.
pub fn register_channels(
    reference: (StackColorRole, &LinearImage),
    others: &[(StackColorRole, &LinearImage)],
    gpu: &GpuOptions,
    mut observe: impl FnMut(StackColorRole, RegistrationEvent<'_>) -> Result<(), String>,
) -> Result<Vec<(StackColorRole, RegistrationResult)>, String> {
    let (reference_role, reference) = reference;
    observe(reference_role, RegistrationEvent::PreparingReference)?;
    let registrar = Registrar::new_with_gpu(reference, RegistrationOptions::default(), gpu)
        .map_err(|error| error.to_string())?;
    observe(reference_role, RegistrationEvent::ReferenceReady)?;
    let mut registered = Vec::with_capacity(others.len());
    for (role, image) in others {
        let role = *role;
        observe(role, RegistrationEvent::Registering)?;
        let registration = registrar.register(image).map_err(|error| {
            format!(
                "Failed to register {} to {}: {error}",
                role.label(),
                reference_role.label()
            )
        })?;
        if registration.rms_error_pixels > MAX_REGISTRATION_RMS_PIXELS {
            return Err(format!(
                "{} registration RMS {:.3}px exceeds {:.3}px",
                role.label(),
                registration.rms_error_pixels,
                MAX_REGISTRATION_RMS_PIXELS
            ));
        }
        observe(role, RegistrationEvent::Registered(&registration))?;
        registered.push((role, registration));
    }
    Ok(registered)
}

/// Places a registered channel on the reference channel's grid.
pub fn place_on_reference(
    role: StackColorRole,
    image: &LinearImage,
    reference: (StackColorRole, usize, usize),
    registration: &RegistrationResult,
    gpu: &GpuOptions,
) -> Result<LinearImage, String> {
    let (reference_role, width, height) = reference;
    resample_to_reference_with_gpu(image, width, height, registration.transform, gpu).map_err(
        |error| {
            format!(
                "Failed to resample {} onto the {} reference: {error}",
                role.label(),
                reference_role.label()
            )
        },
    )
}

/// What the background step is doing to one channel.
pub enum BackgroundEvent<'a> {
    /// About to fit the channel.
    Fitting,
    /// The fit with protected regions failed for the reason given, and is
    /// tried again without them.
    RetryingWithoutProtection(&'a str),
    /// The channel is fitted.
    Fitted,
    /// About to refit the channel with the model chosen for every channel.
    Refitting,
    /// About to correct the channel.
    Correcting,
    /// The channel is corrected with `fit`. `fallback` is why its protected
    /// fit failed, when it did.
    Corrected {
        fit: &'a BackgroundFit,
        fallback: Option<&'a str>,
    },
}

/// Fits and corrects the background of each channel, in place.
///
/// Each channel is fitted in its own frame. Automatic model selection runs
/// per channel, and left alone it can curve one channel and leave another
/// flat, which tints the corners. So when there is more than one channel,
/// Seiza chooses one model for all of them and any channel fitted with
/// another is refitted.
///
/// `regions` gives the regions to keep out of a channel's samples.
/// `observe` hears each event; an error from it stops the step.
pub fn correct_channel_backgrounds(
    channels: &mut [(StackColorRole, &mut LinearImage)],
    extraction: &StackBackgroundExtraction,
    regions: impl Fn(StackColorRole) -> Vec<ProtectedRegion>,
    mut observe: impl FnMut(StackColorRole, BackgroundEvent<'_>) -> Result<(), String>,
) -> Result<(), String> {
    let mut fitted = Vec::with_capacity(channels.len());
    for (role, image) in channels.iter() {
        let role = *role;
        observe(role, BackgroundEvent::Fitting)?;
        let mut retried = Ok(());
        let (fit, fallback) = fit_channel_background(image, extraction, regions(role), |error| {
            retried = observe(role, BackgroundEvent::RetryingWithoutProtection(error));
        })
        .map_err(|error| format!("Failed to fit {} background: {error}", role.label()))?;
        retried?;
        observe(role, BackgroundEvent::Fitted)?;
        fitted.push((fit, fallback));
    }

    let fits: Vec<&BackgroundFit> = fitted.iter().map(|(fit, _)| fit).collect();
    let shared = if fits.len() > 1 {
        seiza_background::select_shared_model(&extraction.config.model, &fits)
    } else {
        None
    };

    for ((role, image), (mut fit, fallback)) in channels.iter_mut().zip(fitted) {
        let role = *role;
        if let Some(shared) = shared.as_ref()
            && !fit_uses_model(&fit, shared)
        {
            observe(role, BackgroundEvent::Refitting)?;
            fit = refit_channel_background(image, extraction, regions(role), &fit, shared)
                .map_err(|error| format!("Failed to refit {} background: {error}", role.label()))?;
        }
        observe(role, BackgroundEvent::Correcting)?;
        fit.correct_in_place_with_strength(
            &mut image.data,
            extraction.correction_mode,
            extraction.strength,
        )
        .map_err(|error| format!("Failed to correct {} background: {error}", role.label()))?;
        observe(
            role,
            BackgroundEvent::Corrected {
                fit: &fit,
                fallback: fallback.as_deref(),
            },
        )?;
    }
    Ok(())
}

pub(crate) fn fit_channel_background(
    image: &LinearImage,
    extraction: &StackBackgroundExtraction,
    protected_regions: Vec<ProtectedRegion>,
    mut on_fallback: impl FnMut(&str),
) -> Result<(BackgroundFit, Option<String>), seiza_background::Error> {
    let mut config = extraction.config.clone();
    if extraction.protect_catalog_emission {
        config.protected_regions = protected_regions;
    } else {
        config.protected_regions.clear();
    }
    let protected_fit = !config.protected_regions.is_empty();
    match seiza_background::fit_background(
        &image.data,
        image.width,
        image.height,
        image.channels,
        &config,
    ) {
        Ok(fit) => Ok((fit, None)),
        Err(error) if protected_fit => {
            let reason = error.to_string();
            on_fallback(&reason);
            config.protected_regions.clear();
            seiza_background::fit_background(
                &image.data,
                image.width,
                image.height,
                image.channels,
                &config,
            )
            .map(|fit| (fit, Some(reason)))
        }
        Err(error) => Err(error),
    }
}

/// Whether `fit` already carries the surface `model` names.
pub(crate) fn fit_uses_model(fit: &BackgroundFit, model: &ModelConfig) -> bool {
    match (&fit.model, model) {
        (
            FittedModel::Polynomial { degree, .. },
            ModelConfig::Polynomial { degree: wanted, .. },
        ) => degree == wanted,
        (FittedModel::RadialBasis { .. }, ModelConfig::RadialBasis { .. }) => true,
        _ => false,
    }
}

/// Refit one channel with the model chosen for every channel.
///
/// The refit keeps whatever catalog protection the first fit ended up with,
/// and keeps its candidate scores so the manifest still shows why the shared
/// model won.
pub(crate) fn refit_channel_background(
    image: &LinearImage,
    extraction: &StackBackgroundExtraction,
    protected_regions: Vec<ProtectedRegion>,
    first: &BackgroundFit,
    model: &ModelConfig,
) -> Result<BackgroundFit, seiza_background::Error> {
    let mut config = extraction.config.clone();
    config.model = model.clone();
    config.protected_regions = if first.diagnostics.protected_regions > 0 {
        protected_regions
    } else {
        Vec::new()
    };
    let mut fit = seiza_background::fit_background(
        &image.data,
        image.width,
        image.height,
        image.channels,
        &config,
    )?;
    if let Some(mut selection) = first.diagnostics.model_selection.clone() {
        selection.selected = match &fit.model {
            FittedModel::Polynomial { degree, .. } => format!("polynomial_{degree}"),
            _ => "radial_basis".into(),
        };
        fit.diagnostics.model_selection = Some(selection);
    }
    Ok(fit)
}

/// Deconvolves one channel with Seiza: the step PSF Guard's channel cards
/// and color previews run on an input before its processing tools.
pub fn deconvolve(
    image: &LinearImage,
    config: seiza_deconvolution::DeconvolutionConfig,
) -> Result<(LinearImage, StackDeconvolutionResult), String> {
    crate::server::stack_preview::stretch::apply_deconvolution(image, config)
}

/// One channel's deconvolution, with the identity its result is kept
/// under: the channel's stack and the settings.
pub struct ChannelDeconvolution {
    pub config: seiza_deconvolution::DeconvolutionConfig,
    pub id: String,
}

impl ChannelDeconvolution {
    /// The deconvolution of the stack `source` at `revision`, kept under
    /// `scope`, as [`ToolChain::resolve`] keys a chain.
    pub fn resolve(
        config: seiza_deconvolution::DeconvolutionConfig,
        scope: &str,
        source: &str,
        revision: &str,
    ) -> Result<Self, String> {
        let id = crate::server::stack_preview::stretch::deconvolution_cache_id(
            scope, source, revision, &config,
        )
        .map_err(|error| format!("{error:?}"))?;
        Ok(Self { config, id })
    }

    /// Deconvolves `frame`, or reads the result kept under `cache` from an
    /// earlier run with the same stack and settings. Returns the result and
    /// whether it was read rather than computed.
    pub fn apply(
        &self,
        cache: &Path,
        frame: &seiza_stacking::FitsFrame,
    ) -> Result<(LinearImage, bool), String> {
        use crate::server::stack_preview::stretch;
        if let Some((kept, _)) = stretch::cached_deconvolution(cache, &self.id, self.config)? {
            return Ok((kept.image, true));
        }
        let (restored, result) = deconvolve(&frame.image, self.config)?;
        stretch::store_deconvolution(cache, &self.id, &restored, &frame.headers, &result)?;
        Ok((restored, false))
    }
}

/// Fits the continuum under one emission line from broadband references:
/// the step PSF Guard's color previews run before they subtract it.
///
/// `line` and `references` are linear and on one grid, and still have
/// their stars, which the fit measures. `masks` has one entry per input,
/// the line first: the pixels its stack clipped, one byte each, or `None`
/// where the stack kept no map. Every input needs a map or an explicit
/// saturation level in `config`; without them the fit is skipped, because
/// a clipped star would bias it. In manual mode the settings' weights are
/// the plan, and the fit only supplies flux ratios it lacks.
#[allow(clippy::too_many_arguments)]
pub fn fit_continuum_line(
    line: &LinearImage,
    references: &[&LinearImage],
    sources: &seiza_stacking::ContinuumSources,
    masks: &[Option<&[u8]>],
    config: &EmissionProcessing,
    settings: &EmissionLineSettings,
    label: &str,
) -> Result<seiza_stacking::ContinuumFitOutcome, String> {
    use seiza_stacking::{ContinuumFitOutcome, ContinuumOrigin, ContinuumPlan};
    let pixels = sources.width * sources.height;
    let have_evidence = std::iter::once(&sources.narrowband)
        .chain(&sources.references)
        .all(|source| source.saturation.is_some() || source.clipping_revision.is_some());
    let measured = if have_evidence {
        let filled: Vec<Vec<u8>> = masks
            .iter()
            .map(|mask| mask.map_or_else(|| vec![0; pixels], <[u8]>::to_vec))
            .collect();
        let borrowed: Vec<&[u8]> = filled.iter().map(Vec::as_slice).collect();
        seiza_stacking::measure_and_fit_continuum_masked(
            line,
            references,
            sources,
            &borrowed,
            &[],
            &config.fit,
            &config.photometry,
        )
        .map_err(|error| error.to_string())?
    } else {
        ContinuumFitOutcome::Skipped {
            reason: seiza_stacking::ContinuumSkipReason::UnknownSaturation,
            detail: "Producer clipping maps or explicit physical working-unit limits are required for every input".into(),
            diagnostics: Default::default(),
        }
    };
    if config.mode != ContinuumMode::Manual {
        return Ok(measured);
    }
    if settings.coefficients.len() != references.len() {
        return Err(format!(
            "Enter {} manual continuum weights for {label}",
            references.len()
        ));
    }
    let ratios = if !settings.reference_flux_ratios.is_empty() {
        settings.reference_flux_ratios.clone()
    } else {
        match &measured {
            ContinuumFitOutcome::Fitted { plan } => plan.reference_flux_ratios.clone(),
            ContinuumFitOutcome::Skipped { diagnostics, .. } => {
                diagnostics.reference_flux_ratios.clone()
            }
        }
    };
    Ok(ContinuumFitOutcome::Fitted {
        plan: ContinuumPlan {
            schema_version: 1,
            psf_matching: None,
            sources: sources.clone(),
            coefficients: settings.coefficients.clone(),
            reference_flux_ratios: ratios,
            origin: ContinuumOrigin::Manual,
            diagnostics: None,
        },
    })
}

/// What [`measure_color_gains`] found.
pub struct ColorGains {
    /// What the calibration did, with its evidence, or why it did not run.
    pub outcome: StackColorCalibrationOutcome,
    /// The multipliers of red, green and blue that calibrate them, when
    /// the calibration ran. Green's is 1.
    pub gains: Option<[f32; 3]>,
}

/// Measures the gains that calibrate registered red, green and blue
/// channels against Gaia: the step PSF Guard's color previews run, without
/// its sky offsets.
///
/// `solution` is the plate solve of the reference frame of the stack whose
/// grid the channels are on, and `orientation` that stack's sky layout.
/// The channels are measured as given and not changed, so a caller applies
/// the gains to every state of a channel it holds: as stacked, after its
/// tools, its stars, and with its background corrected. The channels
/// measured must still have their stars.
pub fn measure_color_gains(
    channels: [&LinearImage; 3],
    solution: &AstrometrySolutionResponse,
    orientation: &StackSkyOrientation,
    catalog: &seiza::catalog::PhotometryCatalog,
    settings: &StackColorCalibration,
) -> ColorGains {
    let skipped = |detail: String| ColorGains {
        outcome: StackColorCalibrationOutcome::Skipped(StackColorCalibrationSkip::new(
            StackColorCalibrationSkipReason::Failed,
            detail,
        )),
        gains: None,
    };
    if (channels[0].width, channels[0].height)
        != (orientation.output_width, orientation.output_height)
    {
        return skipped("the stack's sky layout does not match its pixels".into());
    }
    let placement = StarPlacement {
        wcs: crate::astrometry::wcs_from_response(&solution.wcs),
        orientation: orientation.clone(),
    };
    let wcs = match placement.wcs_on(None) {
        Ok(wcs) => wcs,
        Err(error) => return skipped(error),
    };
    // Gains alone: a sky offset belongs to one state of a channel, and the
    // background step already levels the sky of the corrected state.
    let settings = StackColorCalibration {
        neutralize_background: false,
        ..*settings
    };
    let [mut red, mut green, mut blue] = channels.map(Clone::clone);
    let outcome = calibrate_channels(
        [&mut red, &mut green, &mut blue],
        &wcs,
        catalog,
        &settings,
        None,
    );
    let gains = matches!(outcome, StackColorCalibrationOutcome::Calibrated(_)).then(|| {
        let ratio = |before: &LinearImage, after: &LinearImage| -> f32 {
            let (mut was, mut is) = (0.0f64, 0.0f64);
            for (a, b) in before.data.iter().zip(&after.data) {
                if a.is_finite() && b.is_finite() {
                    was += f64::from(*a);
                    is += f64::from(*b);
                }
            }
            if was.abs() > f64::EPSILON {
                (is / was) as f32
            } else {
                1.0
            }
        };
        let measured = [
            ratio(channels[0], &red),
            ratio(channels[1], &green),
            ratio(channels[2], &blue),
        ];
        // Green is the anchor, whatever common scale Seiza applied.
        let green = if measured[1].abs() > f32::EPSILON {
            measured[1]
        } else {
            1.0
        };
        measured.map(|gain| gain / green)
    });
    ColorGains { outcome, gains }
}

/// Stretches the stars of calibrated red, green and blue into one display
/// layer, with Seiza's star stretch: the step PSF Guard's RGB stars use.
///
/// `original` holds the three channels with their stars and `starless` the
/// same channels after star removal, both linear and with the same
/// calibration. `amount` is from 0 to 8. The result is each channel's stars
/// from 0 to 1, ready to screen over a picture; the three share one scale,
/// so the stars keep their colors.
pub fn stretch_star_layer(
    original: [&LinearImage; 3],
    starless: [&LinearImage; 3],
    amount: f64,
) -> Result<[Vec<f32>; 3], String> {
    let original = interleave(original[0], original[1], original[2])?;
    let starless = interleave(starless[0], starless[1], starless[2])?;
    let layer = seiza_stacking::stretch_rgb_stars(&original, &starless, amount)
        .map_err(|error| error.to_string())?;
    let pixels = layer.data.len() / 3;
    let mut channels = [
        Vec::with_capacity(pixels),
        Vec::with_capacity(pixels),
        Vec::with_capacity(pixels),
    ];
    for pixel in layer.data.as_chunks::<3>().0 {
        for (channel, value) in channels.iter_mut().zip(pixel) {
            channel.push(*value);
        }
    }
    Ok(channels)
}

/// Runs a chain of stretches over a whole image, with Seiza's stretch
/// stack, and returns the image stretched for display. A chain with the
/// layered stretch needs this: that stretch blurs the image, so it cannot
/// be resolved from samples. `image` has one channel, or three that are
/// stretched together. Its samples are physical values, as a stack holds
/// them.
pub fn stretch_for_display(
    image: &LinearImage,
    stages: &[StackStretchRequest],
) -> Result<LinearImage, String> {
    use seiza_stretch::{SampleDomain, SampleNormalization};

    // A stack holds physical values. They are brought to the range of 0
    // to 1 first. Seiza resolves one linked range over every channel and
    // skips samples that are not finite, so three channels keep their
    // balance.
    let domain = SampleDomain::PhysicalLinear {
        normalization: SampleNormalization::default(),
    }
    .resolve(&image.data, image.channels)
    .map_err(|error| error.to_string())?;
    let unit = domain
        .apply(&image.data, image.channels)
        .map_err(|error| error.to_string())?;
    let stack =
        seiza_stretch::StretchStack::new(stages.iter().map(StackStretchRequest::config).collect())
            .map_err(|error| error.to_string())?;
    let shape = seiza_stretch::ImageShape::new(image.width, image.height, image.channels);
    let output = stack
        .apply_f32_image(&unit, shape)
        .map_err(|error| error.to_string())?;
    LinearImage::new(image.width, image.height, image.channels, output.data)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use seiza_background::{BackgroundConfig, CorrectionMode};
    use seiza_stacking::SimilarityTransform;

    const WIDTH: usize = 96;
    const HEIGHT: usize = 64;

    /// A channel with a curved background and a deterministic ripple that
    /// stands in for sky noise.
    fn channel(curve: f32) -> LinearImage {
        let data = (0..WIDTH * HEIGHT)
            .map(|index| {
                let (x, y) = (index % WIDTH, index / WIDTH);
                let nx = x as f32 / (WIDTH - 1) as f32 * 2.0 - 1.0;
                let ny = y as f32 / (HEIGHT - 1) as f32 * 2.0 - 1.0;
                0.2 - curve * (nx * nx + ny * ny) + ((x * 7 + y * 13) % 11) as f32 * 2.0e-4
            })
            .collect();
        LinearImage::new(WIDTH, HEIGHT, 1, data).unwrap()
    }

    fn extraction(mode: CorrectionMode, strength: f64) -> StackBackgroundExtraction {
        StackBackgroundExtraction {
            config: BackgroundConfig {
                model: ModelConfig::Automatic {
                    max_degree: 2,
                    ridge: 1.0e-8,
                    rbf_smoothing: 0.01,
                    max_control_points: 192,
                    allow_radial_basis: false,
                    minimum_improvement: 0.08,
                },
                samples_per_axis: 12,
                sample_radius: Some(1),
                search_steps: 0,
                ..BackgroundConfig::default()
            },
            correction_mode: mode,
            strength,
            protect_catalog_emission: false,
        }
    }

    /// Two channels corrected at `strength` and placed on another grid.
    fn corrected_and_placed(mode: CorrectionMode, strength: f64) -> Vec<Vec<f32>> {
        let mut images = [channel(0.05), channel(0.02)];
        let [first, second] = &mut images;
        let mut channels = [
            (StackColorRole::Red, first),
            (StackColorRole::Green, second),
        ];
        correct_channel_backgrounds(
            &mut channels,
            &extraction(mode, strength),
            |_| Vec::new(),
            |_, _| Ok(()),
        )
        .unwrap();
        let transform = SimilarityTransform {
            scale: 1.0,
            rotation_radians: 0.01,
            translation_x: 3.4,
            translation_y: -2.2,
        };
        images
            .iter()
            .map(|image| {
                resample_to_reference_with_gpu(image, WIDTH, HEIGHT, transform, &GpuOptions::cpu())
                    .unwrap()
                    .data
            })
            .collect()
    }

    /// Studio moves the strength by blending the uncorrected channel with
    /// the fully corrected one. That equals correcting at the strength,
    /// because the correction changes each sample in proportion to the
    /// strength and resampling keeps proportions.
    #[test]
    fn a_strength_between_is_a_blend_of_uncorrected_and_fully_corrected() {
        for mode in [CorrectionMode::Subtract, CorrectionMode::Divide] {
            let before = corrected_and_placed(mode, 0.0);
            let after = corrected_and_placed(mode, 1.0);
            // The correction did something to blend.
            let moved = before[0]
                .iter()
                .zip(&after[0])
                .any(|(a, b)| a.is_finite() && b.is_finite() && (a - b).abs() > 1.0e-3);
            assert!(moved, "{mode:?}");
            for strength in [0.25_f32, 0.7] {
                let direct = corrected_and_placed(mode, f64::from(strength));
                for channel in 0..2 {
                    let mut compared = 0;
                    for ((a, b), d) in before[channel]
                        .iter()
                        .zip(&after[channel])
                        .zip(&direct[channel])
                    {
                        if !(a.is_finite() && b.is_finite() && d.is_finite()) {
                            continue;
                        }
                        let blended = a + (b - a) * strength;
                        assert!(
                            (blended - d).abs() < 2.0e-6,
                            "{mode:?} at {strength}: blended {blended}, corrected {d}"
                        );
                        compared += 1;
                    }
                    assert!(compared > WIDTH * HEIGHT / 2);
                }
            }
        }
    }

    #[test]
    fn an_observer_can_stop_a_step() {
        let mut image = channel(0.05);
        let untouched = image.clone();
        let stopped = correct_channel_backgrounds(
            &mut [(StackColorRole::Red, &mut image)],
            &extraction(CorrectionMode::Subtract, 1.0),
            |_| Vec::new(),
            |_, event| match event {
                BackgroundEvent::Correcting => Err("stopped".to_string()),
                _ => Ok(()),
            },
        );
        assert_eq!(stopped, Err("stopped".to_string()));
        assert_eq!(image.data, untouched.data);
    }
}
