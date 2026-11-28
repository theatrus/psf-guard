use super::{ImageShape, Region};
use thiserror::Error;

/// One plane of values in `[0, 1]`, the size of an image, that limits where
/// a step takes effect. 1 gives the step its full effect at a pixel and 0
/// leaves the pixel as it was.
///
/// A mask has one value per pixel whatever the image's channel count.
#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    width: usize,
    height: usize,
    values: Vec<f32>,
}

/// Why a mask could not be made or used.
#[derive(Clone, Debug, PartialEq, Error)]
pub enum MaskError {
    /// A buffer does not hold the samples its dimensions call for.
    #[error("{what} has {given} samples; {expected} expected")]
    SampleCount {
        /// The buffer that has the wrong length.
        what: &'static str,
        /// The number of samples the dimensions call for.
        expected: usize,
        /// The number of samples the buffer holds.
        given: usize,
    },
    /// A mask value is not a number from 0 to 1.
    #[error("the mask value {value} at index {index} is outside 0 to 1")]
    Value {
        /// The position of the value, in row-major order.
        index: usize,
        /// The value that was refused.
        value: f32,
    },
    /// Two masks differ in size.
    #[error("a {width} x {height} mask cannot combine with a {other_width} x {other_height} mask")]
    Size {
        /// This mask's width.
        width: usize,
        /// This mask's height.
        height: usize,
        /// The other mask's width.
        other_width: usize,
        /// The other mask's height.
        other_height: usize,
    },
    /// A range's bounds are out of order or not finite, or its feather is
    /// negative or not finite.
    #[error("a range needs finite bounds with low <= high, and a finite feather of 0 or more")]
    Range,
    /// A region is empty or reaches outside the mask.
    #[error("region {region:?} does not fit in a {width} x {height} mask")]
    Region {
        /// The region that was asked for.
        region: Region,
        /// The mask's width.
        width: usize,
        /// The mask's height.
        height: usize,
    },
    /// A mask or an image has no pixels, an image has no channels, or the
    /// sample count does not fit in a `usize`.
    #[error(
        "a {width} x {height} plane with {channels} channel(s) cannot hold a mask or an image"
    )]
    Shape {
        /// The width that was given.
        width: usize,
        /// The height that was given.
        height: usize,
        /// The channel count that was given: 1 for a mask.
        channels: usize,
    },
}

/// The number of values a mask of this size holds.
fn mask_len(width: usize, height: usize) -> Result<usize, MaskError> {
    ImageShape::new(width, height, 1)
        .samples()
        .ok_or(MaskError::Shape {
            width,
            height,
            channels: 1,
        })
}

/// The sample values a range mask selects.
///
/// A pixel between `low` and `high` gets 1. Outside them the mask falls to 0
/// over `feather`, in the same units as the samples. A feather of 0 gives a
/// hard edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaskRange {
    /// The lowest fully selected value.
    pub low: f32,
    /// The highest fully selected value.
    pub high: f32,
    /// How far outside `low` and `high` the mask reaches 0.
    pub feather: f32,
}

impl Mask {
    /// A mask from its values, one per pixel, in row-major order.
    pub fn new(width: usize, height: usize, values: Vec<f32>) -> Result<Self, MaskError> {
        let expected = mask_len(width, height)?;
        if values.len() != expected {
            return Err(MaskError::SampleCount {
                what: "the mask",
                expected,
                given: values.len(),
            });
        }
        if let Some((index, &value)) = values
            .iter()
            .enumerate()
            .find(|(_, value)| !(0.0..=1.0).contains(*value))
        {
            return Err(MaskError::Value { index, value });
        }
        Ok(Self {
            width,
            height,
            values,
        })
    }

    /// A mask with the same value, from 0 to 1, at every pixel.
    pub fn filled(width: usize, height: usize, value: f32) -> Result<Self, MaskError> {
        let len = mask_len(width, height)?;
        if !(0.0..=1.0).contains(&value) {
            return Err(MaskError::Value { index: 0, value });
        }
        Ok(Self {
            width,
            height,
            values: vec![value; len],
        })
    }

    /// A mask that selects pixels by their value.
    ///
    /// `samples` holds the row-major, interleaved samples `shape` describes.
    /// A pixel's value is the mean of its channels. A pixel with a sample
    /// that is not finite gets 0.
    pub fn from_range(
        samples: &[f32],
        shape: ImageShape,
        range: MaskRange,
    ) -> Result<Self, MaskError> {
        let (width, height, channels) = (shape.width, shape.height, shape.channels);
        let expected = shape.samples().ok_or(MaskError::Shape {
            width,
            height,
            channels,
        })?;
        if samples.len() != expected {
            return Err(MaskError::SampleCount {
                what: "the image",
                expected,
                given: samples.len(),
            });
        }
        let finite = range.low.is_finite() && range.high.is_finite() && range.feather.is_finite();
        if !finite || range.low > range.high || range.feather < 0.0 {
            return Err(MaskError::Range);
        }
        let values = samples
            .chunks_exact(channels)
            .map(|pixel| {
                let value = pixel.iter().sum::<f32>() / channels as f32;
                if !value.is_finite() {
                    return 0.0;
                }
                let outside = (range.low - value).max(value - range.high).max(0.0);
                if outside == 0.0 {
                    1.0
                } else if outside >= range.feather {
                    0.0
                } else {
                    // A smooth step, so the mask's edge has no corner.
                    let t = 1.0 - outside / range.feather;
                    t * t * (3.0 - 2.0 * t)
                }
            })
            .collect();
        Ok(Self {
            width,
            height,
            values,
        })
    }

    /// The mask's width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// The mask's height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// The mask's values, one per pixel, in row-major order.
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// The mean of the mask's values: the share of the image it covers.
    pub fn coverage(&self) -> f64 {
        self.values
            .iter()
            .map(|&value| f64::from(value))
            .sum::<f64>()
            / self.values.len() as f64
    }

    /// The mask that selects what this one leaves out.
    pub fn inverted(&self) -> Mask {
        Mask {
            width: self.width,
            height: self.height,
            values: self.values.iter().map(|value| 1.0 - value).collect(),
        }
    }

    /// The mask that selects what either mask selects: the greater value at
    /// each pixel.
    pub fn union(&self, other: &Mask) -> Result<Mask, MaskError> {
        self.combine(other, f32::max)
    }

    /// The mask that selects what both masks select: the lesser value at
    /// each pixel.
    pub fn intersection(&self, other: &Mask) -> Result<Mask, MaskError> {
        self.combine(other, f32::min)
    }

    /// The part of the mask that covers a region of the image.
    pub fn region(&self, region: Region) -> Result<Mask, MaskError> {
        if region.is_empty() || !region.fits(self.width, self.height) {
            return Err(MaskError::Region {
                region,
                width: self.width,
                height: self.height,
            });
        }
        let mut values = Vec::with_capacity(region.width * region.height);
        for y in region.y..region.bottom() {
            let start = y * self.width + region.x;
            values.extend_from_slice(&self.values[start..start + region.width]);
        }
        Ok(Mask {
            width: region.width,
            height: region.height,
            values,
        })
    }

    /// Applies a step's result through the mask: exactly `original` where
    /// the mask is 0, exactly `changed` where it is 1, and a mix in between.
    ///
    /// Both buffers hold `channels` interleaved samples per pixel for an
    /// image of the mask's size. A sample that is not finite in `original`
    /// stays as it is, so a registration border is kept. Where the mask is 0
    /// a sample of `changed` that is not finite has no effect.
    pub fn blend(
        &self,
        original: &[f32],
        changed: &[f32],
        channels: usize,
    ) -> Result<Vec<f32>, MaskError> {
        let expected = ImageShape::new(self.width, self.height, channels)
            .samples()
            .ok_or(MaskError::Shape {
                width: self.width,
                height: self.height,
                channels,
            })?;
        for (what, given) in [
            ("the original image", original.len()),
            ("the changed image", changed.len()),
        ] {
            if given != expected {
                return Err(MaskError::SampleCount {
                    what,
                    expected,
                    given,
                });
            }
        }
        let mut out = Vec::with_capacity(expected);
        for ((original, changed), &weight) in original
            .chunks_exact(channels)
            .zip(changed.chunks_exact(channels))
            .zip(&self.values)
        {
            for (&from, &to) in original.iter().zip(changed) {
                out.push(if !from.is_finite() || weight == 0.0 {
                    from
                } else if weight == 1.0 {
                    to
                } else {
                    // Weighting each side, not the difference, keeps a small
                    // value from being lost beside a large one.
                    from * (1.0 - weight) + to * weight
                });
            }
        }
        Ok(out)
    }

    fn combine(&self, other: &Mask, pick: fn(f32, f32) -> f32) -> Result<Mask, MaskError> {
        if (self.width, self.height) != (other.width, other.height) {
            return Err(MaskError::Size {
                width: self.width,
                height: self.height,
                other_width: other.width,
                other_height: other.height,
            });
        }
        Ok(Mask {
            width: self.width,
            height: self.height,
            values: self
                .values
                .iter()
                .zip(&other.values)
                .map(|(&a, &b)| pick(a, b))
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_outside_zero_to_one_are_refused() {
        assert!(Mask::new(2, 1, vec![0.0, 1.0]).is_ok());
        assert_eq!(
            Mask::new(2, 1, vec![0.5, 1.5]),
            Err(MaskError::Value {
                index: 1,
                value: 1.5
            })
        );
        assert!(matches!(
            Mask::new(2, 1, vec![f32::NAN, 0.0]),
            Err(MaskError::Value { index: 0, .. })
        ));
        assert!(matches!(
            Mask::new(2, 2, vec![0.0; 3]),
            Err(MaskError::SampleCount { .. })
        ));
        assert!(matches!(
            Mask::filled(2, 2, 1.5),
            Err(MaskError::Value { .. })
        ));
        assert!(matches!(
            Mask::filled(2, 2, f32::NAN),
            Err(MaskError::Value { .. })
        ));
    }

    #[test]
    fn a_size_that_cannot_be_counted_is_refused() {
        let huge = usize::MAX / 2;
        assert!(matches!(
            Mask::new(huge, 4, vec![]),
            Err(MaskError::Shape { .. })
        ));
        assert!(matches!(
            Mask::filled(0, 4, 1.0),
            Err(MaskError::Shape { .. })
        ));
        let mask = Mask::filled(2, 1, 1.0).unwrap();
        assert!(matches!(
            mask.blend(&[], &[], 0),
            Err(MaskError::Shape { .. })
        ));
    }

    #[test]
    fn a_range_selects_by_value_with_a_feathered_edge() {
        let samples = [0.0, 0.15, 0.2, 0.5, 0.8, 0.9, 1.0, f32::NAN];
        let range = MaskRange {
            low: 0.2,
            high: 0.8,
            feather: 0.1,
        };
        let mask = Mask::from_range(&samples, ImageShape::new(8, 1, 1), range).unwrap();
        let values = mask.values();
        assert_eq!(values[0], 0.0);
        // Halfway through the feather, a smooth step is at one half.
        assert!((values[1] - 0.5).abs() < 1e-5);
        assert_eq!(&values[2..5], &[1.0, 1.0, 1.0]);
        assert!(values[5].abs() < 1e-5);
        assert_eq!(values[6], 0.0);
        assert_eq!(values[7], 0.0);
    }

    #[test]
    fn a_range_must_be_ordered_and_finite() {
        let shape = ImageShape::new(1, 1, 1);
        let range = |low, high, feather| MaskRange { low, high, feather };
        for bad in [
            range(0.5, 0.2, 0.0),
            range(0.0, 1.0, -0.1),
            range(0.0, 1.0, f32::INFINITY),
            range(f32::NAN, 1.0, 0.0),
            range(0.0, f32::INFINITY, 0.0),
        ] {
            assert_eq!(Mask::from_range(&[0.5], shape, bad), Err(MaskError::Range));
        }
    }

    #[test]
    fn a_range_uses_the_mean_of_a_pixels_channels() {
        let samples = [0.2, 0.4, 0.6, 0.9, 0.9, 0.9];
        let range = MaskRange {
            low: 0.3,
            high: 0.5,
            feather: 0.0,
        };
        let mask = Mask::from_range(&samples, ImageShape::new(2, 1, 3), range).unwrap();
        assert_eq!(mask.values(), &[1.0, 0.0]);
    }

    #[test]
    fn masks_combine_and_invert() {
        let a = Mask::new(3, 1, vec![0.0, 0.5, 1.0]).unwrap();
        let b = Mask::new(3, 1, vec![1.0, 0.25, 0.5]).unwrap();
        assert_eq!(a.union(&b).unwrap().values(), &[1.0, 0.5, 1.0]);
        assert_eq!(a.intersection(&b).unwrap().values(), &[0.0, 0.25, 0.5]);
        assert_eq!(a.inverted().values(), &[1.0, 0.5, 0.0]);
        assert_eq!(a.coverage(), 0.5);
        let other = Mask::filled(2, 1, 1.0).unwrap();
        assert!(matches!(a.union(&other), Err(MaskError::Size { .. })));
    }

    #[test]
    fn a_blend_follows_the_mask_and_keeps_missing_samples() {
        let mask = Mask::new(4, 1, vec![0.0, 0.5, 1.0, 1.0]).unwrap();
        let original = [1.0, 1.0, 1.0, f32::NAN];
        let changed = [3.0, 3.0, 3.0, 3.0];
        let out = mask.blend(&original, &changed, 1).unwrap();
        assert_eq!(&out[..3], &[1.0, 2.0, 3.0]);
        assert!(out[3].is_nan());
    }

    #[test]
    fn a_blend_is_exact_where_the_mask_is_zero_or_one() {
        let mask = Mask::new(4, 1, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
        // A protected pixel is untouched by a result that is not finite, and
        // a fully selected one takes the result exactly, however far apart
        // the two values are.
        let original = [1.0, 1.0, 65535.0, 1.0e8];
        let changed = [f32::NAN, f32::INFINITY, 0.001, 1.0];
        let out = mask.blend(&original, &changed, 1).unwrap();
        assert_eq!(out, [1.0, 1.0, 0.001, 1.0]);
    }

    #[test]
    fn a_blend_weights_every_channel_of_a_pixel_alike() {
        let mask = Mask::new(2, 1, vec![0.0, 1.0]).unwrap();
        let original = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let changed = [9.0; 6];
        let out = mask.blend(&original, &changed, 3).unwrap();
        assert_eq!(out, [1.0, 2.0, 3.0, 9.0, 9.0, 9.0]);
    }

    #[test]
    fn a_region_of_a_mask_matches_the_image_region() {
        let mask = Mask::new(3, 2, vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5]).unwrap();
        let part = mask.region(Region::new(1, 0, 2, 2)).unwrap();
        assert_eq!(part.values(), &[0.1, 0.2, 0.4, 0.5]);
        assert!(matches!(
            mask.region(Region::new(2, 0, 2, 1)),
            Err(MaskError::Region { .. })
        ));
        assert!(matches!(
            mask.region(Region::new(usize::MAX, 0, 2, 1)),
            Err(MaskError::Region { .. })
        ));
    }
}
