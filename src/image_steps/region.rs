use super::ImageShape;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A rectangle of an image, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Region {
    /// The first column.
    pub x: usize,
    /// The first row.
    pub y: usize,
    /// The number of columns.
    pub width: usize,
    /// The number of rows.
    pub height: usize,
}

/// Why a region could not be used with an image.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum RegionError {
    /// The region reaches outside the image.
    #[error("region {region:?} does not fit in a {} x {} image", shape.width, shape.height)]
    OutOfBounds {
        /// The region that was asked for.
        region: Region,
        /// The image it was asked of.
        shape: ImageShape,
    },
    /// A buffer does not hold the samples its shape calls for.
    #[error("{what} has {given} samples; {expected} expected")]
    SampleCount {
        /// The buffer that has the wrong length.
        what: &'static str,
        /// The number of samples the shape calls for.
        expected: usize,
        /// The number of samples the buffer holds.
        given: usize,
    },
    /// An image or a region has no pixels, an image has no channels, or an
    /// image's sample count does not fit in a `usize`.
    #[error("{shape:?} is not the shape of an image, or the region is empty")]
    Shape {
        /// The shape that was given.
        shape: ImageShape,
    },
}

impl Region {
    /// A region from its first column and row and its size.
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The region that covers a whole image.
    pub fn whole(width: usize, height: usize) -> Self {
        Self::new(0, 0, width, height)
    }

    /// The column after the last one. A region that would run past
    /// `usize::MAX` ends there.
    pub fn right(&self) -> usize {
        self.x.saturating_add(self.width)
    }

    /// The row after the last one. A region that would run past `usize::MAX`
    /// ends there.
    pub fn bottom(&self) -> usize {
        self.y.saturating_add(self.height)
    }

    /// Whether the region has no pixels.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Whether the region lies inside an image of this size.
    pub fn fits(&self, width: usize, height: usize) -> bool {
        self.x <= width
            && self.width <= width - self.x
            && self.y <= height
            && self.height <= height - self.y
    }

    /// The part two regions share, if they overlap.
    pub fn intersection(&self, other: &Region) -> Option<Region> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (x < right && y < bottom).then(|| Region::new(x, y, right - x, bottom - y))
    }

    /// The region grown by `padding` pixels on every side and cut off at the
    /// edges of a `width` by `height` image.
    ///
    /// Where the region is nearer to an edge than `padding`, the result ends
    /// at that edge. A step then meets the image's edge in the padded block
    /// exactly where it meets it in the whole image.
    pub fn padded(&self, padding: usize, width: usize, height: usize) -> Region {
        let x = self.x.saturating_sub(padding).min(width);
        let y = self.y.saturating_sub(padding).min(height);
        let right = self.right().saturating_add(padding).min(width);
        let bottom = self.bottom().saturating_add(padding).min(height);
        Region::new(x, y, right.saturating_sub(x), bottom.saturating_sub(y))
    }

    /// This region's position inside `outer`, when `outer` contains it.
    pub fn relative_to(&self, outer: &Region) -> Option<Region> {
        (self.x >= outer.x
            && self.y >= outer.y
            && self.right() <= outer.right()
            && self.bottom() <= outer.bottom())
        .then(|| Region::new(self.x - outer.x, self.y - outer.y, self.width, self.height))
    }

    /// Checks that the region is not empty and lies inside an image of this
    /// shape, and returns the number of samples the image holds.
    pub fn check(&self, shape: ImageShape) -> Result<usize, RegionError> {
        let samples = shape.samples().ok_or(RegionError::Shape { shape })?;
        if self.is_empty() {
            return Err(RegionError::Shape { shape });
        }
        if !self.fits(shape.width, shape.height) {
            return Err(RegionError::OutOfBounds {
                region: *self,
                shape,
            });
        }
        Ok(samples)
    }

    /// Copies the region's samples out of an image.
    ///
    /// `image` holds the row-major, interleaved samples `shape` describes.
    /// The result has the same layout for the region's size.
    pub fn copy_from(&self, image: &[f32], shape: ImageShape) -> Result<Vec<f32>, RegionError> {
        self.check_buffer(image.len(), shape)?;
        let row = self.width * shape.channels;
        let mut samples = Vec::with_capacity(row * self.height);
        for y in self.y..self.bottom() {
            let start = (y * shape.width + self.x) * shape.channels;
            samples.extend_from_slice(&image[start..start + row]);
        }
        Ok(samples)
    }

    /// Writes samples for the region into an image, replacing what is there.
    ///
    /// `samples` holds the region's samples, with the image's channel count.
    pub fn paste_into(
        &self,
        samples: &[f32],
        image: &mut [f32],
        shape: ImageShape,
    ) -> Result<(), RegionError> {
        self.check_buffer(image.len(), shape)?;
        let row = self.width * shape.channels;
        if samples.len() != row * self.height {
            return Err(RegionError::SampleCount {
                what: "the region",
                expected: row * self.height,
                given: samples.len(),
            });
        }
        for (index, source) in samples.chunks_exact(row).enumerate() {
            let start = ((self.y + index) * shape.width + self.x) * shape.channels;
            image[start..start + row].copy_from_slice(source);
        }
        Ok(())
    }

    pub(crate) fn check_buffer(&self, given: usize, shape: ImageShape) -> Result<(), RegionError> {
        let expected = self.check(shape)?;
        if given != expected {
            return Err(RegionError::SampleCount {
                what: "the image",
                expected,
                given,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(shape: ImageShape) -> Vec<f32> {
        (0..shape.samples().unwrap()).map(|i| i as f32).collect()
    }

    #[test]
    fn padding_stops_at_the_image_edge() {
        // Not square, so a width used for a height shows.
        let (width, height) = (100, 60);
        let region = Region::new(2, 5, 10, 10);
        assert_eq!(region.padded(4, width, height), Region::new(0, 1, 16, 18));
        assert_eq!(
            Region::new(90, 52, 10, 5).padded(8, width, height),
            Region::new(82, 44, 18, 16)
        );
        assert_eq!(region.padded(0, width, height), region);
        assert_eq!(
            region.padded(usize::MAX, width, height),
            Region::whole(width, height)
        );
    }

    #[test]
    fn a_region_is_found_inside_its_padded_block() {
        let region = Region::new(2, 5, 10, 10);
        let padded = region.padded(4, 100, 60);
        assert_eq!(region.relative_to(&padded), Some(Region::new(2, 4, 10, 10)));
        assert_eq!(padded.relative_to(&region), None);
    }

    #[test]
    fn regions_intersect_only_where_they_overlap() {
        let a = Region::new(0, 0, 10, 10);
        assert_eq!(
            a.intersection(&Region::new(5, 8, 10, 10)),
            Some(Region::new(5, 8, 5, 2))
        );
        assert_eq!(a.intersection(&Region::new(10, 0, 4, 4)), None);
    }

    #[test]
    fn copy_then_paste_round_trips_interleaved_samples() {
        let shape = ImageShape::new(7, 5, 3);
        let source = image(shape);
        let region = Region::new(2, 1, 4, 3);
        let samples = region.copy_from(&source, shape).unwrap();
        assert_eq!(samples.len(), 4 * 3 * 3);
        // The first sample is row 1, column 2, channel 0.
        assert_eq!(samples[0], ((shape.width + 2) * shape.channels) as f32);

        let mut target = vec![0.0; source.len()];
        region.paste_into(&samples, &mut target, shape).unwrap();
        for y in 0..shape.height {
            for x in 0..shape.width {
                let inside = (2..6).contains(&x) && (1..4).contains(&y);
                for c in 0..shape.channels {
                    let index = (y * shape.width + x) * shape.channels + c;
                    let expected = if inside { source[index] } else { 0.0 };
                    assert_eq!(target[index], expected);
                }
            }
        }
    }

    #[test]
    fn a_region_outside_the_image_is_refused() {
        let shape = ImageShape::new(4, 4, 1);
        let source = image(shape);
        assert_eq!(
            Region::new(2, 2, 3, 1).copy_from(&source, shape),
            Err(RegionError::OutOfBounds {
                region: Region::new(2, 2, 3, 1),
                shape,
            })
        );
        assert_eq!(
            Region::new(0, 0, 2, 2).copy_from(&source, ImageShape::new(4, 4, 3)),
            Err(RegionError::SampleCount {
                what: "the image",
                expected: 48,
                given: 16,
            })
        );
        assert_eq!(
            Region::new(0, 0, 0, 2).copy_from(&source, shape),
            Err(RegionError::Shape { shape })
        );
    }

    #[test]
    fn a_region_whose_end_overflows_is_refused_not_wrapped() {
        let shape = ImageShape::new(4, 4, 1);
        let source = image(shape);
        // `x + width` wraps to 1 in unchecked arithmetic, which would pass a
        // bounds check.
        let wrapped = Region::new(usize::MAX, 0, 2, 1);
        assert!(!wrapped.fits(4, 4));
        assert_eq!(
            wrapped.copy_from(&source, shape),
            Err(RegionError::OutOfBounds {
                region: wrapped,
                shape,
            })
        );
        assert!(wrapped.padded(1, 4, 4).is_empty());
        assert_eq!(wrapped.intersection(&Region::whole(4, 4)), None);
    }

    #[test]
    fn a_shape_too_large_to_count_is_refused() {
        let shape = ImageShape::new(usize::MAX / 2, 4, 1);
        assert_eq!(shape.samples(), None);
        assert_eq!(
            Region::new(0, 0, 1, 1).copy_from(&[], shape),
            Err(RegionError::Shape { shape })
        );
    }
}
