//! The stretches of the combined picture: PSF Guard's output stretches, as
//! the canvas draws them.
//!
//! An output stretch is one of Seiza's stretches of the whole picture after
//! the layers are combined. Seiza resolves each one into a plan, which the
//! picture's Finish steps carry; Seiza's compositor applies it, on the GPU
//! for the canvas and on the CPU for an export.
//!
//! A stretch with explicit settings, such as a generalized hyperbolic
//! stretch, resolves without the picture. One that Seiza resolves from the
//! picture's statistics, such as the automatic midtones stretch, needs
//! samples of the picture as the stretches before it left it. The layered
//! stretch needs the whole image, so the canvas does not draw it.

use seiza_stretch::{ColorStrategy, StretchConfig, StretchModel, StretchPlan};
use std::borrow::Cow;

/// The samples Seiza analyses per stretch, at most.
const ANALYSIS_SAMPLES: usize = 300_000;

/// Why the canvas cannot draw an output stretch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotDrawn {
    /// The layered stretch needs the whole image, not samples of it.
    NeedsImage,
    /// The stretch is resolved from the picture, and the canvas has no
    /// samples of the picture yet.
    NeedsSamples,
    /// The stretch has other layers above it and is resolved from samples
    /// of the layers beneath it, which the canvas does not have yet.
    NeedsSamplesBeneath,
    /// Seiza refused to resolve the stretch.
    Refused(String),
}

/// An output stretch's settings, as PSF Guard records them: Seiza's stretch
/// and how it treats the channels.
pub fn config_of(settings: &serde_json::Value) -> Option<StretchConfig> {
    let model: StretchModel = serde_json::from_value(settings.get("model")?.clone()).ok()?;
    let color_strategy = match settings.get("color_strategy") {
        None | Some(serde_json::Value::Null) => ColorStrategy::default(),
        Some(strategy) => serde_json::from_value(strategy.clone()).ok()?,
    };
    Some(StretchConfig {
        model,
        color_strategy,
        max_analysis_samples: ANALYSIS_SAMPLES,
    })
}

/// Whether a stretch is resolved from the picture's samples.
pub fn needs_samples(config: &StretchConfig) -> bool {
    config.model.requires_analysis()
}

/// Resolves a chain of output stretches, in order, as the canvas draws
/// them. `samples` are RGB samples of the picture before the first, from 0
/// to 1, interleaved; each stretch after the first is resolved from them as
/// the stretches before it left them, as Seiza's stretch stack does.
///
/// A stretch that cannot be drawn is reported in its place. The stretches
/// after it are resolved from the samples without it.
pub fn resolve_chain(
    configs: &[StretchConfig],
    samples: Option<&[f32]>,
) -> Vec<Result<StretchPlan, NotDrawn>> {
    // The samples as the stretches so far left them. They are copied only
    // when a stretch changes them for a later one.
    let mut seen: Option<Cow<[f32]>> = samples.map(Cow::Borrowed);
    let mut drawn = Vec::with_capacity(configs.len());
    for (at, config) in configs.iter().enumerate() {
        let plan = resolve(config, seen.as_deref());
        drawn.push(plan.clone());
        let later_needs_samples = configs[at + 1..].iter().any(needs_samples);
        if let (Ok(plan), Some(samples), true) = (&plan, &seen, later_needs_samples) {
            seen = plan.apply_f32(samples, 3).ok().map(Cow::Owned);
        }
    }
    drawn
}

/// A resolved chain for each chain of output stretches seen lately, so a
/// refresh of the canvas that changes no output stretch and no samples does
/// not resolve them again; and the noise measured lately for a Tone step's
/// Texture, for the same reason.
#[derive(Default)]
pub struct ResolvedChains {
    entries: Vec<ResolvedChain>,
    noise: Vec<(u64, String, f32)>,
    /// Samples of the picture beneath an output stretch that has other
    /// layers above it, by the stretch step's id, with the id of their
    /// sampling: such a stretch is resolved from what is beneath it.
    pub beneath: std::collections::HashMap<uuid::Uuid, (u64, std::sync::Arc<Vec<f32>>)>,
    /// The narrowband balance resolved last, with the id of what it was
    /// resolved from: each band matched to H-alpha, with the background and
    /// midtone of its curve.
    pub balance: Option<(u64, Vec<BalanceCurve>)>,
}

/// One band's narrowband balance: the band, and the background and
/// midtone of its curve.
pub type BalanceCurve = (usize, f32, f32);

/// What the scene worker resolved in the background that a scene built
/// elsewhere needs too: the samples beneath stretches with layers above
/// them, and the narrowband balance.
#[derive(Clone, Default)]
pub struct Resolved {
    pub beneath: std::collections::HashMap<uuid::Uuid, (u64, std::sync::Arc<Vec<f32>>)>,
    pub balance: Option<(u64, Vec<BalanceCurve>)>,
}

impl ResolvedChains {
    /// Chains that start from what the scene worker resolved.
    pub fn from_resolved(resolved: &Resolved) -> Self {
        Self {
            beneath: resolved.beneath.clone(),
            balance: resolved.balance.clone(),
            ..Self::default()
        }
    }

    /// What these chains hold that the scene worker resolved.
    pub fn resolved(&self) -> Resolved {
        Resolved {
            beneath: self.beneath.clone(),
            balance: self.balance.clone(),
        }
    }
}

/// One chain resolved: the id of the samples it was resolved from (none
/// when it reads no samples), the chain, and how the canvas draws each
/// stretch of it.
type ResolvedChain = (
    Option<u64>,
    Vec<StretchConfig>,
    Vec<Result<StretchPlan, NotDrawn>>,
);

impl ResolvedChains {
    /// The chains a cache keeps.
    const KEPT: usize = 8;

    /// The noise of the picture as the Finish steps `before` leave it,
    /// measured by `measure` from the samples of sampling `id`, or as it was
    /// measured before from the same samples and steps.
    pub fn noise(
        &mut self,
        id: u64,
        before: &[seiza_compose::picture::FinishStep],
        measure: impl FnOnce() -> f32,
    ) -> f32 {
        let key = format!("{before:?}");
        if let Some((_, _, noise)) = self
            .noise
            .iter()
            .find(|(at, steps, _)| *at == id && *steps == key)
        {
            return *noise;
        }
        let noise = measure();
        self.noise.insert(0, (id, key, noise));
        self.noise.truncate(Self::KEPT);
        noise
    }

    /// [`resolve_chain`], from the cache when this chain was resolved from
    /// these samples before. `samples` carry the id of the sampling they
    /// came from.
    pub fn resolve(
        &mut self,
        configs: &[StretchConfig],
        samples: Option<(u64, &[f32])>,
    ) -> Vec<Result<StretchPlan, NotDrawn>> {
        // Explicit stretches do not read the samples, so any samples give
        // the same result.
        let id = samples
            .filter(|_| configs.iter().any(needs_samples))
            .map(|(id, _)| id);
        if let Some((_, _, known)) = self
            .entries
            .iter()
            .find(|(at, chain, _)| *at == id && chain.as_slice() == configs)
        {
            return known.clone();
        }
        let resolved = resolve_chain(configs, samples.map(|(_, samples)| samples));
        self.entries
            .insert(0, (id, configs.to_vec(), resolved.clone()));
        self.entries.truncate(Self::KEPT);
        resolved
    }
}

/// Whether a stretch needs the whole image, as the layered stretches do:
/// the store stretches the plane, and the canvas draws it as it is.
pub(crate) fn needs_image(model: &StretchModel) -> bool {
    matches!(
        model,
        StretchModel::Layered(_) | StretchModel::LayeredHdr(_)
    )
}

fn resolve(config: &StretchConfig, samples: Option<&[f32]>) -> Result<StretchPlan, NotDrawn> {
    if needs_image(&config.model) {
        return Err(NotDrawn::NeedsImage);
    }
    if !needs_samples(config) {
        return config
            .resolve_explicit(3)
            .map_err(|error| NotDrawn::Refused(error.to_string()));
    }
    let samples = samples.ok_or(NotDrawn::NeedsSamples)?;
    config
        .resolve_for(samples, 3)
        .map_err(|error| NotDrawn::Refused(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn both_layered_stretches_need_the_whole_image() {
        for settings in [
            json!({ "type": "layered" }),
            json!({ "type": "layered-hdr", "headroom": 3 }),
        ] {
            let model: StretchModel = serde_json::from_value(settings.clone()).unwrap();
            assert!(needs_image(&model), "{settings}");
        }
        let model: StretchModel =
            serde_json::from_value(json!({ "type": "median-anchor" })).unwrap();
        assert!(!needs_image(&model));
    }

    fn config(settings: serde_json::Value) -> StretchConfig {
        config_of(&settings).expect("a stretch Seiza knows")
    }

    /// Dim samples with some spread, red, green and blue alike.
    fn dim() -> Vec<f32> {
        (0..30_000)
            .map(|index| 0.02 + 0.01 * ((index * 7919) % 1000) as f32 / 1000.0)
            .collect()
    }

    #[test]
    fn an_explicit_stretch_is_drawn_without_samples() {
        let ghs = config(json!({
            "model": { "type": "ghs", "stretch_factor": 3.0, "local_intensity": 4.0,
                       "symmetry_point": 0.2, "protect_shadows": 0.0,
                       "protect_highlights": 1.0, "black": 0.0, "white": 1.0 },
            "color_strategy": "linked",
        }));
        let [drawn] = &resolve_chain(std::slice::from_ref(&ghs), None)[..] else {
            panic!("one stretch");
        };
        assert_eq!(drawn.as_ref(), Ok(&ghs.resolve_explicit(3).unwrap()));
    }

    #[test]
    fn a_stretch_from_the_pictures_statistics_waits_for_samples() {
        let auto = config(json!({
            "model": { "type": "auto-mtf", "target_median": 0.25, "shadows_clip": -2.8 },
        }));
        assert_eq!(
            resolve_chain(std::slice::from_ref(&auto), None),
            [Err(NotDrawn::NeedsSamples)]
        );
        let samples = dim();
        let [drawn] = &resolve_chain(std::slice::from_ref(&auto), Some(&samples))[..] else {
            panic!("one stretch");
        };
        assert_eq!(drawn.as_ref(), Ok(&auto.resolve_for(&samples, 3).unwrap()));
    }

    #[test]
    fn a_later_stretch_is_resolved_from_what_the_one_before_left() {
        let linear = config(json!({
            "model": { "type": "linear", "black": 0.0, "white": 0.5 },
        }));
        let auto = config(json!({
            "model": { "type": "auto-mtf", "target_median": 0.25, "shadows_clip": -2.8 },
        }));
        let samples = dim();
        let drawn = resolve_chain(&[linear.clone(), auto.clone()], Some(&samples));
        let after = linear
            .resolve_explicit(3)
            .unwrap()
            .apply_f32(&samples, 3)
            .unwrap();
        assert_eq!(drawn[1].as_ref(), Ok(&auto.resolve_for(&after, 3).unwrap()));
    }

    #[test]
    fn a_chain_is_resolved_again_only_for_new_samples() {
        let auto = config(json!({
            "model": { "type": "auto-mtf", "target_median": 0.25, "shadows_clip": -2.8 },
        }));
        let dim = dim();
        let bright: Vec<f32> = dim.iter().map(|sample| sample * 4.0).collect();
        let mut chains = ResolvedChains::default();
        let first = chains.resolve(std::slice::from_ref(&auto), Some((1, &dim)));
        // The same id is the same samples: the kept chain is given back,
        // although these samples differ.
        let kept = chains.resolve(std::slice::from_ref(&auto), Some((1, &bright)));
        assert_eq!(first, kept);
        let again = chains.resolve(std::slice::from_ref(&auto), Some((2, &bright)));
        assert_ne!(first, again);

        // An explicit stretch reads no samples, so new ones change nothing.
        let ghs = config(json!({
            "model": { "type": "ghs", "stretch_factor": 3.0, "local_intensity": 4.0,
                       "symmetry_point": 0.2, "protect_shadows": 0.0,
                       "protect_highlights": 1.0, "black": 0.0, "white": 1.0 },
        }));
        let without = chains.resolve(std::slice::from_ref(&ghs), None);
        assert_eq!(
            without,
            chains.resolve(std::slice::from_ref(&ghs), Some((3, &dim)))
        );
    }

    #[test]
    fn the_layered_stretch_is_left_to_the_engine() {
        let layered = config(json!({
            "model": { "type": "layered", "target_background": 0.15, "aggressiveness": 0.5,
                       "dynamic_range_compression": 0.0, "contrast_recovery": 0.0 },
        }));
        assert_eq!(
            resolve_chain(&[layered], Some(&[0.1; 300])),
            [Err(NotDrawn::NeedsImage)]
        );
    }
}
