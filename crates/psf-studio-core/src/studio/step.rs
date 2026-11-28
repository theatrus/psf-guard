use super::source::region_bytes;
use super::{RegionReader, StudioError};
use crate::image_steps::{Accepts, Block, Domain, ImageShape, Mask, Region};
use std::sync::Arc;

/// What a step becomes at a reduced pyramid level.
pub enum LevelForm {
    /// The step maps each pixel alone, so it runs unchanged.
    Same,
    /// The step runs with its distances scaled to the level.
    Scaled(Arc<dyn Step>),
    /// The step's result depends on pixel scale, so a reduced view leaves it
    /// out. Show it on a full-resolution crop.
    Omitted,
}

/// One operation of a document, resolved and ready to apply to any block.
///
/// A step keeps the region contract of
/// [`RegionOperation`](crate::image_steps::RegionOperation): it holds what
/// it measured from the whole image and measures nothing from a block.
pub trait Step: Send + Sync {
    /// The step's name, as a person reads it.
    fn name(&self) -> &str;

    /// The samples the step works on.
    fn accepts(&self) -> Accepts;

    /// The domain of the step's result for an input in `input`.
    fn output(&self, input: Domain) -> Domain {
        input
    }

    /// The pixels of context a block needs on each side.
    fn padding(&self) -> usize;

    /// How many buffers of a block's size the step holds while it runs,
    /// besides its input and its result.
    fn working_copies(&self) -> usize {
        0
    }

    /// The step's form at a pyramid level above 0.
    fn at_level(&self, level: u8) -> LevelForm;

    /// Applies the step to a block and returns samples of the same shape.
    fn apply(&self, block: Block<'_>) -> Result<Vec<f32>, StudioError>;
}

/// A step applied to a reader. It is a reader itself, so steps and stages
/// stack, and a read asks each one only for the block it needs.
pub struct Applied {
    source: Arc<dyn RegionReader>,
    step: Arc<dyn Step>,
    domain: Domain,
}

impl Applied {
    /// Applies `step` to `source`. Refuses a step that does not work on the
    /// source's samples.
    pub fn new(source: Arc<dyn RegionReader>, step: Arc<dyn Step>) -> Result<Self, StudioError> {
        step.accepts().check(step.name(), source.domain())?;
        let domain = step.output(source.domain());
        Ok(Self {
            source,
            step,
            domain,
        })
    }

    fn padded(&self, region: Region) -> Region {
        let shape = self.source.shape();
        region.padded(self.step.padding(), shape.width, shape.height)
    }
}

impl RegionReader for Applied {
    fn shape(&self) -> ImageShape {
        self.source.shape()
    }

    fn domain(&self) -> Domain {
        self.domain
    }

    fn peak_bytes(&self, region: Region) -> u64 {
        let padded = self.padded(region);
        // The source's own peak is over before this step allocates. While
        // the step runs it holds its input, its result and its working
        // copies.
        let block = region_bytes(padded, self.shape().channels);
        let running = block.saturating_mul(2 + self.step.working_copies() as u64);
        self.source.peak_bytes(padded).max(running)
    }

    fn read(&self, region: Region) -> Result<Vec<f32>, StudioError> {
        let shape = self.shape();
        region.check(shape)?;
        let padded = self.padded(region);
        let samples = self.source.read(padded)?;
        let block = Block::new(&samples, padded, shape)?;
        let result = self.step.apply(block)?;
        if result.len() != samples.len() {
            return Err(StudioError::Step {
                step: self.step.name().to_string(),
                reason: format!(
                    "it returned {} samples for a block of {}",
                    result.len(),
                    samples.len()
                ),
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
}

/// Where a mask comes from. A mask is made for the block a step is working
/// on, never for the whole image.
pub trait MaskSource: Send + Sync {
    /// The mask over `region`, at pyramid `level`. `region` is in that
    /// level's pixels.
    fn mask(&self, region: Region, level: u8) -> Result<Mask, StudioError>;
}

/// A step that takes effect only where a mask allows.
pub struct Masked {
    step: Arc<dyn Step>,
    mask: Arc<dyn MaskSource>,
    level: u8,
}

impl Masked {
    /// Limits `step` to `mask`, at full resolution.
    pub fn new(step: Arc<dyn Step>, mask: Arc<dyn MaskSource>) -> Self {
        Self {
            step,
            mask,
            level: 0,
        }
    }
}

impl Step for Masked {
    fn name(&self) -> &str {
        self.step.name()
    }

    fn accepts(&self) -> Accepts {
        self.step.accepts()
    }

    fn output(&self, input: Domain) -> Domain {
        self.step.output(input)
    }

    fn padding(&self) -> usize {
        self.step.padding()
    }

    fn working_copies(&self) -> usize {
        // The mask is one plane, at most one more buffer of the block's size.
        self.step.working_copies() + 1
    }

    fn at_level(&self, level: u8) -> LevelForm {
        let step = match self.step.at_level(level) {
            LevelForm::Same => self.step.clone(),
            LevelForm::Scaled(step) => step,
            LevelForm::Omitted => return LevelForm::Omitted,
        };
        LevelForm::Scaled(Arc::new(Masked {
            step,
            mask: self.mask.clone(),
            level,
        }))
    }

    fn apply(&self, block: Block<'_>) -> Result<Vec<f32>, StudioError> {
        let changed = self.step.apply(block)?;
        let mask = self.mask.mask(block.region, self.level)?;
        Ok(mask.blend(block.samples, &changed, block.image.channels)?)
    }
}
