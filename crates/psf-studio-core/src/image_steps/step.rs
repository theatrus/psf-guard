use super::{ImageShape, Region, RegionError};
use thiserror::Error;

/// A rectangular block of an image, handed to an operation.
///
/// Make one with [`Block::new`]. More may be added to a block later, such as
/// a row stride or a validity mask, without breaking an operation that reads
/// what is here.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Block<'a> {
    /// The block's samples: row-major, channel-interleaved.
    pub samples: &'a [f32],
    /// Where the block lies in the image. An operation whose effect depends
    /// on position, such as a point-spread function that varies across the
    /// field or a background model, reads the position from here.
    pub region: Region,
    /// The shape of the whole image the block was cut from.
    pub image: ImageShape,
}

/// Which sides of a block lie on the image's own edges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    /// The block's first column is the image's first column.
    pub left: bool,
    /// The block's first row is the image's first row.
    pub top: bool,
    /// The block's last column is the image's last column.
    pub right: bool,
    /// The block's last row is the image's last row.
    pub bottom: bool,
}

impl<'a> Block<'a> {
    /// A block of `image`, with the samples of `region`.
    pub fn new(samples: &'a [f32], region: Region, image: ImageShape) -> Result<Self, RegionError> {
        region.check(image)?;
        let expected = region.width * region.height * image.channels;
        if samples.len() != expected {
            return Err(RegionError::SampleCount {
                what: "the block",
                expected,
                given: samples.len(),
            });
        }
        Ok(Self {
            samples,
            region,
            image,
        })
    }

    /// The shape of the block's own samples.
    pub fn shape(&self) -> ImageShape {
        ImageShape::new(self.region.width, self.region.height, self.image.channels)
    }

    /// Which sides of the block are edges of the image. On the other sides
    /// the image continues past the block.
    pub fn edges(&self) -> Edges {
        Edges {
            left: self.region.x == 0,
            top: self.region.y == 0,
            right: self.region.right() == self.image.width,
            bottom: self.region.bottom() == self.image.height,
        }
    }
}

/// An operation that is resolved once from a whole image and can then be
/// applied to any part of it.
///
/// A host shows a full-resolution crop while a person moves a slider, works
/// through a mosaic in tiles, and caches each result by region. All three
/// need the same guarantee: **the result for a region matches the same
/// pixels of the result for the whole image.** [`region_mismatch`] measures
/// it.
///
/// To keep the guarantee, an implementation:
///
/// - holds every value it measured from the image, such as a sample range, a
///   noise level or a fitted model. `apply` must not measure such a value
///   from the block it is given, because a block is not the image;
/// - does not scale its result by a value that only the finished whole-image
///   result gives, such as a ratio of total flux before and after. A crop
///   cannot know that value. Measure it another way, or leave it out;
/// - states in `padding` how far a pixel's result depends on its neighbors;
/// - handles a side of the block as the image's edge only where
///   [`Block::edges`] says it is one. A side inside the image needs no
///   special handling: `apply_to_region` gives the block `padding` more
///   pixels there and discards them from the result.
///
/// The trait covers applying. Resolving, diagnostics, progress and
/// cancellation stay on the step's own functions, which differ from step to
/// step. An operation returns samples of the block's shape, so a step that
/// changes an image's size, such as resampling or binning, is not one.
pub trait RegionOperation {
    /// Why the operation could not be applied.
    type Error: std::error::Error + 'static;

    /// How many pixels of context a block needs on each side for the result
    /// inside it to match the whole image.
    ///
    /// Return 0 for an operation that maps each pixel alone. For an exact
    /// match this is the operation's full reach, which for an iterative
    /// operation grows with every iteration. An operation whose influence
    /// falls off quickly may return less. It then matches within a
    /// tolerance, and must state that tolerance and test it with
    /// [`region_mismatch`].
    fn padding(&self) -> usize;

    /// Applies the operation to a block and returns samples of the same
    /// shape. Within `padding` pixels of a side that is not an image edge,
    /// the result may differ from the whole-image result.
    fn apply(&self, block: Block<'_>) -> Result<Vec<f32>, Self::Error>;
}

/// Why an operation could not be applied to a region.
#[derive(Clone, Debug, PartialEq, Error)]
pub enum RegionApplyError<E: std::error::Error + 'static> {
    /// The region does not fit the image.
    #[error(transparent)]
    Region(#[from] RegionError),
    /// The operation refused.
    #[error("the operation could not be applied")]
    Operation(#[source] E),
    /// The operation returned a block of another size.
    #[error("the operation returned {given} samples for a block of {expected}")]
    ResultSize {
        /// The number of samples in the block it was given.
        expected: usize,
        /// The number of samples it returned.
        given: usize,
    },
}

/// Applies an operation to one region of an image and returns that region's
/// samples.
///
/// The operation is given the region grown by its padding, cut off at the
/// image's edges, and the padding is removed from what it returns.
pub fn apply_to_region<O: RegionOperation>(
    operation: &O,
    image: &[f32],
    shape: ImageShape,
    region: Region,
) -> Result<Vec<f32>, RegionApplyError<O::Error>> {
    region.check_buffer(image.len(), shape)?;
    let padded = region.padded(operation.padding(), shape.width, shape.height);
    let samples = padded.copy_from(image, shape)?;
    let block = Block::new(&samples, padded, shape)?;
    let result = operation
        .apply(block)
        .map_err(RegionApplyError::Operation)?;
    if result.len() != samples.len() {
        return Err(RegionApplyError::ResultSize {
            expected: samples.len(),
            given: result.len(),
        });
    }
    if padded == region {
        return Ok(result);
    }
    let inner = region
        .relative_to(&padded)
        .expect("a padded region contains its region");
    Ok(inner.copy_from(&result, block.shape())?)
}

/// How far a region's result is from the whole image's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionMismatch {
    /// The largest absolute difference between a finite sample of the
    /// region's result and the same sample of the whole-image result.
    pub max_difference: f32,
    /// The column, row and channel of that sample, in image coordinates.
    /// `None` when no finite sample differs.
    pub at: Option<(usize, usize, usize)>,
    /// How many samples are not finite in at least one result and are not
    /// the same in both. A NaN matches a NaN, and an infinity matches the
    /// same infinity; every other pairing is counted here.
    pub non_finite_disagreements: usize,
    /// The number of samples compared.
    pub samples: usize,
}

/// Measures whether an operation keeps the [`RegionOperation`] guarantee for
/// one region: applies it to the whole image and to the region, and compares
/// the two over the region.
///
/// Every implementation of [`RegionOperation`] should be tested with this, on
/// regions inside the image, near its edges, on its edges and in its
/// corners.
pub fn region_mismatch<O: RegionOperation>(
    operation: &O,
    image: &[f32],
    shape: ImageShape,
    region: Region,
) -> Result<RegionMismatch, RegionApplyError<O::Error>> {
    let part = apply_to_region(operation, image, shape, region)?;
    let reference = apply_to_region(operation, image, shape, shape.region())?;
    let reference = region.copy_from(&reference, shape)?;

    let mut mismatch = RegionMismatch {
        max_difference: 0.0,
        at: None,
        non_finite_disagreements: 0,
        samples: part.len(),
    };
    for (index, (&a, &b)) in part.iter().zip(&reference).enumerate() {
        if !a.is_finite() || !b.is_finite() {
            let same = (a.is_nan() && b.is_nan()) || a == b;
            if !same {
                mismatch.non_finite_disagreements += 1;
            }
            continue;
        }
        let difference = (a - b).abs();
        if difference > mismatch.max_difference {
            let pixel = index / shape.channels;
            mismatch.max_difference = difference;
            mismatch.at = Some((
                region.x + pixel % region.width,
                region.y + pixel / region.width,
                index % shape.channels,
            ));
        }
    }
    Ok(mismatch)
}
