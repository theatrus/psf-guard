//! Which of a stack's passes run on the GPU: one server-wide choice, edited
//! on the Stacking settings page and read when each build starts.
//!
//! Seiza runs debayering, star detection, resampling, the live accumulator
//! and the final pass's rejection on the GPU through wgpu (Metal, Vulkan or
//! Direct3D 12), for frames of a million samples or more. A machine with no
//! GPU falls back to the CPU. After a GPU error, Seiza keeps every pass on
//! the CPU until the process restarts. The GPU works in single precision, so
//! its stacks differ from the CPU's by rounding. The choice is therefore not
//! part of a stack's method or fingerprint, and changing it does not make
//! built stacks out of date.
//!
//! Every Seiza call that can run on the GPU follows the choice: the live
//! pass, a stack reopened from its checkpoint, the final pass's frame
//! preparation, resampling and integration, artifact searches, and color
//! previews' channel alignment.

use serde::{Deserialize, Serialize};
use std::sync::{PoisonError, RwLock};

/// How many frame calls may run on the GPU at once by default: Seiza's
/// default. Fewer left an integrated GPU idle while a call copied; more
/// raised peak memory.
pub const DEFAULT_GPU_CONCURRENCY: u32 = 3;

/// The most frame calls the settings allow on the GPU at once.
pub const MAX_GPU_CONCURRENCY: u32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StackGpu {
    /// `false` keeps every pass on the CPU and never opens the device.
    pub enabled: bool,
    /// Debayer a Bayer frame by VNG.
    pub debayer: bool,
    /// Detect each frame's stars for registration, and the stars that align
    /// a color preview's channels.
    pub detection: bool,
    /// Resample each frame onto the reference grid, a finished stack onto
    /// the north-up grid, and a color preview's channels onto its reference
    /// channel.
    pub resampling: bool,
    /// Keep the live stack's running mean on the GPU, and normalize and
    /// admit each frame there.
    pub accumulator: bool,
    /// Run the final pass's rejection and integration.
    pub integration: bool,
    /// How many frame calls may run on the GPU at once; the others wait
    /// without using GPU memory.
    pub concurrency: u32,
    /// Log each GPU call's stages and times to the server's standard error.
    pub trace: bool,
}

impl Default for StackGpu {
    fn default() -> Self {
        Self {
            enabled: true,
            debayer: true,
            detection: true,
            resampling: true,
            accumulator: true,
            integration: true,
            concurrency: DEFAULT_GPU_CONCURRENCY,
            trace: false,
        }
    }
}

impl StackGpu {
    /// Refuse a concurrency Seiza cannot use or the settings do not offer.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_GPU_CONCURRENCY).contains(&self.concurrency) {
            return Err(format!(
                "the GPU's concurrency must be between 1 and {MAX_GPU_CONCURRENCY}"
            ));
        }
        Ok(())
    }

    /// This choice with its frame count inside the offered range, for a
    /// value read from a registry edited by hand: saving any other change
    /// would otherwise be refused until the count was fixed.
    pub fn clamped(self) -> Self {
        Self {
            concurrency: self.concurrency.clamp(1, MAX_GPU_CONCURRENCY),
            ..self
        }
    }

    /// The Seiza options for a build, a color composition or an artifact
    /// search. PSF Guard learns no defect map, so
    /// Seiza's defect screen follows the main switch.
    pub fn options(&self) -> seiza_stacking::GpuOptions {
        seiza_stacking::GpuOptions {
            enabled: self.enabled,
            defects: true,
            debayer: self.debayer,
            detection: self.detection,
            resampling: self.resampling,
            accumulator: self.accumulator,
            integration: self.integration,
            concurrency: self.clamped().concurrency as usize,
            memory_bytes: None,
            trace: self.trace,
        }
    }
}

static CURRENT: RwLock<Option<StackGpu>> = RwLock::new(None);

/// Use `gpu` for every build that starts from now on.
pub fn configure(gpu: StackGpu) {
    *CURRENT.write().unwrap_or_else(PoisonError::into_inner) = Some(gpu);
}

/// What builds starting now use: the configured choice, or the default.
pub fn current() -> StackGpu {
    CURRENT
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_seiza_s_and_runs_every_pass_on_the_gpu() {
        assert_eq!(
            StackGpu::default().options(),
            seiza_stacking::GpuOptions::default()
        );
    }

    #[test]
    fn off_keeps_every_pass_on_the_cpu() {
        let off = StackGpu {
            enabled: false,
            ..StackGpu::default()
        };
        assert_eq!(off.options(), seiza_stacking::GpuOptions::cpu());
    }

    #[test]
    fn each_pass_maps_to_its_own_switch() {
        let chosen = StackGpu {
            debayer: false,
            accumulator: false,
            concurrency: 2,
            trace: true,
            ..StackGpu::default()
        };
        assert_eq!(
            chosen.options(),
            seiza_stacking::GpuOptions {
                debayer: false,
                accumulator: false,
                concurrency: 2,
                trace: true,
                ..seiza_stacking::GpuOptions::default()
            }
        );
    }

    #[test]
    fn a_stored_choice_reads_back_and_omitted_fields_take_defaults() {
        let read: StackGpu = serde_json::from_str(r#"{"detection": false}"#).unwrap();
        assert_eq!(
            read,
            StackGpu {
                detection: false,
                ..StackGpu::default()
            }
        );
        // A field a later version drops does not refuse the registry.
        let read: StackGpu = serde_json::from_str(r#"{"enabled": false, "tiles": 4}"#).unwrap();
        assert!(!read.enabled);
    }

    #[test]
    fn concurrency_outside_the_offered_range_is_refused() {
        for concurrency in [0, MAX_GPU_CONCURRENCY + 1] {
            assert!(StackGpu {
                concurrency,
                ..StackGpu::default()
            }
            .validate()
            .is_err());
        }
        assert!(StackGpu::default().validate().is_ok());
        // A stored count outside the range reads as the nearest limit.
        let stored = StackGpu {
            concurrency: 0,
            ..StackGpu::default()
        };
        assert_eq!(stored.clamped().concurrency, 1);
        assert!(stored.clamped().validate().is_ok());
    }
}
