use super::StudioError;
use crate::image_steps::{Domain, ImageShape, Region};

/// Something that gives the samples of any region of an image: a source
/// file, a cached result, an edited file, or a chain of steps.
///
/// Nothing in the scaffolding asks for a whole image unless the caller's
/// region is the whole image, so a reader never has to hold one.
pub trait RegionReader: Send + Sync {
    /// The shape of the whole image.
    fn shape(&self) -> ImageShape;

    /// Whether the samples are linear or display-referred.
    fn domain(&self) -> Domain;

    /// The most bytes of sample memory that reading `region` needs at any
    /// moment, including the result. A caller checks this against its budget
    /// before it reads.
    fn peak_bytes(&self, region: Region) -> u64;

    /// The samples of `region`: row-major, channel-interleaved.
    fn read(&self, region: Region) -> Result<Vec<f32>, StudioError>;
}

/// The bytes that the samples of a region take.
pub(super) fn region_bytes(region: Region, channels: usize) -> u64 {
    (region.width as u64)
        .saturating_mul(region.height as u64)
        .saturating_mul(channels as u64)
        .saturating_mul(size_of::<f32>() as u64)
}

/// An image held in memory. For tests, small sources and results that are
/// already loaded.
pub struct MemoryImage {
    shape: ImageShape,
    domain: Domain,
    samples: Vec<f32>,
}

impl MemoryImage {
    /// An image from its samples.
    pub fn new(shape: ImageShape, domain: Domain, samples: Vec<f32>) -> Result<Self, StudioError> {
        shape.region().copy_from(&samples, shape)?;
        Ok(Self {
            shape,
            domain,
            samples,
        })
    }

    /// The image at one pyramid level: each pixel is the mean of the
    /// `2^level` square of pixels it covers. A pixel at the right or bottom
    /// edge covers what is left.
    pub fn reduced(&self, level: u8) -> MemoryImage {
        let factor = 1usize << level;
        let shape = ImageShape::new(
            self.shape.width.div_ceil(factor),
            self.shape.height.div_ceil(factor),
            self.shape.channels,
        );
        let mut samples = Vec::with_capacity(shape.width * shape.height * shape.channels);
        for y in 0..shape.height {
            for x in 0..shape.width {
                let rows = y * factor..((y + 1) * factor).min(self.shape.height);
                let columns = x * factor..((x + 1) * factor).min(self.shape.width);
                let count = (rows.len() * columns.len()) as f32;
                for channel in 0..shape.channels {
                    let mut sum = 0.0;
                    for row in rows.clone() {
                        for column in columns.clone() {
                            let index = (row * self.shape.width + column) * shape.channels;
                            sum += self.samples[index + channel];
                        }
                    }
                    samples.push(sum / count);
                }
            }
        }
        MemoryImage {
            shape,
            domain: self.domain,
            samples,
        }
    }
}

impl RegionReader for MemoryImage {
    fn shape(&self) -> ImageShape {
        self.shape
    }

    fn domain(&self) -> Domain {
        self.domain
    }

    fn peak_bytes(&self, region: Region) -> u64 {
        region_bytes(region, self.shape.channels)
    }

    fn read(&self, region: Region) -> Result<Vec<f32>, StudioError> {
        Ok(region.copy_from(&self.samples, self.shape)?)
    }
}
