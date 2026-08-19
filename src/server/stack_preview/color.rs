//! On-demand color previews composed from persisted per-filter stack artifacts.
//!
//! The channel stacks remain the source of truth. Color jobs capture their
//! exact artifact revisions, register them to a common pixel grid, and then
//! delegate RGB/LRGB/narrowband composition to `seiza-stacking`.

use super::{
    LatestStackPreviews, StackJobState, StackPreviewImageQuery, StackPreviewImageSize,
    StackPreviewManager, MAX_REMEMBERED_JOBS, SEIZA_STACKING_VERSION,
};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{
        header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE},
        StatusCode,
    },
    response::Response,
    Json,
};
use rayon::ThreadPoolBuilder;
use seiza_background::{BackgroundConfig, BackgroundFit, CorrectionMode, ProtectedRegion};
use seiza_stacking::{
    combine_lrgb, combine_narrowband, combine_rgb, resample_to_reference, write_color_fits_f32,
    write_processed_image_fits_f32, ColorComposition, ColorCrop, ColorNormalization, ColorOptions,
    ColorTransfer, CropReport, FitsFrame, ForaxxOptions, LinearImage, NarrowbandPalette, Registrar,
    RegistrationOptions,
};
use seiza_stretch::{StretchStack, StretchStackOutput, StretchStageProgress};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tokio_util::io::ReaderStream;

use crate::server::api::ApiResponse;
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;
use crate::server::state::AppState;

const STACK_COLOR_CACHE_VERSION: u32 = 12;
const COLOR_INPUT_CACHE_VERSION: u32 = 3;
const SEIZA_BACKGROUND_VERSION: &str = "0.2.0";
const MAX_REGISTRATION_RMS_PIXELS: f64 = 2.0;
const COLOR_BYTES_PER_PIXEL: u64 = 64;
const COLOR_DECONVOLUTION_BYTES_PER_PIXEL: u64 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackColorRole {
    Luminance,
    Red,
    Green,
    Blue,
    Ha,
    Oiii,
    Sii,
}

impl StackColorRole {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Luminance => "L",
            Self::Red => "R",
            Self::Green => "G",
            Self::Blue => "B",
            Self::Ha => "H-alpha",
            Self::Oiii => "OIII",
            Self::Sii => "SII",
        }
    }
}

/// How a color preview is trimmed to the sky every channel covers.
///
/// Registering one filter stack onto another leaves blank edges where a source
/// frame did not reach. Previews default to keeping them, which is the shape
/// every earlier release produced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StackColorCrop {
    /// Keep the whole reference grid, blank edges included.
    #[default]
    None,
    /// Keep the box the covered pixels span.
    Bounds,
    /// Keep the largest rectangle every channel covers in full.
    Inscribed,
}

impl StackColorCrop {
    fn seiza(self) -> ColorCrop {
        match self {
            Self::None => ColorCrop::None,
            Self::Bounds => ColorCrop::Bounds,
            Self::Inscribed => ColorCrop::Inscribed,
        }
    }
}

/// What a crop kept, and which channel is most likely to have bounded it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackColorCropReport {
    /// Width of the shared input grid.
    pub grid_width: usize,
    /// Height of the shared input grid.
    pub grid_height: usize,
    /// Horizontal origin of the kept region on that grid.
    pub x: usize,
    /// Vertical origin of the kept region on that grid.
    pub y: usize,
    /// Width of the kept region.
    pub width: usize,
    /// Height of the kept region.
    pub height: usize,
    /// Share of the input grid the kept region covers, from zero to one.
    pub retained_fraction: f64,
    /// One entry per input channel, in the order they were composed.
    pub channels: Vec<StackColorChannelCoverage>,
}

impl StackColorCropReport {
    /// Convert a `seiza-stacking` report, naming each entry with the role that
    /// produced it.
    ///
    /// `roles` lists the channels in the order they were passed to the
    /// composition, which is the order the report preserves.
    fn from_seiza(report: &CropReport, roles: &[StackColorRole]) -> Self {
        Self {
            grid_width: report.grid_width,
            grid_height: report.grid_height,
            x: report.region.x,
            y: report.region.y,
            width: report.region.width,
            height: report.region.height,
            retained_fraction: report.retained_fraction(),
            channels: report
                .channels
                .iter()
                .enumerate()
                .map(|(index, channel)| StackColorChannelCoverage {
                    role: roles.get(index).copied(),
                    name: channel.name.clone(),
                    covered_pixels: channel.covered_pixels,
                    center_offset_pixels: channel.center_offset_pixels(),
                    off_center: channel.off_center,
                })
                .collect(),
        }
    }
}

/// What one channel covered of the shared grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackColorChannelCoverage {
    pub role: Option<StackColorRole>,
    /// Channel name as `seiza-stacking` reported it, for example `OIII`.
    pub name: String,
    pub covered_pixels: usize,
    /// Offset of this channel's coverage center from the median center of
    /// every channel, in pixels.
    pub center_offset_pixels: f64,
    /// Whether that offset looks like a pointing error rather than dither.
    /// A flagged channel is the one that bounded the crop.
    pub off_center: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackColorKind {
    Rgb,
    Lrgb,
    Narrowband,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StackNarrowbandPalette {
    Sho,
    Soh,
    Hso,
    Hos,
    Osh,
    Ohs,
    Hoo,
    ForaxxSho,
    ForaxxHoo,
}

impl StackNarrowbandPalette {
    fn all(sii_available: bool) -> Vec<Self> {
        let mut palettes = if sii_available {
            vec![
                Self::Sho,
                Self::Soh,
                Self::Hso,
                Self::Hos,
                Self::Osh,
                Self::Ohs,
            ]
        } else {
            Vec::new()
        };
        palettes.extend([Self::Hoo, Self::ForaxxHoo]);
        if sii_available {
            palettes.push(Self::ForaxxSho);
        }
        palettes
    }

    fn seiza(self) -> NarrowbandPalette {
        match self {
            Self::Sho => NarrowbandPalette::Sho,
            Self::Soh => NarrowbandPalette::Soh,
            Self::Hso => NarrowbandPalette::Hso,
            Self::Hos => NarrowbandPalette::Hos,
            Self::Osh => NarrowbandPalette::Osh,
            Self::Ohs => NarrowbandPalette::Ohs,
            Self::Hoo => NarrowbandPalette::Hoo,
            Self::ForaxxSho => NarrowbandPalette::ForaxxSho,
            Self::ForaxxHoo => NarrowbandPalette::ForaxxHoo,
        }
    }

    fn requires_sii(self) -> bool {
        self.seiza().requires_sii()
    }

    fn label(self) -> &'static str {
        self.seiza().name()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct StackColorRequest {
    pub target_id: i32,
    pub kind: StackColorKind,
    #[serde(default)]
    pub palette: Option<StackNarrowbandPalette>,
    #[serde(default)]
    pub force: bool,
    /// How the composed preview is trimmed to the sky every channel covers.
    /// Absent requests keep the whole reference grid, as earlier releases did.
    #[serde(default)]
    pub crop: StackColorCrop,
    /// Optional non-destructive display pipeline. Absent requests retain the
    /// original quick-look behavior for API compatibility.
    #[serde(default)]
    pub processing: Option<StackColorProcessing>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StackColorProcessing {
    /// Optional smooth background correction applied to each linear channel
    /// independently before cross-channel registration.
    #[serde(default)]
    pub background_extraction: Option<StackBackgroundExtraction>,
    /// Optional deconvolution of selected registered linear input channels,
    /// before display normalization. Empty by default: color previews never
    /// restore pixels unless the user explicitly enables a role.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub input_deconvolutions: BTreeMap<StackColorRole, seiza_deconvolution::DeconvolutionConfig>,
    /// Ordered display stretches applied independently after registration and
    /// robust normalization of each physical input channel.
    #[serde(default)]
    pub input_stretches: BTreeMap<StackColorRole, Vec<super::stretch::StackStretchRequest>>,
    /// Ordered display stretches applied to the composed RGB result.
    #[serde(default)]
    pub output_stretches: Vec<super::stretch::StackStretchRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackBackgroundExtraction {
    pub config: BackgroundConfig,
    #[serde(default)]
    pub correction_mode: CorrectionMode,
    /// Fraction of the fitted correction applied to each channel.
    #[serde(default = "full_background_strength")]
    pub strength: f64,
    /// Keep solved catalog emission out of the background samples. Older
    /// requests retain the protected behavior.
    #[serde(default = "catalog_background_protection_enabled")]
    pub protect_catalog_emission: bool,
}

const fn full_background_strength() -> f64 {
    1.0
}

const fn catalog_background_protection_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackColorProgressState {
    Pending,
    Running,
    Completed,
    Skipped,
    Reused,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackColorProgressPhase {
    LoadingSources,
    BackgroundPreparation,
    RegisteringSources,
    DeconvolvingInputs,
    NormalizingInputs,
    StretchingInputs,
    ComposingColor,
    StretchingOutput,
    WritingFits,
    RenderingOriginal,
    RenderingScreen,
    PublishingArtifacts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackColorPhaseProgress {
    pub phase: StackColorProgressPhase,
    pub label: String,
    pub state: StackColorProgressState,
    pub completed_units: usize,
    pub total_units: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StackColorProgress {
    pub completed_units: usize,
    pub total_units: usize,
    pub active_phase: Option<StackColorProgressPhase>,
    pub current_role: Option<StackColorRole>,
    pub current_stage: Option<usize>,
    pub stage_count: Option<usize>,
    pub phases: Vec<StackColorPhaseProgress>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StackColorSource {
    pub role: StackColorRole,
    pub filter_name: String,
    pub job_id: String,
    pub group_index: usize,
    pub artifact_revision: String,
    pub accepted_frames: usize,
    #[serde(default)]
    pub reference_image_id: Option<i32>,
    #[serde(default)]
    pub sky_orientation: Option<super::StackSkyOrientation>,
    #[serde(default)]
    pub registration_transform: Option<seiza_stacking::SimilarityTransform>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StackBackgroundProtection {
    pub reference_image_id: i32,
    #[serde(default)]
    pub catalog_version: Option<String>,
    #[serde(default)]
    pub object_names: Vec<String>,
    #[serde(default)]
    pub region_count: usize,
    #[serde(default)]
    pub region_fingerprint: String,
}

#[derive(Debug, Clone)]
struct ResolvedBackgroundProtection {
    summary: StackBackgroundProtection,
    regions: Vec<ProtectedRegion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackColorJob {
    pub schema_version: u32,
    pub job_id: String,
    pub database_id: String,
    pub project_id: i32,
    pub target_id: i32,
    pub target_name: String,
    pub kind: StackColorKind,
    pub palette: Option<StackNarrowbandPalette>,
    pub label: String,
    pub state: StackJobState,
    pub phase: String,
    pub processed_channels: usize,
    pub total_channels: usize,
    #[serde(default)]
    pub progress: StackColorProgress,
    pub created_unix_seconds: i64,
    pub artifact_revision: String,
    pub cache_version: u32,
    pub stacking_version: String,
    #[serde(default)]
    pub background_version: String,
    #[serde(default)]
    pub deconvolution_version: String,
    #[serde(default)]
    pub linear_input_id: Option<String>,
    pub sources: Vec<StackColorSource>,
    #[serde(default)]
    pub crop: StackColorCrop,
    /// What the crop kept, once the composition has run. Absent while the job
    /// is queued, and for an uncropped preview, which measures no coverage.
    #[serde(default)]
    pub crop_report: Option<StackColorCropReport>,
    #[serde(default)]
    pub processing: Option<StackColorProcessing>,
    #[serde(default)]
    pub resolved_input_stretches: BTreeMap<StackColorRole, Vec<serde_json::Value>>,
    #[serde(default)]
    pub resolved_input_deconvolutions:
        BTreeMap<StackColorRole, super::stretch::StackDeconvolutionResult>,
    #[serde(default)]
    pub resolved_output_stretches: Vec<serde_json::Value>,
    #[serde(default)]
    pub resolved_backgrounds: BTreeMap<StackColorRole, BackgroundFit>,
    /// Fresh pixel-solve catalog regions used to keep real extended emission
    /// out of the background samples. These are part of the cache identity.
    #[serde(default)]
    pub resolved_background_protection: BTreeMap<StackColorRole, StackBackgroundProtection>,
    /// Protected fits that failed and were retried without catalog regions.
    #[serde(default)]
    pub background_protection_fallbacks: BTreeMap<StackColorRole, String>,
    pub preview_url: String,
    pub fits_url: String,
    pub error: Option<String>,
    #[serde(default)]
    pub outdated: bool,
    #[serde(default)]
    pub outdated_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StackColorAvailableRole {
    pub role: StackColorRole,
    pub filter_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StackColorTargetAvailability {
    pub target_id: i32,
    pub target_name: String,
    pub available_roles: Vec<StackColorAvailableRole>,
    pub ambiguous_roles: Vec<StackColorRole>,
    pub unmapped_filters: Vec<String>,
    pub rgb_available: bool,
    pub lrgb_available: bool,
    pub narrowband_palettes: Vec<StackNarrowbandPalette>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LatestStackColorPreviews {
    schema_version: u32,
    database_id: String,
    project_id: i32,
    updated_unix_seconds: i64,
    jobs: Vec<StackColorJob>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedColorInputs {
    schema_version: u32,
    input_id: String,
    roles: Vec<StackColorRole>,
    #[serde(default)]
    registered_transforms: BTreeMap<StackColorRole, seiza_stacking::SimilarityTransform>,
    resolved_backgrounds: BTreeMap<StackColorRole, BackgroundFit>,
    #[serde(default)]
    background_protection_fallbacks: BTreeMap<StackColorRole, String>,
    resolved_deconvolutions: BTreeMap<StackColorRole, super::stretch::StackDeconvolutionResult>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StackColorCatalog {
    pub schema_version: u32,
    pub database_id: String,
    pub project_id: i32,
    pub targets: Vec<StackColorTargetAvailability>,
    pub jobs: Vec<StackColorJob>,
}

#[derive(Default)]
struct TargetSources {
    target_name: String,
    by_role: BTreeMap<StackColorRole, Vec<StackColorSource>>,
    unmapped_filters: Vec<String>,
}

struct PreparedColorJob {
    public: StackColorJob,
    cache_root: PathBuf,
    background_regions: BTreeMap<StackColorRole, Vec<ProtectedRegion>>,
}

fn color_progress(
    channel_count: usize,
    processing: Option<&StackColorProcessing>,
) -> StackColorProgress {
    let input_stages = processing
        .map(|processing| {
            processing
                .input_stretches
                .values()
                .map(Vec::len)
                .sum::<usize>()
        })
        .unwrap_or(0);
    let output_stages = processing
        .map(|processing| processing.output_stretches.len())
        .unwrap_or(0);
    let deconvolution_units = processing
        .map(|processing| processing.input_deconvolutions.len())
        .unwrap_or(0);
    let background_units = if processing
        .and_then(|processing| processing.background_extraction.as_ref())
        .is_some()
    {
        channel_count.saturating_mul(2)
    } else {
        channel_count
    };
    let definitions = [
        (
            StackColorProgressPhase::LoadingSources,
            "Loading source stacks",
            channel_count,
        ),
        (
            StackColorProgressPhase::BackgroundPreparation,
            "Background preparation",
            background_units,
        ),
        (
            StackColorProgressPhase::RegisteringSources,
            "Registering source stacks",
            channel_count,
        ),
        (
            StackColorProgressPhase::DeconvolvingInputs,
            "Deconvolving input channels",
            deconvolution_units,
        ),
        (
            StackColorProgressPhase::NormalizingInputs,
            "Normalizing input channels",
            channel_count,
        ),
        (
            StackColorProgressPhase::StretchingInputs,
            "Applying input stretch stages",
            input_stages,
        ),
        (
            StackColorProgressPhase::ComposingColor,
            "Composing color",
            1,
        ),
        (
            StackColorProgressPhase::StretchingOutput,
            "Applying output stretch stages",
            output_stages,
        ),
        (StackColorProgressPhase::WritingFits, "Writing FITS", 1),
        (
            StackColorProgressPhase::RenderingOriginal,
            "Rendering full-size preview",
            1,
        ),
        (
            StackColorProgressPhase::RenderingScreen,
            "Rendering screen preview",
            1,
        ),
        (
            StackColorProgressPhase::PublishingArtifacts,
            "Publishing cached artifacts",
            1,
        ),
    ];
    let phases = definitions
        .into_iter()
        .map(|(phase, label, total_units)| StackColorPhaseProgress {
            phase,
            label: label.into(),
            state: StackColorProgressState::Pending,
            completed_units: 0,
            total_units,
        })
        .collect::<Vec<_>>();
    StackColorProgress {
        total_units: phases.iter().map(|phase| phase.total_units).sum(),
        phases,
        ..StackColorProgress::default()
    }
}

struct ColorProgressTracker<'a> {
    state: &'a Arc<AppState>,
    job_id: &'a str,
}

/// Apply an input-channel stretch without turning registration gaps into
/// valid black pixels. Seiza's display stretch maps non-finite samples to
/// black, which is right for final rendering but would erase the coverage
/// mask before color composition and cropping.
fn apply_input_stretch_stack(
    stack: &StretchStack,
    data: &[f32],
    channel_count: usize,
    mut progress: impl FnMut(StretchStageProgress),
) -> Result<StretchStackOutput<f32>, seiza_stretch::StretchError> {
    let stage_count = stack.len();
    let mut current = data.to_vec();
    let mut plans = Vec::with_capacity(stage_count);
    for (stage_index, config) in stack.stages().iter().cloned().enumerate() {
        let output = StretchStack::single(config).apply_f32_with_progress(
            &current,
            channel_count,
            |event| {
                progress(StretchStageProgress {
                    stage_index,
                    stage_count,
                    state: event.state,
                });
            },
        )?;
        let mut next = output.data;
        for (sample, input) in next.iter_mut().zip(&current) {
            if !input.is_finite() {
                *sample = f32::NAN;
            }
        }
        current = next;
        plans.extend(output.plans);
    }
    Ok(StretchStackOutput {
        data: current,
        plans,
    })
}

impl ColorProgressTracker<'_> {
    fn begin(
        &self,
        phase: StackColorProgressPhase,
        label: impl Into<String>,
        role: Option<StackColorRole>,
        stage: Option<(usize, usize)>,
    ) {
        let label = label.into();
        self.state.stack_previews.update_color(self.job_id, |job| {
            job.phase = label.clone();
            job.progress.active_phase = Some(phase);
            job.progress.current_role = role;
            job.progress.current_stage = stage.map(|(index, _)| index);
            job.progress.stage_count = stage.map(|(_, count)| count);
            if let Some(entry) = job
                .progress
                .phases
                .iter_mut()
                .find(|entry| entry.phase == phase)
            {
                entry.label = label;
                entry.state = StackColorProgressState::Running;
            }
        });
    }

    fn advance(&self, phase: StackColorProgressPhase, units: usize) {
        self.state.stack_previews.update_color(self.job_id, |job| {
            if let Some(entry) = job
                .progress
                .phases
                .iter_mut()
                .find(|entry| entry.phase == phase)
            {
                let remaining = entry.total_units.saturating_sub(entry.completed_units);
                let increment = units.min(remaining);
                entry.completed_units += increment;
                job.progress.completed_units += increment;
            }
        });
    }

    fn finish(&self, phase: StackColorProgressPhase) {
        self.state.stack_previews.update_color(self.job_id, |job| {
            if let Some(entry) = job
                .progress
                .phases
                .iter_mut()
                .find(|entry| entry.phase == phase)
            {
                let remaining = entry.total_units.saturating_sub(entry.completed_units);
                entry.completed_units = entry.total_units;
                entry.state = StackColorProgressState::Completed;
                job.progress.completed_units += remaining;
            }
        });
    }

    fn skip(&self, phase: StackColorProgressPhase, label: impl Into<String>) {
        let label = label.into();
        self.state.stack_previews.update_color(self.job_id, |job| {
            if let Some(entry) = job
                .progress
                .phases
                .iter_mut()
                .find(|entry| entry.phase == phase)
            {
                let remaining = entry.total_units.saturating_sub(entry.completed_units);
                entry.label = label.clone();
                entry.completed_units = entry.total_units;
                entry.state = StackColorProgressState::Skipped;
                job.progress.completed_units += remaining;
            }
        });
    }

    fn reuse(&self, phase: StackColorProgressPhase, label: impl Into<String>) {
        let label = label.into();
        self.state.stack_previews.update_color(self.job_id, |job| {
            if let Some(entry) = job
                .progress
                .phases
                .iter_mut()
                .find(|entry| entry.phase == phase)
            {
                let remaining = entry.total_units.saturating_sub(entry.completed_units);
                entry.label = label.clone();
                entry.completed_units = entry.total_units;
                entry.state = StackColorProgressState::Reused;
                job.progress.completed_units += remaining;
            }
        });
    }

    fn fail_active(&self) {
        self.state.stack_previews.update_color(self.job_id, |job| {
            let Some(active) = job.progress.active_phase else {
                return;
            };
            if let Some(entry) = job
                .progress
                .phases
                .iter_mut()
                .find(|entry| entry.phase == active)
            {
                entry.state = StackColorProgressState::Failed;
            }
        });
    }
}

impl StackPreviewManager {
    pub(super) fn get_color(&self, job_id: &str) -> Option<StackColorJob> {
        self.color_jobs.lock().unwrap().get(job_id).cloned()
    }

    fn insert_color(&self, job: StackColorJob) -> bool {
        let mut jobs = self.color_jobs.lock().unwrap();
        if jobs.len() >= MAX_REMEMBERED_JOBS && !jobs.contains_key(&job.job_id) {
            let Some(oldest) = jobs
                .values()
                .filter(|entry| {
                    matches!(
                        entry.state,
                        StackJobState::Completed | StackJobState::Failed
                    )
                })
                .min_by_key(|entry| entry.created_unix_seconds)
                .map(|entry| entry.job_id.clone())
            else {
                return false;
            };
            jobs.remove(&oldest);
        }
        jobs.insert(job.job_id.clone(), job);
        true
    }

    fn update_color(&self, job_id: &str, update: impl FnOnce(&mut StackColorJob)) {
        if let Some(job) = self.color_jobs.lock().unwrap().get_mut(job_id) {
            update(job);
        }
    }

    fn persist_latest_color(&self, cache_root: &FsPath, job: &StackColorJob) -> Result<(), String> {
        let _guard = self.latest_write.lock().unwrap();
        persist_latest_color(cache_root, job)
    }
}

pub(super) fn load_persisted_color_job(
    cache_root: &FsPath,
    job_id: &str,
) -> Result<StackColorJob, AppError> {
    let bytes =
        std::fs::read(color_manifest_path(cache_root, job_id)).map_err(|_| AppError::NotFound)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| AppError::InternalError(format!("Invalid color manifest: {error}")))
}

pub async fn get_stack_color_catalog(
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<StackColorCatalog>>, AppError> {
    let latest = load_latest_stacks(&ctx, project_id)?;
    let sources = collect_sources(&ctx.cache_dir_path, &latest);
    let targets = availability(&sources);
    let mut jobs = load_latest_colors(&ctx, project_id)?.jobs;
    for job in &mut jobs {
        job.outdated_reason = color_job_outdated_reason(&ctx, job, &latest)?;
        job.outdated = job.outdated_reason.is_some();
    }
    jobs.sort_by(|left, right| {
        left.target_name
            .cmp(&right.target_name)
            .then_with(|| left.label.cmp(&right.label))
    });
    Ok(Json(ApiResponse::success(StackColorCatalog {
        schema_version: 1,
        database_id: ctx.id.clone(),
        project_id,
        targets,
        jobs,
    })))
}

pub async fn start_stack_color(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
    Json(request): Json<StackColorRequest>,
) -> Result<Json<ApiResponse<StackColorJob>>, AppError> {
    validate_request(&request)?;
    let ctx_arc = Arc::clone(&ctx.0);
    let request_for_prepare = request.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_color_job(&ctx_arc, project_id, &request_for_prepare)
    })
    .await
    .map_err(|error| {
        AppError::InternalError(format!("Color preparation task failed: {error}"))
    })??;

    if let Some(existing) = state.stack_previews.get_color(&prepared.public.job_id) {
        if matches!(
            existing.state,
            StackJobState::Queued | StackJobState::Running
        ) {
            return Ok(Json(ApiResponse::success(existing)));
        }
        if !request.force
            && existing.state == StackJobState::Completed
            && color_artifacts_exist(&prepared.cache_root, &existing.job_id)
        {
            let existing = mark_color_reused(existing);
            state
                .stack_previews
                .persist_latest_color(&prepared.cache_root, &existing)
                .map_err(AppError::InternalError)?;
            let _ = state.stack_previews.insert_color(existing.clone());
            return Ok(Json(ApiResponse::success(existing)));
        }
    }
    let manifest = color_manifest_path(&prepared.cache_root, &prepared.public.job_id);
    if !request.force
        && let Ok(bytes) = std::fs::read(&manifest)
        && let Ok(existing) = serde_json::from_slice::<StackColorJob>(&bytes)
        && existing.state == StackJobState::Completed
        && color_artifacts_exist(&prepared.cache_root, &existing.job_id)
    {
        let existing = mark_color_reused(existing);
        state
            .stack_previews
            .persist_latest_color(&prepared.cache_root, &existing)
            .map_err(AppError::InternalError)?;
        let _ = state.stack_previews.insert_color(existing.clone());
        return Ok(Json(ApiResponse::success(existing)));
    }

    let response = prepared.public.clone();
    if !state.stack_previews.insert_color(response.clone()) {
        return Err(AppError::BadRequest(format!(
            "At most {MAX_REMEMBERED_JOBS} color preview jobs may be active at once"
        )));
    }
    enqueue_color_job(Arc::clone(&state), prepared);
    Ok(Json(ApiResponse::success(response)))
}

fn mark_color_reused(mut job: StackColorJob) -> StackColorJob {
    job.phase = "Reused cached color preview".into();
    job.progress.active_phase = None;
    job.progress.current_role = None;
    job.progress.current_stage = None;
    job.progress.stage_count = None;
    for phase in &mut job.progress.phases {
        if phase.state == StackColorProgressState::Completed {
            phase.state = StackColorProgressState::Reused;
        }
    }
    job
}

pub async fn get_stack_color_job(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id, job_id)): Path<(String, i32, String)>,
) -> Result<Json<ApiResponse<StackColorJob>>, AppError> {
    super::validate_job_id(&job_id)?;
    if let Some(job) = state.stack_previews.get_color(&job_id) {
        if job.database_id != ctx.id || job.project_id != project_id {
            return Err(AppError::NotFound);
        }
        return Ok(Json(ApiResponse::success(job)));
    }
    let bytes = std::fs::read(color_manifest_path(&ctx.cache_dir_path, &job_id))
        .map_err(|_| AppError::NotFound)?;
    let job: StackColorJob = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::InternalError(format!("Invalid color manifest: {error}")))?;
    if job.database_id != ctx.id || job.project_id != project_id {
        return Err(AppError::NotFound);
    }
    let _ = state.stack_previews.insert_color(job.clone());
    Ok(Json(ApiResponse::success(job)))
}

pub async fn get_stack_color_image(
    ctx: DbContext,
    Path((_db_id, job_id)): Path<(String, String)>,
    Query(query): Query<StackPreviewImageQuery>,
) -> Result<Response, AppError> {
    super::validate_job_id(&job_id)?;
    let path = match query.size {
        StackPreviewImageSize::Screen => color_preview_path(&ctx.cache_dir_path, &job_id),
        StackPreviewImageSize::Original => {
            color_original_preview_path(&ctx.cache_dir_path, &job_id)
        }
    };
    stream_artifact(path, "image/png", None).await
}

pub async fn download_stack_color_fits(
    ctx: DbContext,
    Path((_db_id, job_id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    super::validate_job_id(&job_id)?;
    let manifest = std::fs::read(color_manifest_path(&ctx.cache_dir_path, &job_id))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<StackColorJob>(&bytes).ok());
    let label = manifest
        .as_ref()
        .map(|job| job.label.to_ascii_lowercase().replace('-', "_"))
        .unwrap_or_else(|| "color".into());
    let filename = format!("psf-guard-{label}-{}.fits", &job_id[..12]);
    stream_artifact(
        color_fits_path(&ctx.cache_dir_path, &job_id),
        "application/fits",
        Some(filename),
    )
    .await
}

async fn stream_artifact(
    path: PathBuf,
    content_type: &'static str,
    filename: Option<String>,
) -> Result<Response, AppError> {
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let length = file
        .metadata()
        .await
        .map_err(|error| {
            AppError::InternalError(format!("Failed to stat color artifact: {error}"))
        })?
        .len();
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, content_type)
        .header(CONTENT_LENGTH, length)
        .header(CACHE_CONTROL, "private, max-age=31536000, immutable");
    if let Some(filename) = filename {
        response = response.header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        );
    }
    response
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|error| {
            AppError::InternalError(format!("Failed to stream color artifact: {error}"))
        })
}

fn validate_request(request: &StackColorRequest) -> Result<(), AppError> {
    match (request.kind, request.palette) {
        (StackColorKind::Rgb | StackColorKind::Lrgb, Some(_)) => Err(AppError::BadRequest(
            "RGB and LRGB color previews do not take a narrowband palette".into(),
        )),
        (StackColorKind::Narrowband, None) => Err(AppError::BadRequest(
            "Narrowband color previews require a palette".into(),
        )),
        _ => {
            let Some(processing) = &request.processing else {
                return Ok(());
            };
            let required = required_roles(request.kind, request.palette);
            if let Some(role) = processing
                .input_stretches
                .keys()
                .find(|role| !required.contains(role))
            {
                return Err(AppError::BadRequest(format!(
                    "{} is not an input to {}",
                    role.label(),
                    composition_label(request.kind, request.palette)
                )));
            }
            if let Some(extraction) = &processing.background_extraction {
                if !extraction.config.protected_regions.is_empty() {
                    return Err(AppError::BadRequest(
                        "Background protected regions come from fresh plate solves and cannot be supplied by the client"
                            .into(),
                    ));
                }
                if !extraction.strength.is_finite() || !(0.0..=1.0).contains(&extraction.strength) {
                    return Err(AppError::BadRequest(
                        "Background correction strength must be between 0 and 1".into(),
                    ));
                }
                extraction.config.validate().map_err(|error| {
                    AppError::BadRequest(format!("Invalid background settings: {error}"))
                })?;
            }
            if let Some(error) = processing
                .input_deconvolutions
                .values()
                .find_map(|request| request.validate().err())
            {
                return Err(AppError::BadRequest(error.to_string()));
            }
            if let Some(role) = processing
                .input_deconvolutions
                .keys()
                .find(|role| !required.contains(role))
            {
                return Err(AppError::BadRequest(format!(
                    "{} is not an input to {}",
                    role.label(),
                    composition_label(request.kind, request.palette)
                )));
            }
            if processing.input_stretches.values().flatten().any(|stage| {
                stage.color_strategy == seiza_stretch::ColorStrategy::LuminancePreserving
            }) {
                return Err(AppError::BadRequest(
                    "A mono input stretch cannot use luminance-preserving color".into(),
                ));
            }
            let stage_count = processing
                .input_stretches
                .values()
                .map(Vec::len)
                .sum::<usize>()
                + processing.output_stretches.len();
            if stage_count > 64 {
                return Err(AppError::BadRequest(
                    "A color processing stack may contain at most 64 total stages".into(),
                ));
            }
            Ok(())
        }
    }
}

fn prepare_color_job(
    ctx: &crate::server::database_context::DatabaseContext,
    project_id: i32,
    request: &StackColorRequest,
) -> Result<PreparedColorJob, AppError> {
    let latest = load_latest_stacks(ctx, project_id)?;
    let targets = collect_sources(&ctx.cache_dir_path, &latest);
    let target = targets.get(&request.target_id).ok_or_else(|| {
        AppError::BadRequest("No completed channel stacks are available for that target".into())
    })?;
    let roles = required_roles(request.kind, request.palette);
    let mut sources = Vec::with_capacity(roles.len());
    for role in roles {
        let candidates = target.by_role.get(&role).map(Vec::as_slice).unwrap_or(&[]);
        match candidates {
            [source] => sources.push(source.clone()),
            [] => {
                return Err(AppError::BadRequest(format!(
                    "{} requires a {} channel stack",
                    composition_label(request.kind, request.palette),
                    role.label()
                )))
            }
            _ => {
                return Err(AppError::BadRequest(format!(
                    "{} has multiple channel stacks that map to {}; rename filters to make the role unambiguous",
                    target.target_name,
                    role.label()
                )))
            }
        }
    }

    let resolved_background_protection = if request.processing.as_ref().is_some_and(|processing| {
        processing
            .background_extraction
            .as_ref()
            .is_some_and(|extraction| extraction.protect_catalog_emission)
    }) {
        resolve_background_protection(ctx, &sources)?
    } else {
        BTreeMap::new()
    };
    let mut background_regions = BTreeMap::new();
    let mut background_protection_summary = BTreeMap::new();
    for (role, protection) in resolved_background_protection {
        background_regions.insert(role, protection.regions);
        background_protection_summary.insert(role, protection.summary);
    }
    let label = composition_label(request.kind, request.palette).to_string();
    let linear_input_id = request
        .processing
        .as_ref()
        .map(|processing| {
            color_input_cache_id(
                &ctx.id,
                project_id,
                request.target_id,
                processing,
                &sources,
                &background_protection_summary,
            )
        })
        .transpose()?;
    let mut hasher = Sha256::new();
    hasher.update(ctx.id.as_bytes());
    hasher.update(project_id.to_le_bytes());
    hasher.update(request.target_id.to_le_bytes());
    hasher.update(label.as_bytes());
    hasher.update(STACK_COLOR_CACHE_VERSION.to_le_bytes());
    hasher.update(SEIZA_STACKING_VERSION.as_bytes());
    hasher.update(SEIZA_BACKGROUND_VERSION.as_bytes());
    if request
        .processing
        .as_ref()
        .is_some_and(|processing| !processing.input_deconvolutions.is_empty())
    {
        hasher.update(super::stretch::deconvolution_version().as_bytes());
    }
    hasher.update(serde_json::to_vec(&request.crop).map_err(|error| {
        AppError::InternalError(format!("Failed to encode color crop mode: {error}"))
    })?);
    hasher.update(serde_json::to_vec(&request.processing).map_err(|error| {
        AppError::InternalError(format!(
            "Failed to encode color processing options: {error}"
        ))
    })?);
    hasher.update(
        serde_json::to_vec(&background_protection_summary).map_err(|error| {
            AppError::InternalError(format!("Failed to encode background protection: {error}"))
        })?,
    );
    for source in &sources {
        hasher.update([source.role as u8]);
        hasher.update(source.filter_name.as_bytes());
        hasher.update(source.job_id.as_bytes());
        hasher.update(source.group_index.to_le_bytes());
        hasher.update(source.artifact_revision.as_bytes());
    }
    let digest = hasher.finalize();
    let mut job_id = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut job_id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    let artifact_revision = super::new_artifact_revision();
    let total_channels = sources.len();
    Ok(PreparedColorJob {
        public: StackColorJob {
            schema_version: 1,
            job_id: job_id.clone(),
            database_id: ctx.id.clone(),
            project_id,
            target_id: request.target_id,
            target_name: target.target_name.clone(),
            kind: request.kind,
            palette: request.palette,
            label,
            state: StackJobState::Queued,
            phase: "Waiting for color processor".into(),
            processed_channels: 0,
            total_channels,
            progress: color_progress(total_channels, request.processing.as_ref()),
            created_unix_seconds: chrono::Utc::now().timestamp(),
            artifact_revision: artifact_revision.clone(),
            cache_version: STACK_COLOR_CACHE_VERSION,
            stacking_version: SEIZA_STACKING_VERSION.into(),
            background_version: SEIZA_BACKGROUND_VERSION.into(),
            deconvolution_version: request
                .processing
                .as_ref()
                .filter(|processing| !processing.input_deconvolutions.is_empty())
                .map(|_| super::stretch::deconvolution_version())
                .unwrap_or_default(),
            linear_input_id,
            sources,
            crop: request.crop,
            crop_report: None,
            processing: request.processing.clone(),
            resolved_input_stretches: BTreeMap::new(),
            resolved_input_deconvolutions: BTreeMap::new(),
            resolved_output_stretches: Vec::new(),
            resolved_backgrounds: BTreeMap::new(),
            resolved_background_protection: background_protection_summary,
            background_protection_fallbacks: BTreeMap::new(),
            preview_url: format!(
                "/api/db/{}/stack-previews/color/{job_id}/preview?v={artifact_revision}",
                ctx.id
            ),
            fits_url: format!(
                "/api/db/{}/stack-previews/color/{job_id}/fits?v={artifact_revision}",
                ctx.id
            ),
            error: None,
            outdated: false,
            outdated_reason: None,
        },
        cache_root: ctx.cache_dir_path.clone(),
        background_regions,
    })
}

fn color_input_cache_id(
    database_id: &str,
    project_id: i32,
    target_id: i32,
    processing: &StackColorProcessing,
    sources: &[StackColorSource],
    resolved_background_protection: &BTreeMap<StackColorRole, StackBackgroundProtection>,
) -> Result<String, AppError> {
    let mut hasher = Sha256::new();
    hasher.update(database_id.as_bytes());
    hasher.update(project_id.to_le_bytes());
    hasher.update(target_id.to_le_bytes());
    hasher.update(COLOR_INPUT_CACHE_VERSION.to_le_bytes());
    hasher.update(SEIZA_STACKING_VERSION.as_bytes());
    if processing.background_extraction.is_some() {
        hasher.update(SEIZA_BACKGROUND_VERSION.as_bytes());
    }
    if !processing.input_deconvolutions.is_empty() {
        hasher.update(seiza_deconvolution::ALGORITHM_VERSION.to_le_bytes());
    }
    hasher.update(
        serde_json::to_vec(&(
            &processing.background_extraction,
            &processing.input_deconvolutions,
            resolved_background_protection,
        ))
        .map_err(|error| {
            AppError::InternalError(format!("Failed to encode color input processing: {error}"))
        })?,
    );
    for source in sources {
        hasher.update([source.role as u8]);
        hasher.update(source.filter_name.as_bytes());
        hasher.update(source.job_id.as_bytes());
        hasher.update(source.group_index.to_le_bytes());
        hasher.update(source.artifact_revision.as_bytes());
    }
    let mut id = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(id)
}

fn resolve_background_protection(
    ctx: &crate::server::database_context::DatabaseContext,
    sources: &[StackColorSource],
) -> Result<BTreeMap<StackColorRole, ResolvedBackgroundProtection>, AppError> {
    let mut resolved = BTreeMap::new();
    for source in sources {
        let Some(reference_image_id) = source.reference_image_id else {
            continue;
        };
        let Some(orientation) = source.sky_orientation.as_ref() else {
            continue;
        };
        let Some(analysis) = ctx.astrometry_evidence.evidence_for_source(
            &ctx.cache_dir_path,
            reference_image_id,
            None,
        ) else {
            continue;
        };
        let Some(solution) = analysis.solution.as_ref() else {
            continue;
        };
        let (regions, object_names) = background_regions_from_solution(solution, orientation);
        let bytes = serde_json::to_vec(&regions).map_err(|error| {
            AppError::InternalError(format!(
                "Failed to encode background-protection regions: {error}"
            ))
        })?;
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let mut region_fingerprint = String::with_capacity(64);
        for byte in hasher.finalize() {
            write!(&mut region_fingerprint, "{byte:02x}").expect("writing to a String cannot fail");
        }
        resolved.insert(
            source.role,
            ResolvedBackgroundProtection {
                summary: StackBackgroundProtection {
                    reference_image_id,
                    catalog_version: solution.catalog_version.clone(),
                    object_names,
                    region_count: regions.len(),
                    region_fingerprint,
                },
                regions,
            },
        );
    }
    Ok(resolved)
}

fn background_regions_from_solution(
    solution: &crate::astrometry::AstrometrySolutionResponse,
    orientation: &super::StackSkyOrientation,
) -> (Vec<ProtectedRegion>, Vec<String>) {
    let width = orientation.output_width;
    let height = orientation.output_height;
    if width < 2 || height < 2 {
        return (Vec::new(), Vec::new());
    }

    let mut regions = Vec::new();
    let mut object_names = Vec::new();
    for object in crate::sequence_analysis::cataloged_extended_emission_objects(solution) {
        let before = regions.len();
        for outline in &object.outlines {
            if !matches!(outline.quality.as_str(), "catalog" | "curated") {
                continue;
            }
            for contour in &outline.contours {
                if contour.closed {
                    let points = contour
                        .points
                        .iter()
                        .map(|point| {
                            let (x, y) = orientation.source_to_output.apply(point[0], point[1]);
                            [x, y]
                        })
                        .collect::<Vec<_>>();
                    if let Ok(region) = ProtectedRegion::polygon_from_pixels(&points, width, height)
                    {
                        regions.push(region);
                    }
                }
            }
        }
        if regions.len() == before
            && object.x.is_finite()
            && object.y.is_finite()
            && object.semi_major_px.is_finite()
            && object.semi_minor_px.is_finite()
            && object.semi_major_px > 0.0
            && object.semi_minor_px > 0.0
        {
            let angle = object.angle_deg.unwrap_or(0.0).to_radians();
            let points = (0..48)
                .map(|index| {
                    let phase = std::f64::consts::TAU * index as f64 / 48.0;
                    let major = object.semi_major_px * phase.cos();
                    let minor = object.semi_minor_px * phase.sin();
                    orientation.source_to_output.apply(
                        object.x + major * angle.cos() - minor * angle.sin(),
                        object.y + major * angle.sin() + minor * angle.cos(),
                    )
                })
                .map(|(x, y)| [x, y])
                .collect::<Vec<_>>();
            if let Ok(region) = ProtectedRegion::polygon_from_pixels(&points, width, height) {
                regions.push(region);
            }
        }
        if regions.len() > before {
            object_names.push(if object.common_name.is_empty() {
                object.name.clone()
            } else {
                object.common_name.clone()
            });
        }
    }
    object_names.sort();
    object_names.dedup();
    (regions, object_names)
}

fn required_roles(
    kind: StackColorKind,
    palette: Option<StackNarrowbandPalette>,
) -> Vec<StackColorRole> {
    match kind {
        StackColorKind::Rgb => vec![
            StackColorRole::Red,
            StackColorRole::Green,
            StackColorRole::Blue,
        ],
        StackColorKind::Lrgb => vec![
            StackColorRole::Luminance,
            StackColorRole::Red,
            StackColorRole::Green,
            StackColorRole::Blue,
        ],
        StackColorKind::Narrowband => {
            let palette = palette.expect("validated narrowband palette");
            let mut roles = vec![StackColorRole::Ha, StackColorRole::Oiii];
            if palette.requires_sii() {
                roles.push(StackColorRole::Sii);
            }
            roles
        }
    }
}

fn composition_label(
    kind: StackColorKind,
    palette: Option<StackNarrowbandPalette>,
) -> &'static str {
    match kind {
        StackColorKind::Rgb => "RGB",
        StackColorKind::Lrgb => "LRGB",
        StackColorKind::Narrowband => palette.expect("validated narrowband palette").label(),
    }
}

fn enqueue_color_job(state: Arc<AppState>, prepared: PreparedColorJob) {
    let permit = Arc::clone(&state.stack_previews.permit);
    tokio::spawn(async move {
        let Ok(_permit) = permit.acquire_owned().await else {
            return;
        };
        let guard = state.begin_interactive_job();
        let state_for_job = Arc::clone(&state);
        let job_id = prepared.public.job_id.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            run_color_job(&state_for_job, prepared)
        })
        .await;
        if let Err(error) = result {
            state.stack_previews.update_color(&job_id, |job| {
                job.state = StackJobState::Failed;
                job.phase = "Color worker failed".into();
                job.error = Some(format!("Color worker panicked: {error}"));
            });
        }
    });
}

fn run_color_job(state: &Arc<AppState>, prepared: PreparedColorJob) {
    let job_id = prepared.public.job_id.clone();
    state.stack_previews.update_color(&job_id, |job| {
        job.state = StackJobState::Running;
        job.phase = "Loading channel stacks".into();
    });
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compose_color(
            state,
            &prepared.public,
            &prepared.background_regions,
            &prepared.cache_root,
        )
    }));
    let progress = ColorProgressTracker {
        state,
        job_id: &job_id,
    };
    match result {
        Ok(Ok(())) => {
            progress.begin(
                StackColorProgressPhase::PublishingArtifacts,
                "Publishing cached artifacts",
                None,
                None,
            );
            if let Some(mut completed) = state.stack_previews.get_color(&job_id) {
                finish_color_job(&mut completed);
                let persisted =
                    persist_color_manifest(&prepared.cache_root, &completed).and_then(|()| {
                        state
                            .stack_previews
                            .persist_latest_color(&prepared.cache_root, &completed)
                    });
                match persisted {
                    Ok(()) => {
                        let _ = state.stack_previews.insert_color(completed);
                        state.stack_previews.prune_cache(&prepared.cache_root);
                    }
                    Err(error) => {
                        tracing::warn!("Failed to publish color preview: {error}");
                        state.stack_previews.update_color(&job_id, |job| {
                            job.state = StackJobState::Failed;
                            job.phase = "Publishing color preview failed".into();
                            job.error = Some(error);
                            if let Some(entry) = job.progress.phases.iter_mut().find(|entry| {
                                entry.phase == StackColorProgressPhase::PublishingArtifacts
                            }) {
                                entry.state = StackColorProgressState::Failed;
                            }
                        });
                    }
                }
            }
        }
        Ok(Err(error)) => {
            progress.fail_active();
            state.stack_previews.update_color(&job_id, |job| {
                job.state = StackJobState::Failed;
                job.phase = "Color preview failed".into();
                job.error = Some(error);
            });
        }
        Err(_) => {
            progress.fail_active();
            state.stack_previews.update_color(&job_id, |job| {
                job.state = StackJobState::Failed;
                job.phase = "Color worker failed".into();
                job.error = Some("Color worker panicked".into());
            });
        }
    }
}

fn finish_color_job(job: &mut StackColorJob) {
    if let Some(entry) = job
        .progress
        .phases
        .iter_mut()
        .find(|entry| entry.phase == StackColorProgressPhase::PublishingArtifacts)
    {
        let remaining = entry.total_units.saturating_sub(entry.completed_units);
        entry.completed_units = entry.total_units;
        entry.state = StackColorProgressState::Completed;
        job.progress.completed_units += remaining;
    }
    job.state = StackJobState::Completed;
    job.phase = "Color preview ready".into();
    job.progress.active_phase = None;
    job.progress.current_role = None;
    job.progress.current_stage = None;
    job.progress.stage_count = None;
}

fn fit_channel_background(
    image: &LinearImage,
    extraction: &StackBackgroundExtraction,
    protected_regions: Vec<ProtectedRegion>,
    mut on_fallback: impl FnMut(&str),
) -> Result<(BackgroundFit, Option<String>), seiza_background::Error> {
    let mut config = extraction.config.clone();
    if extraction.protect_catalog_emission {
        config.protected_regions = protected_regions;
    } else {
        config.protected_regions.clear();
    }
    let protected_fit = !config.protected_regions.is_empty();
    match seiza_background::fit_background(
        &image.data,
        image.width,
        image.height,
        image.channels,
        &config,
    ) {
        Ok(fit) => Ok((fit, None)),
        Err(error) if protected_fit => {
            let reason = error.to_string();
            on_fallback(&reason);
            config.protected_regions.clear();
            seiza_background::fit_background(
                &image.data,
                image.width,
                image.height,
                image.channels,
                &config,
            )
            .map(|fit| (fit, Some(reason)))
        }
        Err(error) => Err(error),
    }
}

fn compose_color(
    state: &Arc<AppState>,
    job: &StackColorJob,
    background_regions: &BTreeMap<StackColorRole, Vec<ProtectedRegion>>,
    cache_root: &FsPath,
) -> Result<(), String> {
    let progress = ColorProgressTracker {
        state,
        job_id: &job.job_id,
    };
    let reference_role = match job.kind {
        StackColorKind::Rgb => StackColorRole::Red,
        StackColorKind::Lrgb => StackColorRole::Luminance,
        StackColorKind::Narrowband => StackColorRole::Ha,
    };
    let reference_source = job
        .sources
        .iter()
        .find(|source| source.role == reference_role)
        .ok_or_else(|| "Color job has no reference channel".to_string())?;
    let cached_inputs = job.linear_input_id.as_deref().and_then(|input_id| {
        load_cached_color_input_manifest(cache_root, input_id, reference_role, &job.sources)
    });
    let reused_inputs = cached_inputs.is_some();
    let (
        mut reference,
        mut resolved_backgrounds,
        mut background_protection_fallbacks,
        mut resolved_deconvolutions,
        mut registered_transforms,
    ) = if let Some((manifest, reference)) = cached_inputs {
        state.stack_previews.update_color(&job.job_id, |current| {
            current.processed_channels = current.total_channels;
            current.resolved_backgrounds = manifest.resolved_backgrounds.clone();
            current.background_protection_fallbacks =
                manifest.background_protection_fallbacks.clone();
            current.resolved_input_deconvolutions = manifest.resolved_deconvolutions.clone();
            for source in &mut current.sources {
                source.registration_transform =
                    manifest.registered_transforms.get(&source.role).copied();
            }
        });
        (
            reference,
            manifest.resolved_backgrounds,
            manifest.background_protection_fallbacks,
            manifest.resolved_deconvolutions,
            manifest.registered_transforms,
        )
    } else {
        progress.begin(
            StackColorProgressPhase::LoadingSources,
            "Loading source stacks",
            None,
            None,
        );
        progress.begin(
            StackColorProgressPhase::LoadingSources,
            format!("Loading {} stack", reference_role.label()),
            Some(reference_role),
            None,
        );
        let reference = load_source_frame(cache_root, reference_source)?;
        progress.advance(StackColorProgressPhase::LoadingSources, 1);
        (
            reference,
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        )
    };
    registered_transforms
        .entry(reference_role)
        .or_insert(seiza_stacking::SimilarityTransform::IDENTITY);
    state.stack_previews.update_color(&job.job_id, |current| {
        if let Some(source) = current
            .sources
            .iter_mut()
            .find(|source| source.role == reference_role)
        {
            source.registration_transform = Some(seiza_stacking::SimilarityTransform::IDENTITY);
        }
    });
    let pixels = reference.image.pixel_count();
    let bytes_per_pixel = COLOR_BYTES_PER_PIXEL
        + if job
            .processing
            .as_ref()
            .is_some_and(|processing| !processing.input_deconvolutions.is_empty())
        {
            COLOR_DECONVOLUTION_BYTES_PER_PIXEL
        } else {
            0
        };
    let estimate = (pixels as u64).saturating_mul(bytes_per_pixel);
    let policy = state.worker_policy();
    if let Some(available) = crate::concurrency::available_memory_bytes()
        && estimate > (available as f64 * policy.memory_budget_fraction) as u64
    {
        return Err(format!(
            "Estimated color-composition memory {} MiB exceeds the configured available-memory budget",
            estimate / (1024 * 1024)
        ));
    }
    let budget = crate::concurrency::plan_workers(
        None,
        &policy,
        crate::concurrency::Priority::Interactive,
        Some(pixels),
    );
    let pool = ThreadPoolBuilder::new()
        .num_threads(budget.workers)
        .thread_name(|index| format!("stack-color-{index}"))
        .build()
        .map_err(|error| error.to_string())?;
    tracing::info!(
        "Stack color {}: {} worker(s) — {}",
        job.job_id,
        budget.workers,
        budget.rationale
    );

    // Check the whole-pipeline budget after loading only the reference. Admit
    // the other channel buffers only after that check.
    let mut frames = BTreeMap::new();
    for source in job
        .sources
        .iter()
        .filter(|source| source.role != reference_role)
    {
        let frame = if reused_inputs {
            let input_id = job
                .linear_input_id
                .as_deref()
                .expect("cached inputs have an identity");
            let frame = crate::image_io::open_linear_frame(color_input_fits_path(
                cache_root,
                input_id,
                source.role,
            ))
            .map_err(|error| error.to_string())?;
            validate_mono(&frame.image, source.role)?;
            frame
        } else {
            progress.begin(
                StackColorProgressPhase::LoadingSources,
                format!("Loading {} stack", source.role.label()),
                Some(source.role),
                None,
            );
            let frame = load_source_frame(cache_root, source)?;
            progress.advance(StackColorProgressPhase::LoadingSources, 1);
            frame
        };
        frames.insert(source.role, frame);
    }
    if reused_inputs {
        progress.reuse(
            StackColorProgressPhase::LoadingSources,
            "Reused prepared input channels",
        );
    } else {
        progress.finish(StackColorProgressPhase::LoadingSources);
    }

    if reused_inputs {
        progress.reuse(
            StackColorProgressPhase::BackgroundPreparation,
            "Reused prepared backgrounds",
        );
    } else if let Some(extraction) = job
        .processing
        .as_ref()
        .and_then(|processing| processing.background_extraction.as_ref())
    {
        pool.install(|| {
            for source in &job.sources {
                let frame = if source.role == reference_role {
                    &mut reference
                } else {
                    frames
                        .get_mut(&source.role)
                        .ok_or_else(|| format!("{} source was not loaded", source.role.label()))?
                };
                progress.begin(
                    StackColorProgressPhase::BackgroundPreparation,
                    format!("Fitting {} background", source.role.label()),
                    Some(source.role),
                    None,
                );
                let (fit, fallback) = fit_channel_background(
                    &frame.image,
                    extraction,
                    background_regions
                        .get(&source.role)
                        .cloned()
                        .unwrap_or_default(),
                    |error| {
                        tracing::warn!(
                            "Protected {} background fit failed; retrying without catalog protection: {error}",
                            source.role.label()
                        );
                        progress.begin(
                            StackColorProgressPhase::BackgroundPreparation,
                            format!(
                                "Retrying {} background without catalog protection",
                                source.role.label()
                            ),
                            Some(source.role),
                            None,
                        );
                    },
                )
                .map_err(|error| {
                    format!("Failed to fit {} background: {error}", source.role.label())
                })?;
                progress.advance(StackColorProgressPhase::BackgroundPreparation, 1);
                progress.begin(
                    StackColorProgressPhase::BackgroundPreparation,
                    format!("Correcting {} background", source.role.label()),
                    Some(source.role),
                    None,
                );
                fit.correct_in_place_with_strength(
                    &mut frame.image.data,
                    extraction.correction_mode,
                    extraction.strength,
                )
                .map_err(|error| {
                    format!(
                        "Failed to correct {} background: {error}",
                        source.role.label()
                    )
                })?;
                progress.advance(StackColorProgressPhase::BackgroundPreparation, 1);
                state.stack_previews.update_color(&job.job_id, |current| {
                    current
                        .resolved_backgrounds
                        .insert(source.role, fit.clone());
                    if let Some(reason) = fallback.as_ref() {
                        current
                            .background_protection_fallbacks
                            .insert(source.role, reason.clone());
                    }
                });
                if let Some(reason) = fallback {
                    background_protection_fallbacks.insert(source.role, reason);
                }
                resolved_backgrounds.insert(source.role, fit);
            }
            Ok::<(), String>(())
        })?;
        progress.finish(StackColorProgressPhase::BackgroundPreparation);
    } else {
        progress.skip(
            StackColorProgressPhase::BackgroundPreparation,
            "Background preparation skipped (disabled)",
        );
    }

    pool.install(|| {
        let reference_headers = reference.headers;
        let reference_image = reference.image;
        let mut images = BTreeMap::new();
        images.insert(reference_role, reference_image);
        if reused_inputs {
            images.extend(frames.into_iter().map(|(role, frame)| (role, frame.image)));
            progress.reuse(
                StackColorProgressPhase::RegisteringSources,
                "Reused registered input channels",
            );
        } else {
            progress.begin(
                StackColorProgressPhase::RegisteringSources,
                format!("Using {} as registration reference", reference_role.label()),
                Some(reference_role),
                None,
            );
            let registrar =
                Registrar::new(&images[&reference_role], RegistrationOptions::default())
                    .map_err(|error| error.to_string())?;
            progress.advance(StackColorProgressPhase::RegisteringSources, 1);
            state.stack_previews.update_color(&job.job_id, |current| {
                current.processed_channels = 1;
            });

            for source in job
                .sources
                .iter()
                .filter(|source| source.role != reference_role)
            {
                progress.begin(
                    StackColorProgressPhase::RegisteringSources,
                    format!("Registering {}", source.role.label()),
                    Some(source.role),
                    None,
                );
                let frame = frames
                    .remove(&source.role)
                    .ok_or_else(|| format!("{} source was not loaded", source.role.label()))?;
                let registration = registrar.register(&frame.image).map_err(|error| {
                    format!(
                        "Failed to register {} to {}: {error}",
                        source.role.label(),
                        reference_role.label()
                    )
                })?;
                if registration.rms_error_pixels > MAX_REGISTRATION_RMS_PIXELS {
                    return Err(format!(
                        "{} registration RMS {:.3}px exceeds {:.3}px",
                        source.role.label(),
                        registration.rms_error_pixels,
                        MAX_REGISTRATION_RMS_PIXELS
                    ));
                }
                tracing::info!(
                    "Stack color {} registered {}: {:.3}px RMS, {:.1}px drift, {} stars",
                    job.job_id,
                    source.role.label(),
                    registration.rms_error_pixels,
                    registration.drift_pixels,
                    registration.matched_stars
                );
                let aligned = resample_to_reference(
                    &frame.image,
                    images[&reference_role].width,
                    images[&reference_role].height,
                    registration.transform,
                )
                .map_err(|error| {
                    format!(
                        "Failed to resample {} onto the {} reference: {error}",
                        source.role.label(),
                        reference_role.label()
                    )
                })?;
                registered_transforms.insert(source.role, registration.transform);
                state.stack_previews.update_color(&job.job_id, |current| {
                    if let Some(current_source) = current
                        .sources
                        .iter_mut()
                        .find(|current_source| current_source.role == source.role)
                    {
                        current_source.registration_transform = Some(registration.transform);
                    }
                });
                images.insert(source.role, aligned);
                progress.advance(StackColorProgressPhase::RegisteringSources, 1);
                state.stack_previews.update_color(&job.job_id, |current| {
                    current.processed_channels += 1;
                });
            }
            progress.finish(StackColorProgressPhase::RegisteringSources);
        }

        let options = if let Some(processing) = &job.processing {
            if reused_inputs {
                progress.reuse(
                    StackColorProgressPhase::DeconvolvingInputs,
                    "Reused prepared deconvolution",
                );
                progress.reuse(
                    StackColorProgressPhase::NormalizingInputs,
                    "Reused normalized input channels",
                );
            } else {
                if processing.input_deconvolutions.is_empty() {
                    progress.skip(
                        StackColorProgressPhase::DeconvolvingInputs,
                        "Input deconvolution skipped (disabled)",
                    );
                } else {
                    for source in &job.sources {
                        let Some(request) =
                            processing.input_deconvolutions.get(&source.role).copied()
                        else {
                            continue;
                        };
                        progress.begin(
                            StackColorProgressPhase::DeconvolvingInputs,
                            format!("Deconvolving {}", source.role.label()),
                            Some(source.role),
                            None,
                        );
                        let image = images.remove(&source.role).ok_or_else(|| {
                            format!("{} registered image is missing", source.role.label())
                        })?;
                        let role_label = source.role.label();
                        let (restored, result) = super::stretch::apply_deconvolution(
                            &image, request,
                        )
                        .map_err(|error| format!("Failed to deconvolve {role_label}: {error}"))?;
                        images.insert(source.role, restored);
                        resolved_deconvolutions.insert(source.role, result.clone());
                        state.stack_previews.update_color(&job.job_id, |current| {
                            current
                                .resolved_input_deconvolutions
                                .insert(source.role, result);
                        });
                        progress.advance(StackColorProgressPhase::DeconvolvingInputs, 1);
                    }
                    progress.finish(StackColorProgressPhase::DeconvolvingInputs);
                }

                progress.begin(
                    StackColorProgressPhase::NormalizingInputs,
                    "Normalizing input channels",
                    None,
                    None,
                );
                for source in &job.sources {
                    progress.begin(
                        StackColorProgressPhase::NormalizingInputs,
                        format!("Normalizing {}", source.role.label()),
                        Some(source.role),
                        None,
                    );
                    let image = images.remove(&source.role).ok_or_else(|| {
                        format!("{} registered image is missing", source.role.label())
                    })?;
                    let normalized = super::stretch::normalize_linear_image(&image)?.0;
                    images.insert(source.role, normalized);
                    progress.advance(StackColorProgressPhase::NormalizingInputs, 1);
                }
                progress.finish(StackColorProgressPhase::NormalizingInputs);
                if let Some(input_id) = job.linear_input_id.as_deref() {
                    let manifest = CachedColorInputs {
                        schema_version: COLOR_INPUT_CACHE_VERSION,
                        input_id: input_id.into(),
                        roles: job.sources.iter().map(|source| source.role).collect(),
                        registered_transforms: registered_transforms.clone(),
                        resolved_backgrounds: resolved_backgrounds.clone(),
                        background_protection_fallbacks: background_protection_fallbacks.clone(),
                        resolved_deconvolutions: resolved_deconvolutions.clone(),
                    };
                    store_cached_color_inputs(
                        cache_root,
                        input_id,
                        &job.sources,
                        &images,
                        &reference_headers,
                        &manifest,
                    )?;
                }
            }

            let input_stage_count = processing
                .input_stretches
                .values()
                .map(Vec::len)
                .sum::<usize>();
            if input_stage_count == 0 {
                progress.skip(
                    StackColorProgressPhase::StretchingInputs,
                    "Input stretch stages skipped",
                );
            } else {
                for source in &job.sources {
                    let Some(stages) = processing.input_stretches.get(&source.role) else {
                        continue;
                    };
                    if stages.is_empty() {
                        continue;
                    }
                    let configs = stages
                        .iter()
                        .map(super::stretch::StackStretchRequest::config)
                        .collect::<Vec<_>>();
                    let stack = StretchStack::new(configs).map_err(|error| error.to_string())?;
                    let image = images.remove(&source.role).ok_or_else(|| {
                        format!("{} normalized image is missing", source.role.label())
                    })?;
                    let output = apply_input_stretch_stack(&stack, &image.data, 1, |event| {
                        let number = event.stage_index + 1;
                        let action = match event.state {
                            seiza_stretch::StretchStageState::Resolving => "Resolving",
                            seiza_stretch::StretchStageState::Applying => "Applying",
                            seiza_stretch::StretchStageState::Completed => "Applied",
                        };
                        progress.begin(
                            StackColorProgressPhase::StretchingInputs,
                            format!(
                                "{action} {} stretch {number}/{}",
                                source.role.label(),
                                event.stage_count
                            ),
                            Some(source.role),
                            Some((number, event.stage_count)),
                        );
                        if event.state == seiza_stretch::StretchStageState::Completed {
                            progress.advance(StackColorProgressPhase::StretchingInputs, 1);
                        }
                    })
                    .map_err(|error| error.to_string())?;
                    let resolved = output
                        .plans
                        .iter()
                        .map(|plan| serde_json::to_value(plan).map_err(|error| error.to_string()))
                        .collect::<Result<Vec<_>, _>>()?;
                    state.stack_previews.update_color(&job.job_id, |current| {
                        current
                            .resolved_input_stretches
                            .insert(source.role, resolved);
                    });
                    images.insert(
                        source.role,
                        LinearImage::new(image.width, image.height, 1, output.data)
                            .map_err(|error| error.to_string())?,
                    );
                }
                progress.finish(StackColorProgressPhase::StretchingInputs);
            }
            ColorOptions {
                normalization: ColorNormalization::None,
                input_transfer: ColorTransfer::DisplayReferred,
                crop: job.crop.seiza(),
            }
        } else {
            progress.begin(
                StackColorProgressPhase::NormalizingInputs,
                "Preparing legacy quick-look normalization",
                None,
                None,
            );
            progress.skip(
                StackColorProgressPhase::DeconvolvingInputs,
                "Input deconvolution skipped (legacy quick look)",
            );
            progress.skip(
                StackColorProgressPhase::StretchingInputs,
                "Input stretch stages skipped (legacy quick look)",
            );
            ColorOptions {
                crop: job.crop.seiza(),
                ..ColorOptions::default()
            }
        };

        progress.begin(
            StackColorProgressPhase::ComposingColor,
            format!("Composing {}", job.label),
            None,
            None,
        );
        // The order the composition receives its channels, which is the order
        // its coverage report preserves.
        let composed_roles: Vec<StackColorRole> = match job.kind {
            StackColorKind::Rgb => vec![
                StackColorRole::Red,
                StackColorRole::Green,
                StackColorRole::Blue,
            ],
            StackColorKind::Lrgb => vec![
                StackColorRole::Luminance,
                StackColorRole::Red,
                StackColorRole::Green,
                StackColorRole::Blue,
            ],
            StackColorKind::Narrowband => {
                let mut roles = vec![StackColorRole::Ha, StackColorRole::Oiii];
                // A palette that does not use SII leaves it out of the
                // composition, and so out of the report.
                if job
                    .palette
                    .is_some_and(|palette| palette.seiza().requires_sii())
                {
                    roles.push(StackColorRole::Sii);
                }
                roles
            }
        };
        let mut composition = match job.kind {
            StackColorKind::Rgb => combine_rgb(
                &images[&StackColorRole::Red],
                &images[&StackColorRole::Green],
                &images[&StackColorRole::Blue],
                &options,
            ),
            StackColorKind::Lrgb => combine_lrgb(
                &images[&StackColorRole::Luminance],
                &images[&StackColorRole::Red],
                &images[&StackColorRole::Green],
                &images[&StackColorRole::Blue],
                1.0,
                &options,
            ),
            StackColorKind::Narrowband => {
                let palette = job.palette.expect("validated narrowband palette");
                combine_narrowband(
                    &images[&StackColorRole::Ha],
                    &images[&StackColorRole::Oiii],
                    images.get(&StackColorRole::Sii),
                    palette.seiza(),
                    &options,
                    &ForaxxOptions::default(),
                )
            }
        }
        .map_err(|error| error.to_string())?;
        if job.processing.is_none() {
            progress.finish(StackColorProgressPhase::NormalizingInputs);
        }
        if let Some(report) = composition.crop.as_ref() {
            let report = StackColorCropReport::from_seiza(report, &composed_roles);
            for channel in report.channels.iter().filter(|channel| channel.off_center) {
                tracing::warn!(
                    channel = %channel.name,
                    offset_pixels = channel.center_offset_pixels,
                    job_id = %job.job_id,
                    "color channel sits off center from the others and bounds the crop"
                );
            }
            state.stack_previews.update_color(&job.job_id, |current| {
                current.crop_report = Some(report.clone());
            });
        }
        progress.finish(StackColorProgressPhase::ComposingColor);

        if let Some(processing) = &job.processing {
            if processing.output_stretches.is_empty() {
                progress.skip(
                    StackColorProgressPhase::StretchingOutput,
                    "Output stretch stages skipped",
                );
            } else {
                let configs = processing
                    .output_stretches
                    .iter()
                    .map(super::stretch::StackStretchRequest::config)
                    .collect::<Vec<_>>();
                let stack = StretchStack::new(configs).map_err(|error| error.to_string())?;
                let output = stack
                    .apply_f32_with_progress(&composition.image.data, 3, |event| {
                        let number = event.stage_index + 1;
                        let action = match event.state {
                            seiza_stretch::StretchStageState::Resolving => "Resolving",
                            seiza_stretch::StretchStageState::Applying => "Applying",
                            seiza_stretch::StretchStageState::Completed => "Applied",
                        };
                        progress.begin(
                            StackColorProgressPhase::StretchingOutput,
                            format!("{action} output stretch {number}/{}", event.stage_count),
                            None,
                            Some((number, event.stage_count)),
                        );
                        if event.state == seiza_stretch::StretchStageState::Completed {
                            progress.advance(StackColorProgressPhase::StretchingOutput, 1);
                        }
                    })
                    .map_err(|error| error.to_string())?;
                let resolved = output
                    .plans
                    .iter()
                    .map(|plan| serde_json::to_value(plan).map_err(|error| error.to_string()))
                    .collect::<Result<Vec<_>, _>>()?;
                state.stack_previews.update_color(&job.job_id, |current| {
                    current.resolved_output_stretches = resolved;
                });
                composition = ColorComposition {
                    image: LinearImage::new(
                        composition.image.width,
                        composition.image.height,
                        3,
                        output.data,
                    )
                    .map_err(|error| error.to_string())?,
                    transfer: ColorTransfer::DisplayReferred,
                    ..composition
                };
                progress.finish(StackColorProgressPhase::StretchingOutput);
            }
        } else {
            progress.skip(
                StackColorProgressPhase::StretchingOutput,
                "Output stretch stages skipped (legacy quick look)",
            );
        }

        progress.begin(
            StackColorProgressPhase::WritingFits,
            "Writing color FITS",
            None,
            None,
        );
        let fits_destination = color_fits_path(cache_root, &job.job_id);
        let parent = fits_destination
            .parent()
            .ok_or_else(|| "Color FITS path has no parent".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let temporary = fits_destination.with_extension(format!("{}.tmp.fits", std::process::id()));
        write_color_fits_f32(&temporary, &composition, &reference_headers, &job.label)
            .map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, &fits_destination).map_err(|error| error.to_string())?;
        progress.finish(StackColorProgressPhase::WritingFits);
        let stretch_config = match composition.transfer {
            ColorTransfer::LinearLight => super::stretch::default_linear_config(),
            ColorTransfer::DisplayReferred => super::stretch::display_identity_config(),
        };
        let source_transfer = match composition.transfer {
            ColorTransfer::LinearLight => super::stretch::StackStretchSourceTransfer::Linear,
            ColorTransfer::DisplayReferred => {
                super::stretch::StackStretchSourceTransfer::DisplayReferred
            }
        };
        let mut active_render = None;
        let rendered = super::stretch::render_image_previews_atomic_with_progress(
            &composition.image,
            &stretch_config,
            source_transfer,
            &color_preview_path(cache_root, &job.job_id),
            &color_original_preview_path(cache_root, &job.job_id),
            |render_phase| {
                if let Some(previous) = active_render.replace(render_phase) {
                    progress.finish(render_progress_phase(previous));
                }
                progress.begin(
                    render_progress_phase(render_phase),
                    render_progress_label(render_phase),
                    None,
                    None,
                );
            },
        );
        if let Some(active) = active_render {
            progress.finish(render_progress_phase(active));
        }
        rendered.map(|_| ())
    })
}

fn load_source_frame(cache_root: &FsPath, source: &StackColorSource) -> Result<FitsFrame, String> {
    let frame = crate::image_io::open_linear_frame(super::fits_path(
        cache_root,
        &source.job_id,
        source.group_index,
    ))
    .map_err(|error| {
        format!(
            "Failed to read {} channel stack: {error}",
            source.role.label()
        )
    })?;
    validate_mono(&frame.image, source.role)?;
    Ok(frame)
}

fn load_cached_color_input_manifest(
    cache_root: &FsPath,
    input_id: &str,
    reference_role: StackColorRole,
    sources: &[StackColorSource],
) -> Option<(CachedColorInputs, FitsFrame)> {
    let bytes = std::fs::read(color_input_manifest_path(cache_root, input_id)).ok()?;
    let manifest = serde_json::from_slice::<CachedColorInputs>(&bytes).ok()?;
    let roles = sources.iter().map(|source| source.role).collect::<Vec<_>>();
    if manifest.schema_version != COLOR_INPUT_CACHE_VERSION
        || manifest.input_id != input_id
        || manifest.roles != roles
    {
        return None;
    }
    if !roles
        .iter()
        .all(|role| color_input_fits_path(cache_root, input_id, *role).is_file())
    {
        return None;
    }
    let reference = crate::image_io::open_linear_frame(color_input_fits_path(
        cache_root,
        input_id,
        reference_role,
    ))
    .ok()?;
    validate_mono(&reference.image, reference_role).ok()?;
    Some((manifest, reference))
}

fn store_cached_color_inputs(
    cache_root: &FsPath,
    input_id: &str,
    sources: &[StackColorSource],
    images: &BTreeMap<StackColorRole, LinearImage>,
    reference_headers: &[(String, seiza_fits::HeaderValue)],
    manifest: &CachedColorInputs,
) -> Result<(), String> {
    for source in sources {
        let image = images
            .get(&source.role)
            .ok_or_else(|| format!("{} prepared image is missing", source.role.label()))?;
        let destination = color_input_fits_path(cache_root, input_id, source.role);
        let parent = destination
            .parent()
            .ok_or_else(|| "Color input FITS path has no parent".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let temporary = destination.with_extension(format!("{}.tmp.fits", std::process::id()));
        write_processed_image_fits_f32(&temporary, image, reference_headers, &[])
            .map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, &destination).map_err(|error| error.to_string())?;
    }
    write_json_atomic(&color_input_manifest_path(cache_root, input_id), manifest)
}

fn render_progress_phase(
    phase: super::stretch::StackPreviewRenderPhase,
) -> StackColorProgressPhase {
    match phase {
        super::stretch::StackPreviewRenderPhase::Original => {
            StackColorProgressPhase::RenderingOriginal
        }
        super::stretch::StackPreviewRenderPhase::Screen => StackColorProgressPhase::RenderingScreen,
    }
}

fn render_progress_label(phase: super::stretch::StackPreviewRenderPhase) -> &'static str {
    match phase {
        super::stretch::StackPreviewRenderPhase::Original => "Rendering full-size preview",
        super::stretch::StackPreviewRenderPhase::Screen => "Rendering screen preview",
    }
}

fn validate_mono(image: &LinearImage, role: StackColorRole) -> Result<(), String> {
    if image.channels == 1 {
        Ok(())
    } else {
        Err(format!(
            "{} stack has {} channels; mono-stack color composition requires one channel",
            role.label(),
            image.channels
        ))
    }
}

fn load_latest_stacks(
    ctx: &crate::server::database_context::DatabaseContext,
    project_id: i32,
) -> Result<LatestStackPreviews, AppError> {
    match std::fs::read(super::latest_path(&ctx.cache_dir_path, project_id)) {
        Ok(bytes) => {
            let latest: LatestStackPreviews = serde_json::from_slice(&bytes).map_err(|error| {
                AppError::InternalError(format!("Invalid latest stack preview index: {error}"))
            })?;
            if latest.database_id != ctx.id || latest.project_id != project_id {
                return Err(AppError::NotFound);
            }
            Ok(super::current_latest_stacks(latest))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(LatestStackPreviews {
            schema_version: 1,
            database_id: ctx.id.clone(),
            project_id,
            updated_unix_seconds: 0,
            groups: Vec::new(),
        }),
        Err(error) => Err(AppError::InternalError(format!(
            "Failed to read latest stack preview index: {error}"
        ))),
    }
}

fn load_latest_colors(
    ctx: &crate::server::database_context::DatabaseContext,
    project_id: i32,
) -> Result<LatestStackColorPreviews, AppError> {
    match std::fs::read(latest_color_path(&ctx.cache_dir_path, project_id)) {
        Ok(bytes) => {
            let latest: LatestStackColorPreviews =
                serde_json::from_slice(&bytes).map_err(|error| {
                    AppError::InternalError(format!("Invalid latest color preview index: {error}"))
                })?;
            if latest.database_id != ctx.id || latest.project_id != project_id {
                return Err(AppError::NotFound);
            }
            Ok(current_latest_colors(latest))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(LatestStackColorPreviews {
                schema_version: 1,
                database_id: ctx.id.clone(),
                project_id,
                updated_unix_seconds: 0,
                jobs: Vec::new(),
            })
        }
        Err(error) => Err(AppError::InternalError(format!(
            "Failed to read latest color preview index: {error}"
        ))),
    }
}

fn collect_sources(
    cache_root: &FsPath,
    latest: &LatestStackPreviews,
) -> BTreeMap<i32, TargetSources> {
    let mut targets = BTreeMap::<i32, TargetSources>::new();
    for entry in &latest.groups {
        if super::validate_job_id(&entry.job_id).is_err() {
            continue;
        }
        if !super::fits_path(cache_root, &entry.job_id, entry.group.index).is_file() {
            continue;
        }
        let target = targets.entry(entry.group.target_id).or_default();
        target.target_name = entry.group.target_name.clone();
        let Some(role) = classify_filter(&entry.group.filter_name) else {
            target
                .unmapped_filters
                .push(entry.group.filter_name.clone());
            continue;
        };
        target
            .by_role
            .entry(role)
            .or_default()
            .push(StackColorSource {
                role,
                filter_name: entry.group.filter_name.clone(),
                job_id: entry.job_id.clone(),
                group_index: entry.group.index,
                artifact_revision: entry.artifact_revision.clone(),
                accepted_frames: entry.group.accepted_frames,
                reference_image_id: entry.group.reference_image_id,
                sky_orientation: entry.group.sky_orientation.clone(),
                registration_transform: None,
            });
    }
    for target in targets.values_mut() {
        target.unmapped_filters.sort();
        target.unmapped_filters.dedup();
    }
    targets
}

fn availability(sources: &BTreeMap<i32, TargetSources>) -> Vec<StackColorTargetAvailability> {
    sources
        .iter()
        .map(|(target_id, target)| {
            let mut available_roles = Vec::new();
            let mut ambiguous_roles = Vec::new();
            for (role, candidates) in &target.by_role {
                match candidates.as_slice() {
                    [source] => available_roles.push(StackColorAvailableRole {
                        role: *role,
                        filter_name: source.filter_name.clone(),
                    }),
                    _ => ambiguous_roles.push(*role),
                }
            }
            let unique = available_roles
                .iter()
                .map(|available| available.role)
                .collect::<HashSet<_>>();
            let rgb_available = [
                StackColorRole::Red,
                StackColorRole::Green,
                StackColorRole::Blue,
            ]
            .iter()
            .all(|role| unique.contains(role));
            let lrgb_available = rgb_available && unique.contains(&StackColorRole::Luminance);
            let has_ha_oiii =
                unique.contains(&StackColorRole::Ha) && unique.contains(&StackColorRole::Oiii);
            let narrowband_palettes = if has_ha_oiii {
                StackNarrowbandPalette::all(unique.contains(&StackColorRole::Sii))
            } else {
                Vec::new()
            };
            StackColorTargetAvailability {
                target_id: *target_id,
                target_name: target.target_name.clone(),
                available_roles,
                ambiguous_roles,
                unmapped_filters: target.unmapped_filters.clone(),
                rgb_available,
                lrgb_available,
                narrowband_palettes,
            }
        })
        .collect()
}

fn classify_filter(filter_name: &str) -> Option<StackColorRole> {
    let folded = filter_name
        .to_lowercase()
        .replace('α', "alpha")
        .replace('β', "beta");
    let compact = folded
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    let exact = match compact.as_str() {
        "l" | "lum" | "luminance" => Some(StackColorRole::Luminance),
        "r" | "red" => Some(StackColorRole::Red),
        "g" | "green" => Some(StackColorRole::Green),
        "b" | "blue" => Some(StackColorRole::Blue),
        "ha" | "halpha" | "hydrogenalpha" => Some(StackColorRole::Ha),
        "oiii" | "o3" | "oxygeniii" => Some(StackColorRole::Oiii),
        "sii" | "s2" | "sulfurii" | "sulphurii" => Some(StackColorRole::Sii),
        _ => None,
    };
    if exact.is_some() {
        return exact;
    }
    let distinctive_suffix = [
        (StackColorRole::Ha, ["halpha", "hydrogenalpha"].as_slice()),
        (StackColorRole::Oiii, ["oiii", "oxygeniii"].as_slice()),
        (
            StackColorRole::Sii,
            ["sii", "sulfurii", "sulphurii"].as_slice(),
        ),
    ]
    .into_iter()
    .filter_map(|(role, aliases)| {
        aliases
            .iter()
            .any(|alias| compact.ends_with(alias))
            .then_some(role)
    })
    .collect::<Vec<_>>();
    if let [role] = distinctive_suffix.as_slice() {
        return Some(*role);
    }
    let tokens = folded
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect::<HashSet<_>>();
    let mut candidates = [
        (StackColorRole::Luminance, ["lum", "luminance"].as_slice()),
        (StackColorRole::Red, ["red"].as_slice()),
        (StackColorRole::Green, ["green"].as_slice()),
        (StackColorRole::Blue, ["blue"].as_slice()),
        (StackColorRole::Ha, ["ha", "halpha"].as_slice()),
        (StackColorRole::Oiii, ["oiii", "o3"].as_slice()),
        (StackColorRole::Sii, ["sii", "s2"].as_slice()),
    ]
    .into_iter()
    .filter_map(|(role, aliases)| {
        aliases
            .iter()
            .any(|alias| tokens.contains(alias))
            .then_some(role)
    })
    .collect::<Vec<_>>();
    if tokens.contains("h") && tokens.contains("alpha") {
        candidates.push(StackColorRole::Ha);
    }
    if tokens.contains("oxygen") && tokens.contains("iii") {
        candidates.push(StackColorRole::Oiii);
    }
    if (tokens.contains("sulfur") || tokens.contains("sulphur")) && tokens.contains("ii") {
        candidates.push(StackColorRole::Sii);
    }
    candidates.sort_unstable();
    candidates.dedup();
    match candidates.as_slice() {
        [role] => Some(*role),
        _ => None,
    }
}

fn source_is_current(source: &StackColorSource, latest: &LatestStackPreviews) -> bool {
    latest.groups.iter().any(|entry| {
        entry.job_id == source.job_id
            && entry.artifact_revision == source.artifact_revision
            && entry.group.index == source.group_index
            && entry.group.filter_name == source.filter_name
    })
}

fn color_artifacts_exist(cache_root: &FsPath, job_id: &str) -> bool {
    color_preview_path(cache_root, job_id).is_file()
        && color_original_preview_path(cache_root, job_id).is_file()
        && color_fits_path(cache_root, job_id).is_file()
}

fn color_job_outdated_reason(
    ctx: &crate::server::database_context::DatabaseContext,
    job: &StackColorJob,
    latest: &LatestStackPreviews,
) -> Result<Option<String>, AppError> {
    if job.cache_version != STACK_COLOR_CACHE_VERSION
        || job.stacking_version != SEIZA_STACKING_VERSION
        || job.background_version != SEIZA_BACKGROUND_VERSION
        || (job
            .processing
            .as_ref()
            .is_some_and(|processing| !processing.input_deconvolutions.is_empty())
            && job.deconvolution_version != super::stretch::deconvolution_version())
    {
        return Ok(Some("the color processing version changed".into()));
    }
    if !job.sources.iter().all(|source| {
        source_is_current(source, latest)
            && super::fits_path(&ctx.cache_dir_path, &source.job_id, source.group_index).is_file()
    }) {
        return Ok(Some("one or more source channel stacks changed".into()));
    }
    if job.processing.as_ref().is_some_and(|processing| {
        processing
            .background_extraction
            .as_ref()
            .is_some_and(|extraction| extraction.protect_catalog_emission)
    }) {
        let current = resolve_background_protection(ctx, &job.sources)?
            .into_iter()
            .map(|(role, protection)| (role, protection.summary))
            .collect::<BTreeMap<_, _>>();
        if current != job.resolved_background_protection {
            return Ok(Some("the plate-solve background protection changed".into()));
        }
    }
    if !color_artifacts_exist(&ctx.cache_dir_path, &job.job_id) {
        return Ok(Some("a cached color artifact is missing".into()));
    }
    Ok(None)
}

fn persist_color_manifest(cache_root: &FsPath, job: &StackColorJob) -> Result<(), String> {
    write_json_atomic(&color_manifest_path(cache_root, &job.job_id), job)
}

fn persist_latest_color(cache_root: &FsPath, job: &StackColorJob) -> Result<(), String> {
    let path = latest_color_path(cache_root, job.project_id);
    let mut latest = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<LatestStackColorPreviews>(&bytes).ok())
        .filter(|value| value.database_id == job.database_id && value.project_id == job.project_id)
        .map(current_latest_colors)
        .unwrap_or_else(|| LatestStackColorPreviews {
            schema_version: 1,
            database_id: job.database_id.clone(),
            project_id: job.project_id,
            updated_unix_seconds: 0,
            jobs: Vec::new(),
        });
    if let Some(existing) = latest.jobs.iter_mut().find(|existing| {
        existing.target_id == job.target_id
            && existing.kind == job.kind
            && existing.palette == job.palette
    }) {
        *existing = job.clone();
    } else {
        latest.jobs.push(job.clone());
    }
    latest.updated_unix_seconds = chrono::Utc::now().timestamp();
    write_json_atomic(&path, &latest)
}

fn current_latest_colors(mut latest: LatestStackColorPreviews) -> LatestStackColorPreviews {
    latest.jobs.retain(|job| {
        job.cache_version == STACK_COLOR_CACHE_VERSION
            && job.stacking_version == SEIZA_STACKING_VERSION
    });
    latest
}

fn write_json_atomic(path: &FsPath, value: &impl Serialize) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Color manifest path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn color_dir(cache_root: &FsPath, job_id: &str) -> PathBuf {
    cache_root.join("stack-previews").join("color").join(job_id)
}

fn color_manifest_path(cache_root: &FsPath, job_id: &str) -> PathBuf {
    color_dir(cache_root, job_id).join("manifest.json")
}

fn color_preview_path(cache_root: &FsPath, job_id: &str) -> PathBuf {
    color_dir(cache_root, job_id).join("preview.png")
}

pub(super) fn color_original_preview_path(cache_root: &FsPath, job_id: &str) -> PathBuf {
    color_dir(cache_root, job_id).join("preview-original.png")
}

fn color_fits_path(cache_root: &FsPath, job_id: &str) -> PathBuf {
    color_dir(cache_root, job_id).join("color.fits")
}

fn color_input_dir(cache_root: &FsPath, input_id: &str) -> PathBuf {
    cache_root
        .join("stack-previews")
        .join("color-inputs")
        .join(input_id)
}

fn color_input_manifest_path(cache_root: &FsPath, input_id: &str) -> PathBuf {
    color_input_dir(cache_root, input_id).join("manifest.json")
}

fn color_input_fits_path(cache_root: &FsPath, input_id: &str, role: StackColorRole) -> PathBuf {
    color_input_dir(cache_root, input_id).join(format!("{}.fits", role_cache_name(role)))
}

fn role_cache_name(role: StackColorRole) -> &'static str {
    match role {
        StackColorRole::Luminance => "luminance",
        StackColorRole::Red => "red",
        StackColorRole::Green => "green",
        StackColorRole::Blue => "blue",
        StackColorRole::Ha => "ha",
        StackColorRole::Oiii => "oiii",
        StackColorRole::Sii => "sii",
    }
}

/// Job and cached-input references from every project's durable color index.
pub(super) fn latest_color_references(cache_root: &FsPath) -> Vec<(String, Option<String>)> {
    super::read_latest_indices::<LatestStackColorPreviews>(
        &cache_root.join("stack-previews").join("color"),
    )
    .into_iter()
    .flat_map(|latest| latest.jobs)
    .map(|job| (job.job_id, job.linear_input_id))
    .collect()
}

fn latest_color_path(cache_root: &FsPath, project_id: i32) -> PathBuf {
    cache_root
        .join("stack-previews")
        .join("color")
        .join(format!("latest-project-{project_id}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::stack_preview::{
        LatestStackPreviewGroup, StackGroupState, StackGroupStatus, StackSkyOrientation,
    };

    fn stack_orientation(
        output_width: usize,
        output_height: usize,
        source_to_output: seiza_stacking::AffineTransform,
    ) -> StackSkyOrientation {
        StackSkyOrientation {
            convention: seiza_stacking::SKY_ORIENTATION_NAME.into(),
            version: seiza_stacking::SKY_ORIENTATION_VERSION,
            source: "embedded_wcs".into(),
            output_width,
            output_height,
            source_to_output,
        }
    }

    fn source_group(filter_name: &str, index: usize) -> LatestStackPreviewGroup {
        LatestStackPreviewGroup {
            job_id: format!("{index:064x}"),
            artifact_revision: format!("rev-{index}"),
            accepted_only: false,
            created_unix_seconds: 10,
            cache_version: super::super::STACK_PREVIEW_CACHE_VERSION,
            group: StackGroupStatus {
                snr: None,
                snr_url: None,
                index,
                target_id: 7,
                target_name: "Color target".into(),
                filter_name: filter_name.into(),
                state: StackGroupState::Ready,
                phase: "ready".into(),
                total_candidates: 3,
                eligible_frames: 3,
                quality_excluded: 0,
                missing_files: 0,
                processed_frames: 3,
                accepted_frames: 3,
                rejected_frames: 0,
                reused_frames: 0,
                resume_note: None,
                output_channels: 1,
                sky_orientation: Some(stack_orientation(
                    100,
                    80,
                    seiza_stacking::AffineTransform::IDENTITY,
                )),
                reference_image_id: Some(1),
                total_exposure_seconds: 180.0,
                preview_url: None,
                fits_url: None,
                error: None,
                calibration: crate::calibration::AppliedCalibration::default(),
                input_images: Vec::new(),
                frames: Vec::new(),
            },
        }
    }

    fn background_solution(
        outlines: Vec<crate::astrometry::OverlayOutlineResponse>,
    ) -> crate::astrometry::AstrometrySolutionResponse {
        use crate::astrometry::{
            AstrometrySolutionResponse, CatalogObjectIdentity, OverlayObjectResponse, WcsResponse,
        };
        AstrometrySolutionResponse {
            center_ra_deg: 0.0,
            center_dec_deg: 0.0,
            pixel_scale_arcsec_per_pixel: 1.0,
            matched_stars: 30,
            rms_arcsec: 0.5,
            image_width: 101,
            image_height: 101,
            wcs: WcsResponse {
                crval: [0.0, 0.0],
                crpix: [50.0, 50.0],
                cd: [[0.0, 0.0], [0.0, 0.0]],
                ctype: ["RA---TAN".into(), "DEC--TAN".into()],
                cunit: ["deg".into(), "deg".into()],
                radesys: "ICRS".into(),
                equinox: 2000.0,
            },
            footprint: Vec::new(),
            objects: vec![OverlayObjectResponse {
                identity: CatalogObjectIdentity::default(),
                name: "NGC 1499".into(),
                common_name: "California Nebula".into(),
                kind: "nebula".into(),
                mag: None,
                x: 50.0,
                y: 50.0,
                semi_major_px: 30.0,
                semi_minor_px: 18.0,
                angle_deg: Some(12.0),
                ra_deg: 0.0,
                dec_deg: 0.0,
                prominence: None,
                discovered: None,
                near_capture: None,
                distance_au: None,
                direction_pa_deg: None,
                direction_angle_deg: None,
                outlines,
            }],
            catalog_version: Some("openngc-test".into()),
            capture_time: None,
        }
    }

    fn running_color_job(job_id: &str, state: StackJobState) -> StackColorJob {
        let mut progress = color_progress(3, None);
        progress.completed_units = 4;
        StackColorJob {
            schema_version: 1,
            job_id: job_id.into(),
            database_id: "db-test".into(),
            project_id: 7,
            target_id: 42,
            target_name: "Color target".into(),
            kind: StackColorKind::Narrowband,
            palette: Some(StackNarrowbandPalette::Sho),
            crop: StackColorCrop::None,
            crop_report: None,
            label: "SHO".into(),
            state,
            phase: "Registering source channels".into(),
            processed_channels: 1,
            total_channels: 3,
            progress,
            created_unix_seconds: 200,
            artifact_revision: "rev".into(),
            cache_version: STACK_COLOR_CACHE_VERSION,
            stacking_version: SEIZA_STACKING_VERSION.into(),
            background_version: SEIZA_BACKGROUND_VERSION.into(),
            deconvolution_version: String::new(),
            linear_input_id: None,
            sources: Vec::new(),
            processing: None,
            resolved_input_stretches: BTreeMap::new(),
            resolved_input_deconvolutions: BTreeMap::new(),
            resolved_output_stretches: Vec::new(),
            resolved_backgrounds: BTreeMap::new(),
            resolved_background_protection: BTreeMap::new(),
            background_protection_fallbacks: BTreeMap::new(),
            preview_url: String::new(),
            fits_url: String::new(),
            error: None,
            outdated: false,
            outdated_reason: None,
        }
    }

    #[test]
    fn catalog_outline_is_preferred_for_background_protection() {
        use crate::astrometry::{OverlayContourResponse, OverlayOutlineResponse};
        let solution = background_solution(vec![OverlayOutlineResponse {
            geometry_id: "outline".into(),
            source_record_id: "ngc1499".into(),
            role: "catalog-extent".into(),
            quality: "catalog".into(),
            level: None,
            contours: vec![OverlayContourResponse {
                closed: true,
                points: vec![[10.0, 20.0], [90.0, 20.0], [50.0, 80.0]],
            }],
        }]);

        let orientation = stack_orientation(101, 101, seiza_stacking::AffineTransform::IDENTITY);
        let (regions, names) = background_regions_from_solution(&solution, &orientation);

        assert_eq!(names, ["California Nebula"]);
        assert!(matches!(
            regions.as_slice(),
            [ProtectedRegion::Polygon { .. }]
        ));
    }

    #[test]
    fn background_protection_follows_the_sky_orientation_transform() {
        use crate::astrometry::{OverlayContourResponse, OverlayOutlineResponse};
        let solution = background_solution(vec![OverlayOutlineResponse {
            geometry_id: "outline".into(),
            source_record_id: "ngc1499".into(),
            role: "catalog-extent".into(),
            quality: "catalog".into(),
            level: None,
            contours: vec![OverlayContourResponse {
                closed: true,
                points: vec![[10.0, 20.0], [90.0, 20.0], [50.0, 80.0]],
            }],
        }]);
        let orientation = stack_orientation(
            101,
            101,
            seiza_stacking::AffineTransform {
                matrix: [[0.0, -1.0], [1.0, 0.0]],
                translation_x: 100.0,
                translation_y: 0.0,
            },
        );

        let (regions, _) = background_regions_from_solution(&solution, &orientation);

        let [ProtectedRegion::Polygon { points }] = regions.as_slice() else {
            panic!("expected one transformed polygon");
        };
        assert_eq!(points, &[[0.8, 0.1], [0.8, 0.9], [0.2, 0.5]]);
    }

    #[test]
    fn estimated_outline_falls_back_to_projected_catalog_ellipse() {
        use crate::astrometry::{OverlayContourResponse, OverlayOutlineResponse};
        let solution = background_solution(vec![OverlayOutlineResponse {
            geometry_id: "estimate".into(),
            source_record_id: "ngc1499".into(),
            role: "fallback-extent".into(),
            quality: "estimated".into(),
            level: None,
            contours: vec![OverlayContourResponse {
                closed: true,
                points: vec![[10.0, 20.0], [90.0, 20.0], [50.0, 80.0]],
            }],
        }]);

        let orientation = stack_orientation(101, 101, seiza_stacking::AffineTransform::IDENTITY);
        let (regions, _) = background_regions_from_solution(&solution, &orientation);

        assert!(matches!(
            regions.as_slice(),
            [ProtectedRegion::Polygon { .. }]
        ));
    }

    #[test]
    fn active_reports_running_color_jobs_by_phase_units() {
        let manager = StackPreviewManager::new();
        assert!(manager.insert_color(running_color_job("color-running", StackJobState::Running)));
        assert!(manager.insert_color(running_color_job(
            "color-finished",
            StackJobState::Completed
        )));

        let active = manager.active();
        assert_eq!(active.len(), 1);
        let entry = &active[0];
        assert_eq!(
            entry.kind,
            crate::server::stack_preview::StackActivityKind::Color
        );
        assert_eq!(entry.job_id, "color-running");
        assert_eq!(entry.database_id, "db-test");
        assert_eq!(entry.project_id, 7);
        assert_eq!(entry.label, "Color target · SHO");
        assert_eq!(entry.detail, "Registering source channels");
        assert_eq!(entry.processed_units, 4);
        assert_eq!(entry.total_units, color_progress(3, None).total_units);
    }

    #[test]
    fn active_falls_back_to_channel_counts_without_phase_units() {
        let manager = StackPreviewManager::new();
        let mut job = running_color_job("color-legacy", StackJobState::Running);
        job.progress = StackColorProgress::default();
        job.phase = String::new();
        assert!(manager.insert_color(job));

        let active = manager.active();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].detail, "Composing color");
        assert_eq!(active[0].processed_units, 1);
        assert_eq!(active[0].total_units, 3);
    }

    #[test]
    fn recognizes_common_scheduler_filter_names_conservatively() {
        assert_eq!(
            classify_filter("Luminance"),
            Some(StackColorRole::Luminance)
        );
        assert_eq!(
            classify_filter("Chroma Red 36mm"),
            Some(StackColorRole::Red)
        );
        assert_eq!(
            classify_filter("Antlia 3nm H-alpha"),
            Some(StackColorRole::Ha)
        );
        assert_eq!(
            classify_filter("H-alpha 3nm mounted"),
            Some(StackColorRole::Ha)
        );
        assert_eq!(
            classify_filter("Chroma Oxygen III 3nm"),
            Some(StackColorRole::Oiii)
        );
        assert_eq!(classify_filter("OIII"), Some(StackColorRole::Oiii));
        assert_eq!(classify_filter("S2"), Some(StackColorRole::Sii));
        assert_eq!(classify_filter("L-eXtreme"), None);
        assert_eq!(classify_filter("Red Green test"), None);
    }

    #[test]
    fn palette_requirements_match_two_and_three_filter_sets() {
        assert_eq!(
            StackNarrowbandPalette::all(false),
            vec![
                StackNarrowbandPalette::Hoo,
                StackNarrowbandPalette::ForaxxHoo
            ]
        );
        let three = StackNarrowbandPalette::all(true);
        assert!(three.contains(&StackNarrowbandPalette::Sho));
        assert!(three.contains(&StackNarrowbandPalette::ForaxxSho));
        assert!(three.contains(&StackNarrowbandPalette::Hoo));
    }

    #[test]
    fn duplicate_role_is_ambiguous_instead_of_picking_silently() {
        let cache = tempfile::tempdir().unwrap();
        let groups = vec![source_group("Ha", 0), source_group("H-alpha", 1)];
        for group in &groups {
            let path = super::super::fits_path(cache.path(), &group.job_id, group.group.index);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"fixture").unwrap();
        }
        let latest = LatestStackPreviews {
            schema_version: 1,
            database_id: "db".into(),
            project_id: 1,
            updated_unix_seconds: 1,
            groups,
        };
        let available = availability(&collect_sources(cache.path(), &latest));
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].ambiguous_roles, [StackColorRole::Ha]);
        assert!(available[0].narrowband_palettes.is_empty());
    }

    #[test]
    fn rgb_is_available_without_a_luminance_stack() {
        let cache = tempfile::tempdir().unwrap();
        let groups = vec![
            source_group("R", 0),
            source_group("G", 1),
            source_group("B", 2),
        ];
        for group in &groups {
            let path = super::super::fits_path(cache.path(), &group.job_id, group.group.index);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"fixture").unwrap();
        }
        let latest = LatestStackPreviews {
            schema_version: 1,
            database_id: "db".into(),
            project_id: 1,
            updated_unix_seconds: 1,
            groups,
        };

        let available = availability(&collect_sources(cache.path(), &latest));

        assert_eq!(available.len(), 1);
        assert!(available[0].rgb_available);
        assert!(!available[0].lrgb_available);
        assert_eq!(
            required_roles(StackColorKind::Rgb, None),
            [
                StackColorRole::Red,
                StackColorRole::Green,
                StackColorRole::Blue
            ]
        );
    }

    #[test]
    fn color_artifact_paths_are_separate_from_mono_groups() {
        let root = FsPath::new("/cache/db");
        assert_eq!(
            color_fits_path(root, "abc"),
            PathBuf::from("/cache/db/stack-previews/color/abc/color.fits")
        );
        assert_eq!(
            latest_color_path(root, 7),
            PathBuf::from("/cache/db/stack-previews/color/latest-project-7.json")
        );
    }

    #[test]
    fn cached_color_job_requires_screen_original_and_fits_artifacts() {
        let cache = tempfile::tempdir().unwrap();
        let job_id = "a".repeat(64);
        let paths = [
            color_preview_path(cache.path(), &job_id),
            color_original_preview_path(cache.path(), &job_id),
            color_fits_path(cache.path(), &job_id),
        ];
        std::fs::create_dir_all(paths[0].parent().unwrap()).unwrap();

        for path in &paths {
            assert!(!color_artifacts_exist(cache.path(), &job_id));
            std::fs::write(path, b"fixture").unwrap();
        }
        assert!(color_artifacts_exist(cache.path(), &job_id));
    }

    #[test]
    fn a_request_without_a_crop_keeps_the_whole_grid() {
        let request: StackColorRequest =
            serde_json::from_str(r#"{"target_id":1,"kind":"rgb"}"#).expect("older clients omit it");
        assert_eq!(request.crop, StackColorCrop::None);
        let inscribed: StackColorRequest =
            serde_json::from_str(r#"{"target_id":1,"kind":"rgb","crop":"inscribed"}"#).unwrap();
        assert_eq!(inscribed.crop, StackColorCrop::Inscribed);
        assert_eq!(
            serde_json::to_value(StackColorCrop::Bounds).unwrap(),
            serde_json::json!("bounds")
        );
    }

    #[test]
    fn a_crop_report_names_channels_by_composition_order() {
        let covered = |blank_rows: usize| {
            let values = (0..64 * 64)
                .map(|index| {
                    if index / 64 < blank_rows {
                        f32::NAN
                    } else {
                        0.25
                    }
                })
                .collect::<Vec<_>>();
            LinearImage::new(64, 64, 1, values).unwrap()
        };
        let composition = combine_rgb(
            &covered(0),
            &covered(2),
            &covered(6),
            &ColorOptions {
                normalization: ColorNormalization::None,
                crop: ColorCrop::Inscribed,
                ..ColorOptions::default()
            },
        )
        .unwrap();
        let report = StackColorCropReport::from_seiza(
            composition.crop.as_ref().expect("a cropped composition"),
            &[
                StackColorRole::Red,
                StackColorRole::Green,
                StackColorRole::Blue,
            ],
        );
        assert_eq!((report.grid_width, report.grid_height), (64, 64));
        assert_eq!((report.x, report.y), (0, 6));
        assert_eq!((report.width, report.height), (64, 58));
        assert!((report.retained_fraction - 58.0 / 64.0).abs() < 1.0e-9);
        assert_eq!(
            report
                .channels
                .iter()
                .map(|channel| channel.role)
                .collect::<Vec<_>>(),
            vec![
                Some(StackColorRole::Red),
                Some(StackColorRole::Green),
                Some(StackColorRole::Blue),
            ]
        );
        assert!(report.channels.iter().all(|channel| !channel.off_center));
    }

    #[test]
    fn rejects_palette_shape_mismatches_before_preparing_a_job() {
        assert!(validate_request(&StackColorRequest {
            target_id: 1,
            kind: StackColorKind::Rgb,
            palette: Some(StackNarrowbandPalette::Sho),
            force: false,
            crop: StackColorCrop::None,
            processing: None,
        })
        .is_err());
        assert!(validate_request(&StackColorRequest {
            target_id: 1,
            kind: StackColorKind::Lrgb,
            palette: Some(StackNarrowbandPalette::Sho),
            force: false,
            crop: StackColorCrop::None,
            processing: None,
        })
        .is_err());
        assert!(validate_request(&StackColorRequest {
            target_id: 1,
            kind: StackColorKind::Narrowband,
            palette: None,
            force: false,
            crop: StackColorCrop::None,
            processing: None,
        })
        .is_err());
    }

    #[test]
    fn rejects_client_background_regions_and_invalid_strength() {
        let request = |extraction| StackColorRequest {
            target_id: 1,
            kind: StackColorKind::Rgb,
            palette: None,
            force: false,
            crop: StackColorCrop::None,
            processing: Some(StackColorProcessing {
                background_extraction: Some(extraction),
                ..StackColorProcessing::default()
            }),
        };
        let mut invalid_strength = StackBackgroundExtraction {
            config: BackgroundConfig::default(),
            correction_mode: CorrectionMode::Subtract,
            strength: 1.1,
            protect_catalog_emission: true,
        };
        assert!(validate_request(&request(invalid_strength.clone())).is_err());

        invalid_strength.strength = 1.0;
        invalid_strength
            .config
            .protected_regions
            .push(ProtectedRegion::Ellipse {
                center: [0.5, 0.5],
                radii: [0.2, 0.1],
                rotation_degrees: 0.0,
            });
        assert!(validate_request(&request(invalid_strength)).is_err());
    }

    #[test]
    fn older_background_requests_keep_catalog_protection_enabled() {
        let extraction: StackBackgroundExtraction = serde_json::from_value(serde_json::json!({
            "config": BackgroundConfig::default(),
            "correction_mode": "subtract",
            "strength": 1.0
        }))
        .unwrap();

        assert!(extraction.protect_catalog_emission);
    }

    #[test]
    fn protected_background_fit_retries_without_regions_after_any_error() {
        let values = (0..64 * 64)
            .map(|index| {
                let x = (index % 64) as f32 / 63.0;
                let y = (index / 64) as f32 / 63.0;
                0.1 + x * 0.02 + y * 0.03
            })
            .collect();
        let image = LinearImage::new(64, 64, 1, values).unwrap();
        let whole_image = ProtectedRegion::Polygon {
            points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        };
        let mut fallback_messages = Vec::new();
        let extraction = StackBackgroundExtraction {
            config: BackgroundConfig::default(),
            correction_mode: CorrectionMode::Subtract,
            strength: 1.0,
            protect_catalog_emission: true,
        };

        let (fit, fallback) =
            fit_channel_background(&image, &extraction, vec![whole_image.clone()], |error| {
                fallback_messages.push(error.to_string())
            })
            .unwrap();

        assert!(fallback.is_some());
        assert_eq!(fallback_messages, fallback.into_iter().collect::<Vec<_>>());
        assert_eq!(fit.diagnostics.protected_regions, 0);

        let (unprotected_fit, fallback) = fit_channel_background(
            &image,
            &StackBackgroundExtraction {
                protect_catalog_emission: false,
                ..extraction
            },
            vec![whole_image],
            |_| panic!("disabled protection must not need a fallback"),
        )
        .unwrap();
        assert!(fallback.is_none());
        assert_eq!(unprotected_fit.diagnostics.protected_regions, 0);
    }

    #[test]
    fn progress_ledger_accounts_for_every_pipeline_phase() {
        let processing = StackColorProcessing {
            background_extraction: Some(StackBackgroundExtraction {
                config: BackgroundConfig::default(),
                correction_mode: CorrectionMode::Subtract,
                strength: 1.0,
                protect_catalog_emission: true,
            }),
            input_deconvolutions: BTreeMap::from([(
                StackColorRole::Red,
                seiza_deconvolution::DeconvolutionConfig::conservative(3.1),
            )]),
            input_stretches: BTreeMap::from([
                (
                    StackColorRole::Red,
                    vec![super::super::stretch::StackStretchRequest {
                        model: seiza_stretch::StretchModel::Identity,
                        color_strategy: seiza_stretch::ColorStrategy::Linked,
                    }],
                ),
                (
                    StackColorRole::Green,
                    vec![
                        super::super::stretch::StackStretchRequest {
                            model: seiza_stretch::StretchModel::Identity,
                            color_strategy: seiza_stretch::ColorStrategy::Linked,
                        },
                        super::super::stretch::StackStretchRequest {
                            model: seiza_stretch::StretchModel::Identity,
                            color_strategy: seiza_stretch::ColorStrategy::Linked,
                        },
                    ],
                ),
            ]),
            output_stretches: vec![super::super::stretch::StackStretchRequest {
                model: seiza_stretch::StretchModel::Identity,
                color_strategy: seiza_stretch::ColorStrategy::Linked,
            }],
        };

        let progress = color_progress(3, Some(&processing));

        assert_eq!(progress.phases.len(), 12);
        assert_eq!(progress.total_units, 25);
        assert_eq!(
            progress
                .phases
                .iter()
                .find(|phase| phase.phase == StackColorProgressPhase::StretchingInputs)
                .unwrap()
                .total_units,
            3
        );
        assert!(progress.phases.iter().any(|phase| {
            phase.phase == StackColorProgressPhase::BackgroundPreparation && phase.total_units == 6
        }));
        assert!(progress.phases.iter().any(|phase| {
            phase.phase == StackColorProgressPhase::DeconvolvingInputs && phase.total_units == 1
        }));
        assert!(progress.phases.iter().any(|phase| {
            phase.phase == StackColorProgressPhase::RenderingOriginal && phase.total_units == 1
        }));
        assert!(progress.phases.iter().any(|phase| {
            phase.phase == StackColorProgressPhase::RenderingScreen && phase.total_units == 1
        }));
        assert!(progress.phases.iter().any(|phase| {
            phase.phase == StackColorProgressPhase::PublishingArtifacts && phase.total_units == 1
        }));
    }

    #[test]
    fn disabled_deconvolution_keeps_legacy_processing_serialization() {
        let processing = StackColorProcessing::default();
        let encoded = serde_json::to_value(processing).unwrap();

        assert!(encoded.get("input_deconvolutions").is_none());
    }

    #[test]
    fn color_input_cache_identity_ignores_stretch_edits() {
        let sources = [
            StackColorRole::Red,
            StackColorRole::Green,
            StackColorRole::Blue,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, role)| StackColorSource {
            role,
            filter_name: role.label().into(),
            job_id: format!("{index:064x}"),
            group_index: 0,
            artifact_revision: format!("revision-{index}"),
            accepted_frames: 4,
            reference_image_id: Some(index as i32 + 1),
            sky_orientation: Some(stack_orientation(
                100,
                80,
                seiza_stacking::AffineTransform::IDENTITY,
            )),
            registration_transform: None,
        })
        .collect::<Vec<_>>();
        let mut first = StackColorProcessing::default();
        first.input_deconvolutions.insert(
            StackColorRole::Green,
            seiza_deconvolution::DeconvolutionConfig::conservative(3.1),
        );
        first.input_stretches.insert(
            StackColorRole::Red,
            vec![super::super::stretch::StackStretchRequest {
                model: seiza_stretch::StretchModel::Identity,
                color_strategy: seiza_stretch::ColorStrategy::Linked,
            }],
        );
        let mut changed_stretch = first.clone();
        changed_stretch
            .output_stretches
            .push(super::super::stretch::StackStretchRequest {
                model: seiza_stretch::StretchModel::AutoMtf(seiza_stretch::StretchParams {
                    target_median: 0.3,
                    shadows_clip: -2.8,
                }),
                color_strategy: seiza_stretch::ColorStrategy::Linked,
            });
        let protection = BTreeMap::new();
        let first_id = color_input_cache_id("db", 2, 3, &first, &sources, &protection).unwrap();
        let changed_stretch_id =
            color_input_cache_id("db", 2, 3, &changed_stretch, &sources, &protection).unwrap();
        let mut changed_linear = first;
        changed_linear
            .input_deconvolutions
            .get_mut(&StackColorRole::Green)
            .unwrap()
            .amount = 0.5;
        let changed_linear_id =
            color_input_cache_id("db", 2, 3, &changed_linear, &sources, &protection).unwrap();
        let changed_protection = BTreeMap::from([(
            StackColorRole::Red,
            StackBackgroundProtection {
                reference_image_id: 1,
                catalog_version: Some("catalog-v2".into()),
                object_names: vec!["California Nebula".into()],
                region_count: 1,
                region_fingerprint: "changed-region".into(),
            },
        )]);
        let changed_protection_id =
            color_input_cache_id("db", 2, 3, &changed_stretch, &sources, &changed_protection)
                .unwrap();

        assert_eq!(first_id, changed_stretch_id);
        assert_ne!(first_id, changed_linear_id);
        assert_ne!(first_id, changed_protection_id);
    }

    #[test]
    fn cached_color_inputs_retain_channel_registration() {
        let transform = seiza_stacking::SimilarityTransform {
            scale: 1.01,
            rotation_radians: 0.02,
            translation_x: 3.0,
            translation_y: -4.0,
        };
        let cached = CachedColorInputs {
            schema_version: COLOR_INPUT_CACHE_VERSION,
            input_id: "input".into(),
            roles: vec![StackColorRole::Red, StackColorRole::Green],
            registered_transforms: BTreeMap::from([
                (
                    StackColorRole::Red,
                    seiza_stacking::SimilarityTransform::IDENTITY,
                ),
                (StackColorRole::Green, transform),
            ]),
            resolved_backgrounds: BTreeMap::new(),
            background_protection_fallbacks: BTreeMap::new(),
            resolved_deconvolutions: BTreeMap::new(),
        };

        let encoded = serde_json::to_vec(&cached).unwrap();
        let decoded = serde_json::from_slice::<CachedColorInputs>(&encoded).unwrap();

        assert_eq!(
            decoded.registered_transforms[&StackColorRole::Green],
            transform
        );
    }

    #[test]
    fn input_stretches_keep_registration_gaps_for_color_cropping() {
        let covered = |left: usize, top: usize| {
            let data = (0..8 * 8)
                .map(|index| {
                    let x = index % 8;
                    let y = index / 8;
                    if x < left || y < top {
                        f32::NAN
                    } else {
                        0.25
                    }
                })
                .collect::<Vec<_>>();
            LinearImage::new(8, 8, 1, data).unwrap()
        };
        let stretch = StretchStack::new(vec![
            super::super::stretch::display_identity_config(),
            super::super::stretch::display_identity_config(),
        ])
        .unwrap();
        let green = apply_input_stretch_stack(&stretch, &covered(2, 0).data, 1, |_| {}).unwrap();
        let blue = apply_input_stretch_stack(&stretch, &covered(0, 3).data, 1, |_| {}).unwrap();
        assert_eq!(green.plans.len(), 2);
        assert!(green.data[0].is_nan());
        assert_eq!(green.data[2], 0.25);
        assert!(blue.data[0].is_nan());

        let green = LinearImage::new(8, 8, 1, green.data).unwrap();
        let blue = LinearImage::new(8, 8, 1, blue.data).unwrap();
        let composition = combine_rgb(
            &covered(0, 0),
            &green,
            &blue,
            &ColorOptions {
                normalization: ColorNormalization::None,
                crop: ColorCrop::Bounds,
                ..ColorOptions::default()
            },
        )
        .unwrap();
        let report = composition.crop.expect("a cropped composition");
        assert_eq!((report.region.x, report.region.y), (2, 3));
        assert_eq!((report.region.width, report.region.height), (6, 5));
    }
}
