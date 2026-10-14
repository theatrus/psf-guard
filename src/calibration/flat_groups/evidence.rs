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

use super::metric::{self, Plane};
use super::model::Side;
use super::store::Verdict;

/// Bump when equal lights may fit differently.
pub const EVIDENCE_ALGORITHM_VERSION: u32 = 1;
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
const FAR_SPREADS: f64 = 3.0;
/// ...and the spread is at most this: the sides are 1 apart.
const MAX_SPREAD: f64 = 0.3;
/// Fewest placebo positions a spread needs.
const MIN_PLACEBOS: usize = 6;
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

fn weight(
    residual: &[f32],
    excluded: &[bool],
    template: &[(usize, f32)],
    shift: isize,
) -> Option<f64> {
    let (mut numerator, mut denominator, mut used) = (0f64, 0f64, 0usize);
    for (index, t) in template {
        let at = *index as isize + shift;
        if at < 0 || at as usize >= residual.len() {
            return None;
        }
        let at = at as usize;
        if excluded[at] {
            continue;
        }
        numerator += f64::from(residual[at]) * f64::from(*t);
        denominator += f64::from(*t) * f64::from(*t);
        used += 1;
    }
    (used * 2 >= template.len() && denominator > 0.0).then(|| numerator / denominator)
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
    let deepest = map
        .data
        .iter()
        .map(|v| (v - 1.0).abs())
        .fold(0f32, f32::max);
    if deepest <= 0.0 {
        return None;
    }
    let template: Vec<(usize, f32)> = map
        .data
        .iter()
        .enumerate()
        .map(|(index, v)| (index, v - 1.0))
        .filter(|(_, t)| t.abs() >= SUPPORT_SHARE * deepest)
        .collect();
    if template.is_empty() {
        return None;
    }
    let width = map.width as isize;
    let (min_x, max_x, min_y, max_y) = template.iter().fold(
        (usize::MAX, 0usize, usize::MAX, 0usize),
        |(a, b, c, d), (index, _)| {
            let (x, y) = (index % map.width, index / map.width);
            (a.min(x), b.max(x), c.min(y), d.max(y))
        },
    );
    let alpha = weight(&residual, &excluded, &template, 0)?;
    // Placebos: the same template moved by whole multiples of its own size,
    // clear of itself and inside the frame.
    let (span_x, span_y) = ((max_x - min_x + 1) as isize, (max_y - min_y + 1) as isize);
    let mut placebos = Vec::new();
    for gy in -4isize..=4 {
        for gx in -4isize..=4 {
            if gx == 0 && gy == 0 {
                continue;
            }
            let (dx, dy) = (gx * span_x, gy * span_y);
            let (x0, x1) = (min_x as isize + dx, max_x as isize + dx);
            let (y0, y1) = (min_y as isize + dy, max_y as isize + dy);
            if x0 < 0 || y0 < 0 || x1 >= width || y1 >= map.height as isize {
                continue;
            }
            if let Some(w) = weight(&residual, &excluded, &template, dy * width + dx) {
                placebos.push(w);
            }
        }
    }
    if placebos.len() < MIN_PLACEBOS {
        return Some(Fit {
            carried: alpha
                + if reference_side == Side::After {
                    1.0
                } else {
                    0.0
                },
            spread: f64::INFINITY,
            verdict: Verdict::Undecided,
        });
    }
    // Robust: a placebo that lands on a bright knot of nebula says how
    // that knot fits, not how the weight scatters. The placebos' middle is
    // where a fit sits with no change under it.
    let middle = median(placebos.clone());
    let spread = 1.4826 * median(placebos.iter().map(|w| (w - middle).abs()).collect());
    // The background over the reference: a weight of 0 means the lights
    // share the reference's side.
    let carried = alpha - middle
        + if reference_side == Side::After {
            1.0
        } else {
            0.0
        };
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
