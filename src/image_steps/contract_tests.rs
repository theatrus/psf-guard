//! The region contract, shown on small operations: one that keeps it and
//! several that break it the way an unconverted step does.

use super::{apply_to_region, region_mismatch, Block, ImageShape, Region, RegionOperation};
use std::convert::Infallible;

const WIDTH: usize = 96;
const HEIGHT: usize = 64;

fn shape(channels: usize) -> ImageShape {
    ImageShape::new(WIDTH, HEIGHT, channels)
}

fn image(channels: usize) -> Vec<f32> {
    (0..WIDTH * HEIGHT * channels)
        .map(|index| {
            let pixel = index / channels;
            let (x, y) = ((pixel % WIDTH) as f32, (pixel / WIDTH) as f32);
            (x * 0.37).sin() * (y * 0.21).cos() + 0.01 * x + (index % channels) as f32
        })
        .collect()
}

/// A box blur of radius `radius`, repeated `passes` times. At the image's
/// edge it averages the pixels that exist.
struct BoxBlur {
    radius: usize,
    passes: usize,
    /// The reach it declares. The true reach is `radius * passes`.
    declared: usize,
}

impl RegionOperation for BoxBlur {
    type Error = Infallible;

    fn padding(&self) -> usize {
        self.declared
    }

    fn apply(&self, block: Block<'_>) -> Result<Vec<f32>, Infallible> {
        let ImageShape {
            width,
            height,
            channels,
        } = block.shape();
        let mut current = block.samples.to_vec();
        for _ in 0..self.passes {
            let mut next = vec![0.0; current.len()];
            for y in 0..height {
                for x in 0..width {
                    let rows = y.saturating_sub(self.radius)..(y + self.radius + 1).min(height);
                    let columns = x.saturating_sub(self.radius)..(x + self.radius + 1).min(width);
                    let count = (rows.len() * columns.len()) as f32;
                    for channel in 0..channels {
                        let mut sum = 0.0;
                        for row in rows.clone() {
                            for column in columns.clone() {
                                sum += current[(row * width + column) * channels + channel];
                            }
                        }
                        next[(y * width + x) * channels + channel] = sum / count;
                    }
                }
            }
            current = next;
        }
        Ok(current)
    }
}

/// Scales samples by the range of whatever it is given: the fault that makes
/// a crop preview differ from the committed result.
struct ScaleByOwnRange;

impl RegionOperation for ScaleByOwnRange {
    type Error = Infallible;

    fn padding(&self) -> usize {
        0
    }

    fn apply(&self, block: Block<'_>) -> Result<Vec<f32>, Infallible> {
        let low = block.samples.iter().copied().fold(f32::INFINITY, f32::min);
        let high = block
            .samples
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        Ok(block
            .samples
            .iter()
            .map(|value| (value - low) / (high - low))
            .collect())
    }
}

/// The same scaling with the range measured once from the whole image.
struct ScaleByImageRange {
    low: f32,
    high: f32,
}

impl RegionOperation for ScaleByImageRange {
    type Error = Infallible;

    fn padding(&self) -> usize {
        0
    }

    fn apply(&self, block: Block<'_>) -> Result<Vec<f32>, Infallible> {
        Ok(block
            .samples
            .iter()
            .map(|value| (value - self.low) / (self.high - self.low))
            .collect())
    }
}

/// Adds each sample's column and row in the image, as an operation that
/// depends on position does. `faulty` uses the position in the block.
struct AddPosition {
    faulty: bool,
}

impl RegionOperation for AddPosition {
    type Error = Infallible;

    fn padding(&self) -> usize {
        0
    }

    fn apply(&self, block: Block<'_>) -> Result<Vec<f32>, Infallible> {
        let shape = block.shape();
        let (origin_x, origin_y) = if self.faulty {
            (0, 0)
        } else {
            (block.region.x, block.region.y)
        };
        Ok(block
            .samples
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let pixel = index / shape.channels;
                let (x, y) = (
                    origin_x + pixel % shape.width,
                    origin_y + pixel / shape.width,
                );
                value + x as f32 + 1000.0 * y as f32
            })
            .collect())
    }
}

/// Changes one sample of the image, or makes it not finite, only when it is
/// given less than the whole image.
struct DisturbOneSample {
    at: (usize, usize, usize),
    to: f32,
}

impl RegionOperation for DisturbOneSample {
    type Error = Infallible;

    fn padding(&self) -> usize {
        0
    }

    fn apply(&self, block: Block<'_>) -> Result<Vec<f32>, Infallible> {
        let mut out = block.samples.to_vec();
        if block.region != block.image.region() {
            let (x, y, channel) = self.at;
            let shape = block.shape();
            let index = ((y - block.region.y) * shape.width + (x - block.region.x))
                * shape.channels
                + channel;
            out[index] = self.to;
        }
        Ok(out)
    }
}

fn regions() -> [Region; 6] {
    [
        // Inside the image.
        Region::new(30, 20, 24, 16),
        // On the left edge, and on the top edge.
        Region::new(0, 10, 20, 30),
        Region::new(40, 0, 30, 12),
        // Nearer to an edge than the padding, without touching it.
        Region::new(3, 2, 20, 20),
        // In the bottom right corner.
        Region::new(WIDTH - 17, HEIGHT - 9, 17, 9),
        Region::whole(WIDTH, HEIGHT),
    ]
}

#[test]
fn an_operation_with_its_true_reach_matches_the_whole_image() {
    for channels in [1, 3] {
        let image = image(channels);
        let blur = BoxBlur {
            radius: 2,
            passes: 3,
            declared: 6,
        };
        for region in regions() {
            let mismatch = region_mismatch(&blur, &image, shape(channels), region).unwrap();
            assert_eq!(mismatch.max_difference, 0.0, "{region:?}");
            assert_eq!(mismatch.at, None);
            assert_eq!(mismatch.non_finite_disagreements, 0);
            assert_eq!(mismatch.samples, region.width * region.height * channels);
        }
    }
}

#[test]
fn too_little_padding_shows_as_a_mismatch_inside_the_image() {
    let image = image(1);
    let blur = BoxBlur {
        radius: 2,
        passes: 3,
        declared: 2,
    };
    let inside = Region::new(30, 20, 24, 16);
    let mismatch = region_mismatch(&blur, &image, shape(1), inside).unwrap();
    assert!(mismatch.max_difference > 1e-3, "{mismatch:?}");
    // The whole image needs no padding, so it still matches itself.
    let mismatch = region_mismatch(&blur, &image, shape(1), shape(1).region()).unwrap();
    assert_eq!(mismatch.max_difference, 0.0);
}

#[test]
fn a_statistic_taken_from_the_block_shows_as_a_mismatch() {
    let image = image(1);
    let region = Region::new(30, 20, 24, 16);
    let mismatch = region_mismatch(&ScaleByOwnRange, &image, shape(1), region).unwrap();
    assert!(mismatch.max_difference > 0.05, "{mismatch:?}");

    let low = image.iter().copied().fold(f32::INFINITY, f32::min);
    let high = image.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let resolved = ScaleByImageRange { low, high };
    for region in regions() {
        let mismatch = region_mismatch(&resolved, &image, shape(1), region).unwrap();
        assert_eq!(mismatch.max_difference, 0.0, "{region:?}");
    }
}

#[test]
fn a_block_knows_where_it_lies_in_the_image() {
    let image = image(3);
    for region in regions() {
        let good = region_mismatch(&AddPosition { faulty: false }, &image, shape(3), region);
        assert_eq!(good.unwrap().max_difference, 0.0, "{region:?}");
    }
    let region = Region::new(30, 20, 24, 16);
    let bad = region_mismatch(&AddPosition { faulty: true }, &image, shape(3), region).unwrap();
    assert!(bad.max_difference >= 30.0, "{bad:?}");
}

#[test]
fn a_block_reports_which_sides_are_image_edges() {
    let image = image(1);
    let samples = Region::new(0, 10, 20, 54)
        .copy_from(&image, shape(1))
        .unwrap();
    let block = Block::new(&samples, Region::new(0, 10, 20, 54), shape(1)).unwrap();
    let edges = block.edges();
    assert!(edges.left && edges.bottom);
    assert!(!edges.top && !edges.right);
    assert!(Block::new(&samples, Region::new(0, 10, 20, 55), shape(1)).is_err());
    assert!(Block::new(&samples[1..], Region::new(0, 10, 20, 54), shape(1)).is_err());
}

#[test]
fn the_mismatch_names_the_sample_that_differs() {
    let image = image(3);
    let region = Region::new(30, 20, 24, 16);
    // Column, row and channel all differ, so a swap of any two shows.
    let at = (41, 27, 2);
    let disturb = DisturbOneSample { at, to: 500.0 };
    let mismatch = region_mismatch(&disturb, &image, shape(3), region).unwrap();
    assert_eq!(mismatch.at, Some(at));
    assert!(mismatch.max_difference > 400.0);
    assert_eq!(mismatch.non_finite_disagreements, 0);
}

#[test]
fn samples_that_are_not_finite_must_agree_too() {
    let mut image = image(1);
    image[25 * WIDTH + 35] = f32::NAN;
    let region = Region::new(30, 20, 24, 16);
    let blur = BoxBlur {
        radius: 1,
        passes: 1,
        declared: 1,
    };
    // The blur spreads the missing sample the same way in both results.
    let mismatch = region_mismatch(&blur, &image, shape(1), region).unwrap();
    assert_eq!(mismatch.non_finite_disagreements, 0);
    assert_eq!(mismatch.max_difference, 0.0);

    // A finite sample against one that is not, and two that differ in kind.
    let image = self::image(1);
    for to in [f32::NAN, f32::INFINITY] {
        let disturb = DisturbOneSample {
            at: (41, 27, 0),
            to,
        };
        let mismatch = region_mismatch(&disturb, &image, shape(1), region).unwrap();
        assert_eq!(mismatch.non_finite_disagreements, 1, "{to}");
        assert_eq!(mismatch.max_difference, 0.0);
    }
    let mut image = self::image(1);
    image[27 * WIDTH + 41] = f32::INFINITY;
    for (to, disagreements) in [(f32::INFINITY, 0), (f32::NEG_INFINITY, 1), (f32::NAN, 1)] {
        let disturb = DisturbOneSample {
            at: (41, 27, 0),
            to,
        };
        let mismatch = region_mismatch(&disturb, &image, shape(1), region).unwrap();
        assert_eq!(mismatch.non_finite_disagreements, disagreements, "{to}");
    }
}

#[test]
fn a_region_that_does_not_fit_is_refused_before_the_operation_runs() {
    let image = image(1);
    let outside = Region::new(WIDTH - 4, 0, 8, 8);
    assert!(apply_to_region(&ScaleByOwnRange, &image, shape(1), outside).is_err());
    let wrapped = Region::new(usize::MAX, 0, 2, 1);
    assert!(apply_to_region(&ScaleByOwnRange, &image, shape(1), wrapped).is_err());
    assert!(apply_to_region(&ScaleByOwnRange, &image, shape(3), shape(3).region()).is_err());
}
