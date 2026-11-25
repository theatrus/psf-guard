//! The steps of color processing, each as one function that PSF Guard's
//! color previews and PSF-Studio both call.
//!
//! A step here holds the order of the Seiza calls and the rules around
//! them. It knows nothing of jobs, caches or progress bars: a caller passes
//! a function that hears what the step is doing, and may stop it.
//!
//! The background step is the first to move here. The rest of the color
//! pipeline still lives in `server::stack_preview::color` and moves here
//! step by step.

use crate::server::stack_preview::color::{StackBackgroundExtraction, StackColorRole};
use seiza_background::{BackgroundFit, FittedModel, ModelConfig, ProtectedRegion};
use seiza_stacking::LinearImage;

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
