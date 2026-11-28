use super::Region;
use serde::{Deserialize, Serialize};

/// The width, height and channel count of row-major, channel-interleaved
/// samples.
///
/// This describes a buffer a caller owns. It is not an image type: Seiza's
/// steps keep taking a slice with its shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImageShape {
    /// Columns.
    pub width: usize,
    /// Rows.
    pub height: usize,
    /// Interleaved channels per pixel: 1 for mono, 3 for RGB.
    pub channels: usize,
}

impl ImageShape {
    /// A shape from its width, height and channel count.
    pub fn new(width: usize, height: usize, channels: usize) -> Self {
        Self {
            width,
            height,
            channels,
        }
    }

    /// The number of pixels, or `None` when a dimension is zero or the count
    /// does not fit in a `usize`.
    pub fn pixels(self) -> Option<usize> {
        self.width
            .checked_mul(self.height)
            .filter(|&pixels| pixels > 0)
    }

    /// The number of samples a buffer of this shape holds, or `None` when a
    /// dimension is zero or the count does not fit in a `usize`.
    pub fn samples(self) -> Option<usize> {
        self.pixels()?
            .checked_mul(self.channels)
            .filter(|&samples| samples > 0)
    }

    /// The region that covers the whole image.
    pub fn region(self) -> Region {
        Region::whole(self.width, self.height)
    }
}
