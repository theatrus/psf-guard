//! Small steps for the scaffolding's tests, one of each kind a document has.

use super::{LevelForm, Step, StudioError};
use crate::image_steps::{Accepts, Block, Domain};
use std::sync::Arc;

/// Multiplies every sample: a step that maps each pixel alone.
pub(super) struct Gain(pub f32);

impl Step for Gain {
    fn name(&self) -> &str {
        "Gain"
    }
    fn accepts(&self) -> Accepts {
        Accepts::Either
    }
    fn padding(&self) -> usize {
        0
    }
    fn at_level(&self, _: u8) -> LevelForm {
        LevelForm::Same
    }
    fn apply(&self, block: Block<'_>) -> Result<Vec<f32>, StudioError> {
        Ok(block.samples.iter().map(|sample| sample * self.0).collect())
    }
}

/// A box blur: a step with a radius, which scales with the pyramid level.
pub(super) struct BoxBlur {
    pub radius: usize,
}

impl Step for BoxBlur {
    fn name(&self) -> &str {
        "Box blur"
    }
    fn accepts(&self) -> Accepts {
        Accepts::Either
    }
    fn padding(&self) -> usize {
        self.radius
    }
    fn working_copies(&self) -> usize {
        0
    }
    fn at_level(&self, level: u8) -> LevelForm {
        LevelForm::Scaled(Arc::new(BoxBlur {
            radius: self.radius >> level,
        }))
    }
    fn apply(&self, block: Block<'_>) -> Result<Vec<f32>, StudioError> {
        let shape = block.shape();
        let mut out = vec![0.0; block.samples.len()];
        for y in 0..shape.height {
            for x in 0..shape.width {
                let rows = y.saturating_sub(self.radius)..(y + self.radius + 1).min(shape.height);
                let columns = x.saturating_sub(self.radius)..(x + self.radius + 1).min(shape.width);
                let count = (rows.len() * columns.len()) as f32;
                for channel in 0..shape.channels {
                    let mut sum = 0.0;
                    for row in rows.clone() {
                        for column in columns.clone() {
                            sum += block.samples
                                [(row * shape.width + column) * shape.channels + channel];
                        }
                    }
                    out[(y * shape.width + x) * shape.channels + channel] = sum / count;
                }
            }
        }
        Ok(out)
    }
}

/// Stands in for deconvolution: linear only, many working copies, and left
/// out of a reduced view.
pub(super) struct Sharpen;

impl Step for Sharpen {
    fn name(&self) -> &str {
        "Sharpen"
    }
    fn accepts(&self) -> Accepts {
        Accepts::Linear
    }
    fn padding(&self) -> usize {
        8
    }
    fn working_copies(&self) -> usize {
        8
    }
    fn at_level(&self, _: u8) -> LevelForm {
        LevelForm::Omitted
    }
    fn apply(&self, block: Block<'_>) -> Result<Vec<f32>, StudioError> {
        Ok(block.samples.iter().map(|sample| sample + 1.0).collect())
    }
}

/// Stands in for a stretch: takes linear samples and gives display ones.
pub(super) struct Stretch;

impl Step for Stretch {
    fn name(&self) -> &str {
        "Stretch"
    }
    fn accepts(&self) -> Accepts {
        Accepts::Linear
    }
    fn output(&self, _: Domain) -> Domain {
        Domain::Display
    }
    fn padding(&self) -> usize {
        0
    }
    fn at_level(&self, _: u8) -> LevelForm {
        LevelForm::Same
    }
    fn apply(&self, block: Block<'_>) -> Result<Vec<f32>, StudioError> {
        Ok(block.samples.iter().map(|sample| sample.sqrt()).collect())
    }
}
