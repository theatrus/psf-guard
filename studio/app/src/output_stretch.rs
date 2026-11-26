//! The stretches of the combined picture: PSF Guard's output stretches, as
//! the canvas draws them.
//!
//! An output stretch is one of Seiza's stretches of the whole picture after
//! the layers are combined. Seiza resolves each one, and the canvas draws
//! it as three stretch-table rows, one per channel: a linked stretch puts
//! its one curve in all three, an unlinked one a curve per channel, and a
//! luminance-preserving one its luminance curve in the first, which the
//! Finish pass applies to the pixel's luminance.
//!
//! A stretch with explicit settings, such as a generalized hyperbolic
//! stretch, resolves without the picture. One that Seiza resolves from the
//! picture's statistics, such as the automatic midtones stretch, needs
//! samples of the picture as the stretches before it left it. The layered
//! stretch needs the whole image, so the canvas does not draw it.

use psf_studio_render::STRETCH_TABLE_WIDTH;
use seiza_stretch::{ColorStrategy, StretchConfig, StretchModel, StretchPlan};
use std::borrow::Cow;

/// The samples Seiza analyses per stretch, at most.
const ANALYSIS_SAMPLES: usize = 300_000;

/// One output stretch as the canvas draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct PictureStretch {
    /// The red, green and blue rows of the stretch table.
    pub rows: [Vec<f32>; 3],
    /// Whether the first row maps the pixel's luminance, which scales the
    /// pixel, in place of one row per channel.
    pub luminance: bool,
}

/// Why the canvas cannot draw an output stretch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotDrawn {
    /// The layered stretch needs the whole image, not samples of it.
    NeedsImage,
    /// The stretch is resolved from the picture, and the canvas has no
    /// samples of the picture yet.
    NeedsSamples,
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
) -> Vec<Result<PictureStretch, NotDrawn>> {
    // The samples as the stretches so far left them. They are copied only
    // when a stretch changes them for a later one.
    let mut seen: Option<Cow<[f32]>> = samples.map(Cow::Borrowed);
    let mut drawn = Vec::with_capacity(configs.len());
    for (at, config) in configs.iter().enumerate() {
        let plan = resolve(config, seen.as_deref());
        drawn.push(plan.as_ref().map(stretch_of).map_err(Clone::clone));
        let later_needs_samples = configs[at + 1..].iter().any(needs_samples);
        if let (Ok(plan), Some(samples), true) = (&plan, &seen, later_needs_samples) {
            seen = plan.apply_f32(samples, 3).ok().map(Cow::Owned);
        }
    }
    drawn
}

/// A resolved chain for each chain of output stretches seen lately, so a
/// refresh of the canvas that changes no output stretch and no samples does
/// not resolve them again.
#[derive(Default)]
pub struct ResolvedChains {
    entries: Vec<ResolvedChain>,
}

/// One chain resolved: the id of the samples it was resolved from (none
/// when it reads no samples), the chain, and how the canvas draws each
/// stretch of it.
type ResolvedChain = (
    Option<u64>,
    Vec<StretchConfig>,
    Vec<Result<PictureStretch, NotDrawn>>,
);

impl ResolvedChains {
    /// The chains a cache keeps.
    const KEPT: usize = 8;

    /// [`resolve_chain`], from the cache when this chain was resolved from
    /// these samples before. `samples` carry the id of the sampling they
    /// came from.
    pub fn resolve(
        &mut self,
        configs: &[StretchConfig],
        samples: Option<(u64, &[f32])>,
    ) -> Vec<Result<PictureStretch, NotDrawn>> {
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

fn resolve(config: &StretchConfig, samples: Option<&[f32]>) -> Result<StretchPlan, NotDrawn> {
    if matches!(config.model, StretchModel::Layered(_)) {
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

/// A resolved plan as stretch-table rows.
fn stretch_of(plan: &StretchPlan) -> PictureStretch {
    let row = |curve: seiza_stretch::ResolvedCurve| -> Vec<f32> {
        let last = (STRETCH_TABLE_WIDTH - 1) as f32;
        // Entry i stands for the value (i / last) squared, as a layer's row
        // does.
        (0..STRETCH_TABLE_WIDTH)
            .map(|index| curve.map((index as f32 / last).powi(2)).clamp(0.0, 1.0))
            .collect()
    };
    let curves = plan.curves();
    match plan.color_strategy() {
        ColorStrategy::Unlinked => PictureStretch {
            rows: std::array::from_fn(|channel| row(curves[channel.min(curves.len() - 1)])),
            luminance: false,
        },
        strategy => {
            let one = row(curves[0]);
            PictureStretch {
                rows: [one.clone(), one.clone(), one],
                luminance: strategy == ColorStrategy::LuminancePreserving,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(settings: serde_json::Value) -> StretchConfig {
        config_of(&settings).expect("a stretch Seiza knows")
    }

    /// The value a row gives at `value`, read as the shader reads it: by
    /// the square root of the value, interpolated.
    fn at(row: &[f32], value: f32) -> f32 {
        let place = value.clamp(0.0, 1.0).sqrt() * (row.len() - 1) as f32;
        let low = place.floor() as usize;
        let high = (low + 1).min(row.len() - 1);
        let t = place - low as f32;
        row[low] * (1.0 - t) + row[high] * t
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
        let drawn = drawn.as_ref().expect("drawn");
        assert!(!drawn.luminance);
        let plan = ghs.resolve_explicit(3).unwrap();
        for value in [0.01_f32, 0.1, 0.3, 0.7] {
            let expected = plan.curves()[0].map(value);
            for row in &drawn.rows {
                assert!((at(row, value) - expected).abs() < 2e-3, "{value}");
            }
        }
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

        // A dim picture with some spread: the stretch puts its median near
        // the target, as Seiza's plan of the same samples does.
        let samples: Vec<f32> = (0..30_000)
            .map(|index| 0.02 + 0.01 * ((index * 7919) % 1000) as f32 / 1000.0)
            .collect();
        let [drawn] = &resolve_chain(std::slice::from_ref(&auto), Some(&samples))[..] else {
            panic!("one stretch");
        };
        let drawn = drawn.as_ref().expect("drawn");
        let plan = auto.resolve_for(&samples, 3).unwrap();
        for value in [0.02_f32, 0.025, 0.03] {
            assert!((at(&drawn.rows[1], value) - plan.curves()[0].map(value)).abs() < 3e-3);
        }
    }

    #[test]
    fn a_later_stretch_is_resolved_from_what_the_one_before_left() {
        let linear = config(json!({
            "model": { "type": "linear", "black": 0.0, "white": 0.5 },
        }));
        let auto = config(json!({
            "model": { "type": "auto-mtf", "target_median": 0.25, "shadows_clip": -2.8 },
        }));
        let samples: Vec<f32> = (0..30_000)
            .map(|index| 0.02 + 0.01 * ((index * 7919) % 1000) as f32 / 1000.0)
            .collect();
        let drawn = resolve_chain(&[linear.clone(), auto.clone()], Some(&samples));
        let after = linear
            .resolve_explicit(3)
            .unwrap()
            .apply_f32(&samples, 3)
            .unwrap();
        let expected = auto.resolve_for(&after, 3).unwrap();
        let second = drawn[1].as_ref().expect("drawn");
        for value in [0.04_f32, 0.05, 0.06] {
            assert!((at(&second.rows[0], value) - expected.curves()[0].map(value)).abs() < 3e-3);
        }
    }

    #[test]
    fn channels_follow_the_color_strategy() {
        let samples: Vec<f32> = (0..30_000)
            .map(|index| match index % 3 {
                0 => 0.02,
                1 => 0.04,
                _ => 0.08,
            } + 0.005 * ((index * 7919) % 1000) as f32 / 1000.0)
            .collect();
        let unlinked = config(json!({
            "model": { "type": "auto-mtf", "target_median": 0.25, "shadows_clip": -2.8 },
            "color_strategy": "unlinked",
        }));
        let drawn = resolve_chain(&[unlinked], Some(&samples));
        let drawn = drawn[0].as_ref().unwrap();
        // Each channel's median lands near the target on its own row.
        assert!((at(&drawn.rows[0], 0.0225) - 0.25).abs() < 0.05);
        assert!((at(&drawn.rows[2], 0.0825) - 0.25).abs() < 0.05);
        assert!(!drawn.luminance);

        let luminance = config(json!({
            "model": { "type": "asinh", "black": 0.0, "white": 1.0, "strength": 20.0 },
            "color_strategy": "luminance-preserving",
        }));
        let drawn = resolve_chain(&[luminance], None);
        let drawn = drawn[0].as_ref().unwrap();
        assert!(drawn.luminance);
        assert_eq!(drawn.rows[0], drawn.rows[2]);
    }

    #[test]
    fn a_chain_is_resolved_again_only_for_new_samples() {
        let auto = config(json!({
            "model": { "type": "auto-mtf", "target_median": 0.25, "shadows_clip": -2.8 },
        }));
        let dim: Vec<f32> = (0..30_000)
            .map(|index| 0.02 + 0.01 * ((index * 7919) % 1000) as f32 / 1000.0)
            .collect();
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
