//! Healing: matching a heal's brightness to its surroundings, and what the
//! canvas shows of where a heal copies from.
//!
//! A heal copies the pixels `offset` away over its stroke. Copied as they
//! are, they show as a patch where the source is brighter or darker than
//! the place healed. When a stroke is made, Studio renders the picture the
//! heal reads around it on the CPU, as an export does, and measures a ring
//! just outside the stroke and the same ring at the source. The difference
//! of their means, per channel, is the heal's shift, which the compositor
//! adds to what it copies.

use super::panes::scene;
use super::Studio;
use crate::store_tiles::StoreSource;
use crate::studio_model::{LayerSource, Place, Row, Stroke};
use seiza_compose::picture::Segment;
use seiza_compose::render::{render_image, RenderOptions};
use seiza_compose::ReferenceRegion;
use uuid::Uuid;

/// The ring a heal is matched on: from the stroke's edge out to this many
/// radii, and at least a few pixels.
const RING_RADII: f32 = 1.5;
const RING_MIN_WIDTH: f32 = 4.0;

/// The distance from `point` to the stroke's path.
fn distance_to(points: &[[f32; 2]], point: [f32; 2]) -> f32 {
    let segment = |from: [f32; 2], to: [f32; 2]| {
        let delta = [to[0] - from[0], to[1] - from[1]];
        let length = delta[0] * delta[0] + delta[1] * delta[1];
        let along = if length > 1e-6 {
            (((point[0] - from[0]) * delta[0] + (point[1] - from[1]) * delta[1]) / length)
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        let nearest = [from[0] + delta[0] * along, from[1] + delta[1] * along];
        ((point[0] - nearest[0]).powi(2) + (point[1] - nearest[1]).powi(2)).sqrt()
    };
    match points {
        [] => f32::INFINITY,
        [only] => segment(*only, *only),
        _ => points
            .windows(2)
            .map(|pair| segment(pair[0], pair[1]))
            .fold(f32::INFINITY, f32::min),
    }
}

/// The part of the picture a heal's measurement reads: the stroke and its
/// source, each with the ring around it, kept inside `size`.
pub(super) fn measured_region(
    stroke: &Stroke,
    offset: [i64; 2],
    size: (u32, u32),
) -> Option<ReferenceRegion> {
    let reach = stroke.radius * (1.0 + RING_RADII) + RING_MIN_WIDTH + 2.0;
    let (mut low, mut high) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for point in &stroke.points {
        for shifted in [
            *point,
            [point[0] + offset[0] as f32, point[1] + offset[1] as f32],
        ] {
            for axis in 0..2 {
                low[axis] = low[axis].min(shifted[axis] - reach);
                high[axis] = high[axis].max(shifted[axis] + reach);
            }
        }
    }
    let x = low[0].max(0.0).floor() as usize;
    let y = low[1].max(0.0).floor() as usize;
    let right = (high[0].ceil().max(0.0) as usize).min(size.0 as usize);
    let bottom = (high[1].ceil().max(0.0) as usize).min(size.1 as usize);
    (right > x && bottom > y).then_some(ReferenceRegion {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}

/// The shift that matches a heal to its surroundings: per channel, the mean
/// of the ring around the stroke less the mean of the same ring around its
/// source, in `samples`, the picture over `region` with `channels` channels
/// interleaved. `None` when either ring has no finite pixel inside it.
pub(super) fn matching_shift(
    stroke: &Stroke,
    offset: [i64; 2],
    samples: &[f32],
    channels: usize,
    region: ReferenceRegion,
) -> Option<Vec<f32>> {
    let inner = stroke.radius;
    let outer = stroke.radius * RING_RADII + RING_MIN_WIDTH;
    let mut sums = vec![[0.0_f64; 2]; channels];
    let mut counts = [0_usize; 2];
    let at = |x: i64, y: i64| -> Option<&[f32]> {
        let (x, y) = (x - region.x as i64, y - region.y as i64);
        if x < 0 || y < 0 || x >= region.width as i64 || y >= region.height as i64 {
            return None;
        }
        let start = (y as usize * region.width + x as usize) * channels;
        let pixel = samples.get(start..start + channels)?;
        pixel.iter().all(|value| value.is_finite()).then_some(pixel)
    };
    for row in 0..region.height {
        for column in 0..region.width {
            let (x, y) = ((region.x + column) as i64, (region.y + row) as i64);
            let distance = distance_to(&stroke.points, [x as f32 + 0.5, y as f32 + 0.5]);
            if distance <= inner || distance > outer {
                continue;
            }
            // The ring around the stroke, and the same ring at its source.
            let (Some(here), Some(there)) = (at(x, y), at(x + offset[0], y + offset[1])) else {
                continue;
            };
            for (side, pixel) in [here, there].into_iter().enumerate() {
                for (sum, value) in sums.iter_mut().zip(pixel) {
                    sum[side] += f64::from(*value);
                }
                counts[side] += 1;
            }
        }
    }
    (counts[0] > 0 && counts[1] > 0).then(|| {
        sums.iter()
            .map(|[here, there]| (here / counts[0] as f64 - there / counts[1] as f64) as f32)
            .collect()
    })
}

/// A cross over `at`, as overlay segments: where an Option-click set the
/// next heal's source.
pub(super) fn source_cross(at: [f64; 2], size: f32, line: f32) -> Vec<Segment> {
    let [x, y] = at.map(|value| value as f32);
    [
        ([x - size, y], [x + size, y]),
        ([x, y - size], [x, y + size]),
    ]
    .into_iter()
    .map(|(from, to)| Segment {
        from,
        to,
        radius: line,
        hardness: 1.0,
        value: 1.0,
    })
    .collect()
}

impl Studio {
    /// The place whose picture a heal added where the selection is reads,
    /// with the channels a heal there shifts: a layer alone in gray, or the
    /// layers up to a place in color. `None` for a heal of an input, which
    /// copies linear samples, or of a group.
    pub(super) fn heal_reads(&self) -> Option<Place> {
        let after = |layer: Uuid| {
            self.layer(layer)
                .and_then(|layer| layer.steps.last())
                .map(|step| step.id)
        };
        match &self.selected {
            Row::Layer(id) | Row::LayerStep { layer: id, .. } => {
                let layer = self.layer(*id)?;
                match &layer.source {
                    LayerSource::Group => None,
                    source if source.shows_pixels() => Some(Place::LayerEdit {
                        layer: *id,
                        after: after(*id),
                    }),
                    // Right above an adjustment: the layers up to it.
                    _ => Some(Place::Layer {
                        layer: *id,
                        alone: false,
                    }),
                }
            }
            Row::Combined | Row::Finish(_) | Row::Final | Row::Mask(_) => {
                let top = self.document.layers().last()?;
                Some(Place::Layer {
                    layer: top.id,
                    alone: false,
                })
            }
            Row::Input(_) | Row::InputStep { .. } => None,
        }
    }

    /// The shift that matches a heal of `stroke` from `offset` away, where
    /// the selection is, to its surroundings: measured on the picture the
    /// heal reads, rendered on the CPU. Empty when it cannot be measured.
    pub(super) fn measure_heal_shift(&mut self, stroke: &Stroke, offset: [i64; 2]) -> Vec<f32> {
        let started = std::time::Instant::now();
        let shift = (|| {
            let place = self.heal_reads()?;
            let store = self.store_path()?.to_path_buf();
            let size = self.canvas.read(|shared| shared.canvas.document_size);
            let region = measured_region(stroke, offset, size)?;
            let view_stretch = self.view_stretch_stages();
            let (mut drawn, states) = scene(
                &self.document,
                &self.band_info,
                &place.view(),
                self.scene_version,
                self.picture_samples
                    .as_ref()
                    .map(|(id, samples)| (*id, samples.as_slice())),
                &mut self.resolved_chains,
                &view_stretch,
            );
            drawn.stretches = self.analysis(&states);
            let picture = drawn.picture(&[]);
            let (tiles, _) = crate::store_tiles::open_store(&store).ok()?;
            let source = StoreSource::new(tiles).with_planes(self.derived.clone());
            let image = render_image(&picture, &source, region, &RenderOptions::default()).ok()?;
            let mut shift = matching_shift(stroke, offset, &image.data, image.channels, region)?;
            // A band layer alone is drawn in gray: its heal shifts the one
            // value. A stars layer's heal works in color.
            if let Place::LayerEdit { layer, .. } = place
                && self
                    .layer(layer)
                    .is_some_and(|layer| matches!(layer.source, LayerSource::Band { .. }))
            {
                shift.truncate(1);
            }
            Some(shift)
        })();
        if super::timing() {
            eprintln!(
                "heal shift {shift:?} in {:.1} ms",
                started.elapsed().as_secs_f64() * 1e3
            );
        }
        shift.unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio_model::StrokeMode;

    #[test]
    fn a_heal_is_shifted_by_the_difference_of_the_rings_around_it() {
        let stroke = Stroke {
            points: vec![[20.0, 20.0]],
            radius: 4.0,
            hardness: 0.6,
            mode: StrokeMode::Add,
        };
        let region = ReferenceRegion {
            x: 0,
            y: 0,
            width: 60,
            height: 40,
        };
        // Brighter by 0.25 to the left of x = 30, where the stroke is; the
        // source is 20 pixels to the right.
        let samples: Vec<f32> = (0..region.height)
            .flat_map(|_| (0..region.width).map(|x| if x < 30 { 0.5 } else { 0.25 }))
            .flat_map(|value| [value, value * 2.0, value])
            .collect();
        let shift = matching_shift(&stroke, [20, 0], &samples, 3, region).unwrap();
        for (got, wanted) in shift.iter().zip([0.25, 0.5, 0.25]) {
            assert!((got - wanted).abs() < 1e-6, "{shift:?}");
        }
        // Copied from where it is as bright, there is nothing to match.
        let level = matching_shift(&stroke, [0, 12], &samples, 3, region).unwrap();
        assert!(level.iter().all(|value| value.abs() < 1e-6), "{level:?}");
        let measured = measured_region(&stroke, [20, 0], (60, 40)).unwrap();
        // The stroke, its source 20 pixels right, and the rings around both.
        assert!(
            measured.x <= 20 - 10 && measured.x + measured.width >= 40 + 10,
            "{measured:?}"
        );
    }
}
