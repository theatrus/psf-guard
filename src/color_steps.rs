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
use crate::server::stack_preview::color_calibration::{
    calibrate_channels, interleave, StackColorCalibration, StackColorCalibrationOutcome,
    StackColorCalibrationSkip, StackColorCalibrationSkipReason, StarPlacement,
};
use crate::server::stack_preview::rc_astro::{self, ProcessingChain, RcAstroOutcome};
use crate::server::stack_preview::stretch::StackDeconvolutionResult;
use crate::server::stack_preview::StackSkyOrientation;
use crate::server::workers::rc_astro::RcAstroRunner;
use crate::server::workers::Workers;
use seiza_stacking::{CancelSignal, ExternalToolSchema, LinearImage};
use std::path::Path;
// Registration, background extraction and display stretches are shared
// with PSF Studio.
pub use psf_studio_core::color::{
    correct_channel_backgrounds, fit_channel_background, fit_continuum_line, fit_uses_model,
    place_on_reference, refit_channel_background, register_channels, stretch_for_display, unscreen,
    BackgroundEvent, DisplayStretch, RegistrationEvent, MAX_REGISTRATION_RMS_PIXELS,
};

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
    /// A star removal always saves the stars it removes, linear, so a
    /// picture can add them back as a layer of their own.
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
        let mut config = config.clone();
        for step in &mut config.steps {
            if step.category() == Some(crate::remote_work::steps::Category::StarRemoval)
                && let Some((_, schema)) = schemas.iter().find(|(tool, _)| *tool == step.provider)
            {
                rc_astro::keep_linear_stars(step, schema)?;
            }
        }
        let config = rc_astro::normalize_against_schemas(&config, &schemas)?;
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
    let layer = seiza_compose::stretch_rgb_stars(&original, &starless, amount)
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
