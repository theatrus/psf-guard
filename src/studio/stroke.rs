use super::{LevelForm, MaskSource, RegionReader, Step, StudioError};
use crate::image_steps::{Accepts, Block, Mask, Region};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Whether a stroke adds to a mask or takes away from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StrokeMode {
    /// The stroke selects what it covers.
    Add,
    /// The stroke clears what it covers.
    Subtract,
}

/// One brush stroke, stored as the path the brush took.
///
/// Points and the radius are in document pixels at full resolution, with a
/// pixel's centre on a whole number. A stroke is therefore a few numbers
/// whatever the image's size, and is drawn again for any region at any zoom.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    /// The brush's path. One point is a dab.
    pub points: Vec<[f32; 2]>,
    /// The brush's radius.
    pub radius: f32,
    /// The share of the radius that is fully covered, from 0 to 1. The rest
    /// falls off smoothly to the edge.
    pub hardness: f32,
    /// Whether the stroke adds or subtracts.
    pub mode: StrokeMode,
}

/// The document coordinate of a level pixel's centre.
fn document_coordinate(pixel: usize, level: u8) -> f32 {
    let scale = (1u32 << level) as f32;
    pixel as f32 * scale + (scale - 1.0) / 2.0
}

impl Stroke {
    /// The level pixels the stroke can touch, grown by `extra` document
    /// pixels, as inclusive first and exclusive last columns and rows. May
    /// reach outside the image.
    fn reach(&self, level: u8, extra: f32) -> Option<[i64; 4]> {
        let first = self.points.first()?;
        let (mut low, mut high) = (*first, *first);
        for point in &self.points {
            low = [low[0].min(point[0]), low[1].min(point[1])];
            high = [high[0].max(point[0]), high[1].max(point[1])];
        }
        let scale = (1u32 << level) as f32;
        let reach = self.radius.max(0.0) + extra;
        let to_level = |document: f32| (document - (scale - 1.0) / 2.0) / scale;
        Some([
            to_level(low[0] - reach).floor() as i64,
            to_level(low[1] - reach).floor() as i64,
            to_level(high[0] + reach).ceil() as i64 + 1,
            to_level(high[1] + reach).ceil() as i64 + 1,
        ])
    }

    /// How much of a document point the stroke covers, from 0 to 1.
    fn coverage(&self, x: f32, y: f32) -> f32 {
        let mut nearest = f32::INFINITY;
        let mut previous = self.points[0];
        for point in &self.points {
            let (dx, dy) = (point[0] - previous[0], point[1] - previous[1]);
            let length = dx * dx + dy * dy;
            let along = if length > 0.0 {
                (((x - previous[0]) * dx + (y - previous[1]) * dy) / length).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (nx, ny) = (previous[0] + dx * along - x, previous[1] + dy * along - y);
            nearest = nearest.min(nx * nx + ny * ny);
            previous = *point;
        }
        let distance = nearest.sqrt();
        let inner = self.radius * self.hardness.clamp(0.0, 1.0);
        if distance <= inner {
            1.0
        } else if distance >= self.radius {
            0.0
        } else {
            let t = 1.0 - (distance - inner) / (self.radius - inner);
            t * t * (3.0 - 2.0 * t)
        }
    }

    /// Draws the stroke onto mask values for `region` at `level`.
    fn draw(&self, values: &mut [f32], region: Region, level: u8) {
        let Some(reach) = self.reach(level, 0.0) else {
            return;
        };
        let x0 = reach[0].max(region.x as i64);
        let y0 = reach[1].max(region.y as i64);
        let x1 = reach[2].min(region.right() as i64);
        let y1 = reach[3].min(region.bottom() as i64);
        for y in y0..y1 {
            let document_y = document_coordinate(y as usize, level);
            for x in x0..x1 {
                let covered = self.coverage(document_coordinate(x as usize, level), document_y);
                if covered == 0.0 {
                    continue;
                }
                let index = (y as usize - region.y) * region.width + (x as usize - region.x);
                let value = values[index];
                values[index] = match self.mode {
                    StrokeMode::Add => value + (1.0 - value) * covered,
                    StrokeMode::Subtract => value * (1.0 - covered),
                };
            }
        }
    }
}

/// A mask painted by hand: strokes applied in order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaintedMask {
    /// The strokes, oldest first.
    pub strokes: Vec<Stroke>,
}

impl MaskSource for PaintedMask {
    fn mask(&self, region: Region, level: u8) -> Result<Mask, StudioError> {
        let mut values = vec![0.0; region.width * region.height];
        for stroke in &self.strokes {
            stroke.draw(&mut values, region, level);
        }
        Ok(Mask::new(region.width, region.height, values)?)
    }
}

/// One stroke of the heal brush: the stroke's area is replaced by the area
/// `offset` away, shifted in brightness so the patch meets its surroundings.
pub struct Heal {
    stroke: Stroke,
    /// Where the replacement comes from, relative to each healed pixel, in
    /// pixels of this step's level.
    offset: [i64; 2],
    /// Added to each channel of the copied samples. Measured once from the
    /// whole image, so a crop and the whole image heal alike.
    shift: Vec<f32>,
    level: u8,
}

impl Heal {
    /// Makes a heal stroke over `reader`'s image, copying from `offset`
    /// document pixels away.
    ///
    /// The brightness shift is the difference between the surroundings of
    /// the stroke and the surroundings of the area it copies from. Only
    /// those two areas are read.
    pub fn resolve(
        reader: &dyn RegionReader,
        stroke: Stroke,
        offset: [i64; 2],
    ) -> Result<Self, StudioError> {
        let shape = reader.shape();
        let mut heal = Heal {
            stroke,
            offset,
            shift: vec![0.0; shape.channels],
            level: 0,
        };
        // The surroundings: a band from the stroke's edge to half a radius
        // beyond it.
        let band = heal.stroke.radius * 0.5 + 2.0;
        let Some(reach) = heal.stroke.reach(0, band) else {
            return Ok(heal);
        };
        // Keep only pixels whose source also lies inside the image.
        let clamp = |value: i64, limit: usize| value.clamp(0, limit as i64) as usize;
        let x0 = clamp(reach[0].max(-offset[0]), shape.width);
        let y0 = clamp(reach[1].max(-offset[1]), shape.height);
        let x1 = clamp(reach[2].min(shape.width as i64 - offset[0]), shape.width);
        let y1 = clamp(reach[3].min(shape.height as i64 - offset[1]), shape.height);
        if x0 >= x1 || y0 >= y1 {
            return Ok(heal);
        }
        let target = Region::new(x0, y0, x1 - x0, y1 - y0);
        let source = Region::new(
            (x0 as i64 + offset[0]) as usize,
            (y0 as i64 + offset[1]) as usize,
            target.width,
            target.height,
        );
        let wide = Stroke {
            radius: heal.stroke.radius + band,
            mode: StrokeMode::Add,
            ..heal.stroke.clone()
        };
        let inner = Stroke {
            hardness: 1.0,
            mode: StrokeMode::Add,
            ..heal.stroke.clone()
        };
        let target_samples = reader.read(target)?;
        let source_samples = reader.read(source)?;
        let mut sums = vec![0f64; shape.channels];
        let mut weight_sum = 0f64;
        for y in 0..target.height {
            let document_y = (target.y + y) as f32;
            for x in 0..target.width {
                let document_x = (target.x + x) as f32;
                let weight = f64::from(
                    (wide.coverage(document_x, document_y)
                        - inner.coverage(document_x, document_y))
                    .max(0.0),
                );
                if weight == 0.0 {
                    continue;
                }
                let index = (y * target.width + x) * shape.channels;
                let pixel = index..index + shape.channels;
                let finite = target_samples[pixel.clone()]
                    .iter()
                    .chain(&source_samples[pixel.clone()])
                    .all(|sample| sample.is_finite());
                if !finite {
                    continue;
                }
                for (channel, sum) in sums.iter_mut().enumerate() {
                    *sum += weight
                        * f64::from(
                            target_samples[index + channel] - source_samples[index + channel],
                        );
                }
                weight_sum += weight;
            }
        }
        if weight_sum > 0.0 {
            heal.shift = sums.iter().map(|sum| (sum / weight_sum) as f32).collect();
        }
        Ok(heal)
    }

    /// The brightness added to each channel of the copied samples.
    pub fn shift(&self) -> &[f32] {
        &self.shift
    }
}

impl Step for Heal {
    fn name(&self) -> &str {
        "Heal"
    }

    fn accepts(&self) -> Accepts {
        Accepts::Either
    }

    fn padding(&self) -> usize {
        self.offset[0]
            .unsigned_abs()
            .max(self.offset[1].unsigned_abs()) as usize
    }

    fn working_copies(&self) -> usize {
        1
    }

    fn at_level(&self, level: u8) -> LevelForm {
        let scale = 1i64 << level;
        // Round away from zero, so a small offset does not become "copy from
        // the same place".
        let scaled = |value: i64| {
            if value == 0 {
                0
            } else {
                value.signum() * ((value.abs() + scale - 1) / scale)
            }
        };
        LevelForm::Scaled(Arc::new(Heal {
            stroke: self.stroke.clone(),
            offset: [scaled(self.offset[0]), scaled(self.offset[1])],
            shift: self.shift.clone(),
            level,
        }))
    }

    fn apply(&self, block: Block<'_>) -> Result<Vec<f32>, StudioError> {
        let shape = block.shape();
        let mut out = block.samples.to_vec();
        let Some(reach) = self.stroke.reach(self.level, 0.0) else {
            return Ok(out);
        };
        let region = block.region;
        let x0 = reach[0].max(region.x as i64);
        let y0 = reach[1].max(region.y as i64);
        let x1 = reach[2].min(region.right() as i64);
        let y1 = reach[3].min(region.bottom() as i64);
        for y in y0..y1 {
            let document_y = document_coordinate(y as usize, self.level);
            for x in x0..x1 {
                let weight = self
                    .stroke
                    .coverage(document_coordinate(x as usize, self.level), document_y);
                if weight == 0.0 {
                    continue;
                }
                // The source must lie in this block. Outside the image there
                // is nothing to copy. Outside the block but inside the image
                // only happens in the block's padding, which is discarded.
                let (source_x, source_y) = (x + self.offset[0], y + self.offset[1]);
                let inside = source_x >= region.x as i64
                    && source_x < region.right() as i64
                    && source_y >= region.y as i64
                    && source_y < region.bottom() as i64;
                if !inside {
                    continue;
                }
                let target = ((y as usize - region.y) * shape.width + (x as usize - region.x))
                    * shape.channels;
                let source = ((source_y as usize - region.y) * shape.width
                    + (source_x as usize - region.x))
                    * shape.channels;
                for channel in 0..shape.channels {
                    let from = block.samples[target + channel];
                    let to = block.samples[source + channel] + self.shift[channel];
                    if !from.is_finite() || !to.is_finite() {
                        continue;
                    }
                    out[target + channel] = if weight == 1.0 {
                        to
                    } else {
                        from * (1.0 - weight) + to * weight
                    };
                }
            }
        }
        Ok(out)
    }
}
