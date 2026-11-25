use super::{Applied, LevelForm, RegionReader, Step, StudioError};
use std::sync::Arc;

/// The pyramid level to show at a zoom, where zoom is screen pixels per
/// document pixel. Level 0 is full resolution and each level halves it. The
/// level chosen never has fewer pixels than the screen shows.
pub fn level_for_zoom(zoom: f64, levels: u8) -> u8 {
    if zoom.is_nan() || zoom <= 0.0 {
        return levels.saturating_sub(1);
    }
    let level = (1.0 / zoom).log2().floor();
    level.clamp(0.0, f64::from(levels.saturating_sub(1))) as u8
}

/// An ordered list of steps.
#[derive(Clone, Default)]
pub struct Pipeline {
    steps: Vec<Arc<dyn Step>>,
}

/// A pipeline built for a reduced pyramid level.
pub struct PreviewChain {
    /// Reads the view.
    pub reader: Arc<dyn RegionReader>,
    /// The names of the steps the view leaves out, because their result
    /// depends on pixel scale. An empty list means the view shows every
    /// step.
    pub omitted: Vec<String>,
}

impl Pipeline {
    /// A pipeline of `steps`, applied in order.
    pub fn new(steps: Vec<Arc<dyn Step>>) -> Self {
        Self { steps }
    }

    /// The steps, in order.
    pub fn steps(&self) -> &[Arc<dyn Step>] {
        &self.steps
    }

    /// The pixels of context the whole pipeline needs on each side.
    pub fn padding(&self) -> usize {
        self.steps.iter().map(|step| step.padding()).sum()
    }

    /// The pipeline applied to `source` at full resolution: the committed
    /// result, and what a 1:1 view shows.
    pub fn over(
        &self,
        source: Arc<dyn RegionReader>,
    ) -> Result<Arc<dyn RegionReader>, StudioError> {
        let mut reader = source;
        for step in &self.steps {
            reader = Arc::new(Applied::new(reader, step.clone())?);
        }
        Ok(reader)
    }

    /// The pipeline applied to `source`, which holds the image at pyramid
    /// `level`. Level 0 is the same as [`Self::over`].
    pub fn over_level(
        &self,
        source: Arc<dyn RegionReader>,
        level: u8,
    ) -> Result<PreviewChain, StudioError> {
        let mut reader = source;
        let mut omitted = Vec::new();
        for step in &self.steps {
            let step = if level == 0 {
                step.clone()
            } else {
                match step.at_level(level) {
                    LevelForm::Same => step.clone(),
                    LevelForm::Scaled(scaled) => scaled,
                    LevelForm::Omitted => {
                        // The samples keep the domain they had, which the
                        // step would have kept or changed. A step that
                        // changes the domain is never omitted.
                        omitted.push(step.name().to_string());
                        continue;
                    }
                }
            };
            reader = Arc::new(Applied::new(reader, step)?);
        }
        Ok(PreviewChain { reader, omitted })
    }
}
