//! How a stack preview integrates its frames: one server-wide choice, edited
//! on the Stacking settings page and recorded with every job and checkpoint.
//!
//! The defaults follow the Seiza stacking command line, which beat WBPP on
//! sharpness and signal-to-noise in Seiza's own comparison: local background
//! normalization, inverse-noise weights, a quadratic registration fit,
//! Lanczos-3 resampling, an automatically chosen reference, and the final
//! leave-one-out pass. [`StackMethod::classic`] is what PSF Guard did before
//! these options existed.

use serde::{Deserialize, Serialize};
use std::sync::{PoisonError, RwLock};

/// Tile edge for both local normalization modes, in reference pixels. The
/// Seiza command line's default.
pub const LOCAL_TILE_SIZE: usize = 256;

/// How frame backgrounds are matched to the reference.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackNormalization {
    /// One gain per channel and a smoothed grid of background offsets, so
    /// frames whose sky gradients differ leave no seam at their edges.
    #[default]
    LocalBackground,
    /// One gain and one offset per channel for the whole frame.
    Global,
    /// A gain and an offset per tile. Removes seams too, but a tile of cloud
    /// or nebula can push a frame's gains far enough to reject it.
    Local,
}

/// How much each admitted frame counts toward the mean.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackWeighting {
    /// Seiza's inverse-noise-variance weights: each frame's noise is
    /// measured after normalization, the reference weighs 1, and weights are
    /// clamped to 0.05..=20.
    #[default]
    Noise,
    /// Every admitted frame counts the same.
    Equal,
}

/// The transform registration fits from reference to frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackRegistration {
    /// A quadratic polynomial, which follows lens distortion across a
    /// meridian flip.
    #[default]
    Quadratic,
    /// A general linear map: shear and unequal scale as well.
    Affine,
    /// Shift, rotation and uniform scale only.
    Similarity,
}

/// How registration resamples each frame onto the reference grid.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackInterpolation {
    /// Lanczos-3, clamped against ringing: sharper stars, slower.
    #[default]
    Lanczos3,
    Bilinear,
}

/// Which frame sets the stack's grid and, under local background
/// normalization, the background every frame is matched to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackReference {
    /// Seiza's score from the frame's own stars and sky noise, preferring
    /// the flattest sky among frames close to the best.
    #[default]
    Auto,
    /// The frame with the best quality grade.
    BestGraded,
}

/// What follows the live pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackFinalPass {
    /// Read every admitted frame three more times and integrate them with
    /// leave-one-out rejection, so transients in the reference and the
    /// first few frames are rejected too.
    #[default]
    Reintegrate,
    /// Publish the live stack as it stands. Faster, but a satellite trail in
    /// one of the first frames can survive.
    Draft,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StackMethod {
    pub normalization: StackNormalization,
    pub weighting: StackWeighting,
    pub registration: StackRegistration,
    pub interpolation: StackInterpolation,
    /// Integrate a Bayer frame's photosites rather than its demosaiced
    /// pixels. Pays only when frames are dithered by several pixels.
    pub bayer_drizzle: bool,
    pub reference: StackReference,
    pub final_pass: StackFinalPass,
}

impl StackMethod {
    /// What PSF Guard did before the method could be chosen. Records written
    /// before it existed read as this, apart from their weighting.
    pub fn classic() -> Self {
        Self {
            normalization: StackNormalization::Global,
            weighting: StackWeighting::Equal,
            registration: StackRegistration::Similarity,
            interpolation: StackInterpolation::Bilinear,
            bayer_drizzle: false,
            reference: StackReference::BestGraded,
            final_pass: StackFinalPass::Reintegrate,
        }
    }

    /// Whether the live pass accumulates the same way under both methods,
    /// so a checkpoint from one can be extended by the other. Only the final
    /// pass runs after the accumulator is complete.
    pub fn same_accumulator(&self, other: &Self) -> bool {
        Self {
            final_pass: StackFinalPass::default(),
            ..*self
        } == Self {
            final_pass: StackFinalPass::default(),
            ..*other
        }
    }

    /// The Seiza options for this method's live pass, around the caller's
    /// impulse filter.
    pub fn stack_options(
        &self,
        cosmetic: Option<seiza_stacking::ImpulseFilterOptions>,
    ) -> seiza_stacking::StackOptions {
        let mut options = seiza_stacking::StackOptions {
            normalization: match self.normalization {
                StackNormalization::LocalBackground => {
                    seiza_stacking::NormalizationMode::LocalBackground {
                        tile_size: LOCAL_TILE_SIZE,
                    }
                }
                StackNormalization::Global => seiza_stacking::NormalizationMode::Global,
                StackNormalization::Local => seiza_stacking::NormalizationMode::Local {
                    tile_size: LOCAL_TILE_SIZE,
                },
            },
            cosmetic,
            weighting: self.weighting.frame_weighting(),
            interpolation: self.seiza_interpolation(),
            cfa_integration: if self.bayer_drizzle {
                seiza_stacking::CfaIntegration::BayerDrizzle
            } else {
                seiza_stacking::CfaIntegration::Demosaic
            },
            ..seiza_stacking::StackOptions::default()
        };
        options.registration.model = match self.registration {
            StackRegistration::Quadratic => seiza_stacking::RegistrationModel::Quadratic,
            StackRegistration::Affine => seiza_stacking::RegistrationModel::Affine,
            StackRegistration::Similarity => seiza_stacking::RegistrationModel::Similarity,
        };
        options
    }

    pub fn seiza_interpolation(&self) -> seiza_stacking::Interpolation {
        match self.interpolation {
            StackInterpolation::Lanczos3 => seiza_stacking::Interpolation::Lanczos3,
            StackInterpolation::Bilinear => seiza_stacking::Interpolation::Bilinear,
        }
    }

    /// Stable bytes for job ids.
    pub fn fingerprint(&self) -> String {
        serde_json::to_string(self).expect("a stacking method serializes")
    }
}

impl StackWeighting {
    pub fn is_equal(&self) -> bool {
        matches!(self, Self::Equal)
    }

    pub(crate) fn frame_weighting(self) -> seiza_stacking::FrameWeighting {
        match self {
            Self::Equal => seiza_stacking::FrameWeighting::Equal,
            Self::Noise => seiza_stacking::FrameWeighting::inverse_noise_variance(),
        }
    }
}

static CURRENT: RwLock<Option<StackMethod>> = RwLock::new(None);

/// Use `method` for every stack this process starts from now on.
pub fn configure(method: StackMethod) {
    *CURRENT.write().unwrap_or_else(PoisonError::into_inner) = Some(method);
}

/// The method new stacks use: the configured one, or the default.
pub fn current() -> StackMethod {
    CURRENT
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_follows_the_seiza_command_line() {
        let options = StackMethod::default().stack_options(None);
        assert_eq!(
            options.normalization,
            seiza_stacking::NormalizationMode::LocalBackground { tile_size: 256 }
        );
        assert_eq!(
            options.weighting,
            seiza_stacking::FrameWeighting::inverse_noise_variance()
        );
        assert_eq!(
            options.registration.model,
            seiza_stacking::RegistrationModel::Quadratic
        );
        assert_eq!(
            options.interpolation,
            seiza_stacking::Interpolation::Lanczos3
        );
        assert_eq!(
            options.cfa_integration,
            seiza_stacking::CfaIntegration::Demosaic
        );
        assert_eq!(StackMethod::default().reference, StackReference::Auto);
        assert_eq!(
            StackMethod::default().final_pass,
            StackFinalPass::Reintegrate
        );
    }

    #[test]
    fn classic_is_the_library_default_psf_guard_used_before() {
        let options = StackMethod::classic().stack_options(None);
        let library = seiza_stacking::StackOptions {
            normalization: seiza_stacking::NormalizationMode::Global,
            ..seiza_stacking::StackOptions::default()
        };
        assert_eq!(options.normalization, library.normalization);
        assert_eq!(options.weighting, library.weighting);
        assert_eq!(options.registration.model, library.registration.model);
        assert_eq!(options.interpolation, library.interpolation);
        assert_eq!(options.cfa_integration, library.cfa_integration);
    }

    #[test]
    fn a_partial_record_fills_from_the_default_and_names_are_snake_case() {
        let method: StackMethod =
            serde_json::from_str(r#"{"final_pass":"draft","normalization":"global"}"#).unwrap();
        assert_eq!(method.final_pass, StackFinalPass::Draft);
        assert_eq!(method.normalization, StackNormalization::Global);
        assert_eq!(method.registration, StackRegistration::Quadratic);
        let json = serde_json::to_value(StackMethod::default()).unwrap();
        assert_eq!(json["normalization"], "local_background");
        assert_eq!(json["reference"], "auto");
        assert_eq!(json["interpolation"], "lanczos3");
    }

    #[test]
    fn only_the_final_pass_leaves_the_accumulator_alone() {
        let full = StackMethod::default();
        let draft = StackMethod {
            final_pass: StackFinalPass::Draft,
            ..full
        };
        assert!(full.same_accumulator(&draft));
        assert_ne!(full.fingerprint(), draft.fingerprint());
        let equal = StackMethod {
            weighting: StackWeighting::Equal,
            ..full
        };
        assert!(!full.same_accumulator(&equal));
    }
}
