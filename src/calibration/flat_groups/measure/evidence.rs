//! Whether a night's own lights carry a change.
//!
//! A light's sky background follows the flat response of the moment it was
//! shot. Dividing a night's background by a flat from one side of a change
//! leaves the change's shape behind, or does not. So the background, taken
//! as the median of a few lights with their stars masked, is divided by a
//! reference flat response and its compact part fitted against the
//! change's map: a weight near 0 means the lights match the reference's
//! side, near 1 the other.
//!
//! Nebulae have compact structure of their own. The weight's spread is
//! therefore measured by fitting the same template at placebo positions
//! across the frame, where no change is; a weight is decisive only when it
//! stands clear of that spread. Over a smooth field it does; over a nebula
//! it usually does not, and the answer is "undecided".
//!
//! A change of several shadows is fitted one shadow at a time, each against
//! placebos the size of that shadow, and the shadows' weights are combined
//! by inverse variance. Shadows spread across the frame leave no room to
//! move the whole template; each one alone has plenty.

use super::metric::{self, Plane};
use super::model::Side;
use super::rows::Verdict;
use seiza_imgproc::components::{connected_components, Connectivity};

/// Bump when equal lights may fit differently. 2: one fit per shadow.
pub const EVIDENCE_ALGORITHM_VERSION: u32 = 2;
/// The most lights one fit reads, evenly spaced through the session.
pub const MAX_LIGHTS: usize = 8;
/// The fewest lights a background needs.
pub const MIN_LIGHTS: usize = 3;
/// A template pixel is part of the change when it departs from 1 by this
/// share of the change's deepest point.
const SUPPORT_SHARE: f32 = 0.25;
/// A side is chosen when the weight lies within this many spreads of it...
const NEAR_SPREADS: f64 = 2.0;
/// ...and at least this many from the other side...
const FAR_SPREADS: f64 = 4.0;
/// ...and the spread is at most this: the sides are 1 apart.
const MAX_SPREAD: f64 = 0.3;
/// Fewest placebo positions a spread needs.
const MIN_PLACEBOS: usize = 6;
/// Pieces of the change's support closer than this, in response pixels, are
/// one shadow: a donut's ring, or a shadow its threshold cut in two.
const SHADOW_JOIN: usize = 3;
/// A shadow needs this many support pixels to be fitted on its own.
const MIN_SHADOW_PIXELS: usize = 4;
/// No shadow's spread counts as smaller than this when shadows are
/// weighted: one whose placebos happen to agree must not drown the others.
const MIN_SHADOW_SPREAD: f64 = 0.02;
/// Placebo positions around a shadow with no room for whole moves: this
/// many directions on each ring...
const RING_DIRECTIONS: usize = 16;
/// ...at these multiples of the shadow's longer side.
const RING_RADII: [f64; 3] = [1.0, 1.5, 2.0];
/// Stars in a few dithered lights stay in their median, and a star field
/// holds thousands: grown as far as in a flat they would cover most of the
/// frame. One pixel takes the cores and leaves the change room to show.
const LIGHT_STAR_GROW: usize = 1;

/// One fit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    /// How much of the change the lights carry: 0 none, 1 all of it.
    pub carried: f64,
    pub spread: f64,
    pub verdict: Verdict,
}

/// The sky background of a few lights: each less its pedestal, divided by
/// its own median, combined by a clipped median.
pub fn background(binned: &[Plane], pedestal: f32, bin: usize) -> Option<Plane> {
    if binned.len() < MIN_LIGHTS {
        return None;
    }
    let less: Vec<Plane> = binned
        .iter()
        .map(|plane| Plane {
            width: plane.width,
            height: plane.height,
            data: plane.data.iter().map(|v| v - pedestal).collect(),
        })
        .collect();
    metric::build_response(&less, bin).map(|response| response.plane)
}

/// The least-squares weight of `template` in `residual`, the template moved
/// by `(dx, dy)` response pixels. Pixels that leave the frame or were
/// excluded take no part; at least half the template must.
fn weight(
    residual: &[f32],
    excluded: &[bool],
    width: usize,
    template: &[(usize, f32)],
    (dx, dy): (isize, isize),
) -> Option<f64> {
    let height = residual.len() / width.max(1);
    let (mut numerator, mut denominator, mut used) = (0f64, 0f64, 0usize);
    for (index, t) in template {
        let x = (index % width) as isize + dx;
        let y = (index / width) as isize + dy;
        if x < 0 || y < 0 || x >= width as isize || y >= height as isize {
            continue;
        }
        let at = y as usize * width + x as usize;
        if excluded[at] {
            continue;
        }
        numerator += f64::from(residual[at]) * f64::from(*t);
        denominator += f64::from(*t) * f64::from(*t);
        used += 1;
    }
    (used * 2 >= template.len() && denominator > 0.0).then(|| numerator / denominator)
}

/// The change's template, `map − 1` where it departs from 1 by
/// [`SUPPORT_SHARE`] of its deepest point, split into shadows: connected
/// pieces of that support, pieces within [`SHADOW_JOIN`] of each other
/// taken as one.
fn shadows(map: &Plane) -> Vec<Vec<(usize, f32)>> {
    let deepest = map
        .data
        .iter()
        .map(|v| (v - 1.0).abs())
        .fold(0f32, f32::max);
    if deepest <= 0.0 {
        return Vec::new();
    }
    let support: Vec<bool> = map
        .data
        .iter()
        .map(|v| (v - 1.0).abs() >= SUPPORT_SHARE * deepest)
        .collect();
    let joined = metric::grow(&support, map.width, map.height, SHADOW_JOIN.div_ceil(2));
    let mask: Vec<u8> = joined.iter().map(|on| u8::from(*on)).collect();
    connected_components(&mask, map.width, map.height, Connectivity::Eight)
        .into_iter()
        .map(|component| {
            let mut pixels: Vec<usize> = component
                .pixels
                .into_iter()
                .filter(|index| support[*index])
                .collect();
            pixels.sort_unstable();
            pixels
                .into_iter()
                .map(|index| (index, map.data[index] - 1.0))
                .collect::<Vec<_>>()
        })
        .filter(|template| template.len() >= MIN_SHADOW_PIXELS)
        .collect()
}

/// One shadow's weight against its placebos: the weight less the placebos'
/// middle, and their spread.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ShadowFit {
    weight: f64,
    spread: f64,
}

/// Fit one shadow. `others` marks the change's other shadows: a placebo
/// that lands on one would weigh the change, not the sky.
fn fit_shadow(
    residual: &[f32],
    excluded: &[bool],
    others: &[bool],
    width: usize,
    template: &[(usize, f32)],
) -> Option<ShadowFit> {
    let height = residual.len() / width;
    let clear = |(dx, dy): (isize, isize)| {
        template.iter().all(|(index, _)| {
            let x = (index % width) as isize + dx;
            let y = (index / width) as isize + dy;
            x < 0
                || y < 0
                || x >= width as isize
                || y >= height as isize
                || !others[y as usize * width + x as usize]
        })
    };
    let alpha = weight(residual, excluded, width, template, (0, 0))?;
    let (min_x, max_x, min_y, max_y) = template.iter().fold(
        (usize::MAX, 0usize, usize::MAX, 0usize),
        |(a, b, c, d), (index, _)| {
            let (x, y) = (index % width, index / width);
            (a.min(x), b.max(x), c.min(y), d.max(y))
        },
    );
    let (span_x, span_y) = ((max_x - min_x + 1) as isize, (max_y - min_y + 1) as isize);
    // Placebos: the shadow moved by whole multiples of its own size, clear
    // of itself and inside the frame.
    let mut placebos = Vec::new();
    for gy in -4isize..=4 {
        for gx in -4isize..=4 {
            if gx == 0 && gy == 0 {
                continue;
            }
            let (dx, dy) = (gx * span_x, gy * span_y);
            let (x0, x1) = (min_x as isize + dx, max_x as isize + dx);
            let (y0, y1) = (min_y as isize + dy, max_y as isize + dy);
            if x0 < 0 || y0 < 0 || x1 >= width as isize || y1 >= height as isize {
                continue;
            }
            if !clear((dx, dy)) {
                continue;
            }
            if let Some(w) = weight(residual, excluded, width, template, (dx, dy)) {
                placebos.push(w);
            }
        }
    }
    // A shadow too large for that, or pressed into a corner: rings around
    // it, still clear of it, each placebo at least half inside the frame.
    if placebos.len() < MIN_PLACEBOS {
        placebos.clear();
        let side = span_x.max(span_y) as f64;
        for radius in RING_RADII {
            for step in 0..RING_DIRECTIONS {
                let angle = std::f64::consts::TAU * step as f64 / RING_DIRECTIONS as f64;
                let dx = (radius * side * angle.cos()).round() as isize;
                let dy = (radius * side * angle.sin()).round() as isize;
                if (dx.abs() < span_x && dy.abs() < span_y) || !clear((dx, dy)) {
                    continue;
                }
                if let Some(w) = weight(residual, excluded, width, template, (dx, dy)) {
                    placebos.push(w);
                }
            }
        }
    }
    if placebos.len() < MIN_PLACEBOS {
        return None;
    }
    // Robust: a placebo that lands on a bright knot of nebula says how
    // that knot fits, not how the weight scatters. The placebos' middle is
    // where a fit sits with no change under it.
    let middle = median(placebos.clone());
    let spread = 1.4826 * median(placebos.iter().map(|w| (w - middle).abs()).collect());
    Some(ShadowFit {
        weight: alpha - middle,
        spread,
    })
}

/// The shadows' weights as one, by inverse variance. When the shadows
/// disagree by more than their spreads allow, the spread grows by that
/// excess (the Birge ratio): it is structure under some shadows, not noise.
fn combine_shadows(fits: &[ShadowFit]) -> Option<ShadowFit> {
    let weights: Vec<f64> = fits
        .iter()
        .map(|fit| fit.spread.max(MIN_SHADOW_SPREAD).powi(-2))
        .collect();
    let total: f64 = weights.iter().sum();
    if fits.is_empty() || total <= 0.0 {
        return None;
    }
    let mean = fits
        .iter()
        .zip(&weights)
        .map(|(fit, w)| fit.weight * w)
        .sum::<f64>()
        / total;
    let mut spread = total.sqrt().recip();
    if fits.len() >= 2 {
        let chi2: f64 = fits
            .iter()
            .zip(&weights)
            .map(|(fit, w)| w * (fit.weight - mean).powi(2))
            .sum();
        spread *= (chi2 / (fits.len() - 1) as f64).sqrt().max(1.0);
    }
    Some(ShadowFit {
        weight: mean,
        spread,
    })
}

/// Fit the background against a change. `reference` is a flat response,
/// pedestal removed, from `reference_side` of the change; `map` is the
/// change's map, after over before, 1 away from it.
pub fn fit(
    background: &Plane,
    reference: &Plane,
    reference_side: Side,
    map: &Plane,
) -> Option<Fit> {
    if background.width != map.width || background.height != map.height {
        return None;
    }
    let (residual, excluded) = metric::compact_ratio(reference, background, LIGHT_STAR_GROW)?;
    let shadows = shadows(map);
    if shadows.is_empty() {
        return None;
    }
    // The background over the reference: a weight of 0 means the lights
    // share the reference's side.
    let offset = if reference_side == Side::After {
        1.0
    } else {
        0.0
    };
    let fits: Vec<ShadowFit> = shadows
        .iter()
        .enumerate()
        .filter_map(|(i, template)| {
            let mut others = vec![false; map.data.len()];
            for (j, shadow) in shadows.iter().enumerate() {
                if j != i {
                    for (index, _) in shadow {
                        others[*index] = true;
                    }
                }
            }
            fit_shadow(&residual, &excluded, &others, map.width, template)
        })
        .collect();
    let Some(combined) = combine_shadows(&fits) else {
        // No shadow could be weighed against placebos: say what the whole
        // change weighs, and decide nothing.
        let whole: Vec<(usize, f32)> = shadows.into_iter().flatten().collect();
        let alpha = weight(&residual, &excluded, map.width, &whole, (0, 0))?;
        return Some(Fit {
            carried: alpha + offset,
            spread: f64::INFINITY,
            verdict: Verdict::Undecided,
        });
    };
    let (carried, spread) = (combined.weight + offset, combined.spread);
    let decides = |side: f64, other: f64| {
        spread <= MAX_SPREAD
            && (carried - side).abs() <= NEAR_SPREADS * spread.max(0.05)
            && (carried - other).abs() >= FAR_SPREADS * spread
    };
    let verdict = if decides(0.0, 1.0) {
        Verdict::Before
    } else if decides(1.0, 0.0) {
        Verdict::After
    } else {
        Verdict::Undecided
    };
    Some(Fit {
        carried,
        spread,
        verdict,
    })
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// One verdict for an imaging session from its filters' fits: a side some
/// fit decides and none contradicts.
pub fn combine(verdicts: &[Verdict]) -> Verdict {
    let before = verdicts.contains(&Verdict::Before);
    let after = verdicts.contains(&Verdict::After);
    match (before, after) {
        (true, false) => Verdict::Before,
        (false, true) => Verdict::After,
        _ => Verdict::Undecided,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::flat_groups::metric::tests::{noise, synthetic};

    fn sky(flat: &Plane, nebula: f32, seed: u64) -> Plane {
        Plane {
            width: flat.width,
            height: flat.height,
            data: flat
                .data
                .iter()
                .enumerate()
                .map(|(index, f)| {
                    let (x, y) = (index % flat.width, index / flat.width);
                    // Nebula: compact blobs of structure everywhere.
                    let structure = nebula * (noise(x / 6, y / 6, seed + 99));
                    1000.0 * f * (1.0 + structure) * (1.0 + 0.002 * noise(x, y, seed))
                })
                .collect(),
        }
    }

    fn setup() -> (Plane, Plane, Plane) {
        let dust = [(60.0, 90.0, 14.0, 0.016)];
        let before = synthetic(240, 160, &[], 0.0, &[], 0.0003, 1);
        let after = synthetic(240, 160, &dust, 0.0, &[], 0.0003, 2);
        let map = metric::change_map(&before, &after).expect("map");
        (before, after, map)
    }

    #[test]
    fn a_smooth_field_decides_its_side() {
        let (before, after, map) = setup();
        for (truth, expected) in [(&before, Verdict::Before), (&after, Verdict::After)] {
            let lights: Vec<Plane> = (0..5).map(|seed| sky(truth, 0.0, seed)).collect();
            let background = background(&lights, 0.0, 10).expect("background");
            let fitted = fit(&background, &before, Side::Before, &map).expect("fit");
            assert_eq!(fitted.verdict, expected, "{fitted:?}");
            let against_after = fit(&background, &after, Side::After, &map).expect("fit");
            assert_eq!(against_after.verdict, expected, "{against_after:?}");
        }
    }

    #[test]
    fn a_nebula_leaves_it_undecided() {
        let (before, _, map) = setup();
        let lights: Vec<Plane> = (0..5).map(|seed| sky(&before, 0.05, seed)).collect();
        let background = background(&lights, 0.0, 10).expect("background");
        let fitted = fit(&background, &before, Side::Before, &map).expect("fit");
        assert_eq!(fitted.verdict, Verdict::Undecided, "{fitted:?}");
    }

    /// Four shadows from one edge of the frame to the other, as C925's
    /// 2026 donut change: the whole template has no room to move.
    fn spread_out() -> (Plane, Plane, Plane) {
        let dust = [
            (7.0, 40.0, 6.0, 0.02),
            (80.0, 120.0, 9.0, 0.016),
            (165.0, 30.0, 9.0, 0.018),
            (232.0, 130.0, 6.0, 0.02),
        ];
        let before = synthetic(240, 160, &[], 0.0, &[], 0.0003, 1);
        let after = synthetic(240, 160, &dust, 0.0, &[], 0.0003, 2);
        let map = metric::change_map(&before, &after).expect("map");
        (before, after, map)
    }

    #[test]
    fn shadows_across_the_frame_are_fitted_one_by_one() {
        let (before, after, map) = spread_out();
        assert_eq!(shadows(&map).len(), 4);
        for (truth, expected) in [(&before, Verdict::Before), (&after, Verdict::After)] {
            let lights: Vec<Plane> = (0..5).map(|seed| sky(truth, 0.0, seed)).collect();
            let background = background(&lights, 0.0, 10).expect("background");
            for (reference, side) in [(&before, Side::Before), (&after, Side::After)] {
                let fitted = fit(&background, reference, side, &map).expect("fit");
                assert_eq!(fitted.verdict, expected, "{fitted:?}");
                assert!(
                    fitted.spread.is_finite() && fitted.spread < 0.1,
                    "{fitted:?}"
                );
            }
        }
    }

    #[test]
    fn shadows_across_a_nebula_never_take_the_wrong_side() {
        // Each shadow's placebos measure the nebula under it. Several
        // shadows over light structure may decide together, and only for
        // the side the lights are on; heavy structure leaves it undecided.
        let (before, after, map) = spread_out();
        for (nebula, may_decide) in [(0.03f32, true), (0.05, true), (0.08, false), (0.1, false)] {
            for (truth, wrong) in [(&before, Verdict::After), (&after, Verdict::Before)] {
                let lights: Vec<Plane> = (0..5).map(|seed| sky(truth, nebula, seed)).collect();
                let background = background(&lights, 0.0, 10).expect("background");
                for (reference, side) in [(&before, Side::Before), (&after, Side::After)] {
                    let fitted = fit(&background, reference, side, &map).expect("fit");
                    assert_ne!(fitted.verdict, wrong, "{nebula}: {fitted:?}");
                    if !may_decide {
                        assert_eq!(fitted.verdict, Verdict::Undecided, "{nebula}: {fitted:?}");
                    }
                }
            }
        }
    }

    /// The nebula tests above use one structure each; here thirty, for one
    /// shadow and for four. At three spreads from the other side two of
    /// these 960 fits took the wrong side, one each, both with spreads near
    /// the limit: four spreads leave none.
    #[test]
    fn many_nebulae_never_take_the_wrong_side() {
        for (before, after, map) in [setup(), spread_out()] {
            let (mut wrong, mut decided) = (Vec::new(), 0);
            for k in 0..30u64 {
                for nebula in [0.02f32, 0.03, 0.05, 0.08] {
                    for (truth, other) in [(&before, Verdict::After), (&after, Verdict::Before)] {
                        let lights: Vec<Plane> = (0..5)
                            .map(|seed| sky(truth, nebula, 1000 * (k + 1) + seed))
                            .collect();
                        let background = background(&lights, 0.0, 10).expect("background");
                        for (reference, side) in [(&before, Side::Before), (&after, Side::After)] {
                            let fitted = fit(&background, reference, side, &map).expect("fit");
                            if fitted.verdict == other {
                                wrong.push((k, nebula, fitted));
                            } else if fitted.verdict != Verdict::Undecided {
                                decided += 1;
                            }
                        }
                    }
                }
            }
            assert!(wrong.is_empty(), "{wrong:?}");
            // Light structure still decides, the right way, often.
            assert!(decided > 100, "{decided}");
        }
    }

    #[test]
    fn a_shadow_with_no_room_is_weighed_on_a_ring() {
        // A shadow over half the frame's height, near its left edge: two
        // whole moves of it stay inside, too few, so its placebos sit on
        // rings around it, partly outside the frame.
        let (width, height) = (160usize, 100usize);
        let template: Vec<(usize, f32)> = (0..width * height)
            .filter(|index| {
                let (x, y) = ((index % width) as f32, (index / width) as f32);
                (x - 40.0).powi(2) + (y - 50.0).powi(2) <= 22f32.powi(2)
            })
            .map(|index| (index, -0.02))
            .collect();
        let mut residual: Vec<f32> = (0..width * height)
            .map(|index| 0.002 * noise(index % width, index / width, 7))
            .collect();
        for (index, t) in &template {
            residual[*index] += t;
        }
        let excluded = vec![false; width * height];
        let others = vec![false; width * height];
        let fitted =
            fit_shadow(&residual, &excluded, &others, width, &template).expect("ring placebos");
        assert!((fitted.weight - 1.0).abs() < 0.1, "{fitted:?}");
        assert!(fitted.spread < 0.2, "{fitted:?}");
    }

    #[test]
    fn shadows_that_disagree_widen_the_spread() {
        let agree = [
            ShadowFit {
                weight: 0.0,
                spread: 0.05,
            },
            ShadowFit {
                weight: 0.02,
                spread: 0.05,
            },
        ];
        let combined = combine_shadows(&agree).unwrap();
        assert!((combined.spread - 0.05 / 2f64.sqrt()).abs() < 1e-9);
        let disagree = [
            ShadowFit {
                weight: 0.0,
                spread: 0.05,
            },
            ShadowFit {
                weight: 1.0,
                spread: 0.05,
            },
        ];
        let combined = combine_shadows(&disagree).unwrap();
        assert!((combined.weight - 0.5).abs() < 1e-9);
        assert!(combined.spread > 0.3, "{combined:?}");
    }

    #[test]
    fn a_session_takes_a_side_no_fit_contradicts() {
        assert_eq!(
            combine(&[Verdict::Before, Verdict::Undecided]),
            Verdict::Before
        );
        assert_eq!(
            combine(&[Verdict::Before, Verdict::After]),
            Verdict::Undecided
        );
        assert_eq!(combine(&[]), Verdict::Undecided);
    }
}
